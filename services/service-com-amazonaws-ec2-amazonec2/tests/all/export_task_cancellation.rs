// SPDX-License-Identifier: CC0-1.0

//! What `CancelExportTask` cancels and what it leaves alone, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test are:
//!
//! 1. AN ID THAT IS NOT ONE EC2 COULD HAVE ISSUED NAMES NOTHING TO LOOK UP, so it is
//!    refused 400 `InvalidID` before the store is reached, and only an id EC2 could have
//!    issued that resolves to nothing is `InvalidExportTaskID.NotFound`. The form of an
//!    export task id is the one the two operations the id member documents as its sources
//!    mint it in — `export-i-<hex>` and `export-ami-<hex>`
//!    (`ec2::ops::cancel_export_task::EXPORT_TASK_ID_PREFIXES`), in the 8- and 17-hex-digit
//!    resource-id forms;
//! 2. A CANCEL THE TASK'S STATE DOES NOT ADMIT IS REFUSED with `IncorrectState` and writes
//!    nothing: "if the export task is complete or is in the process of transferring the
//!    final disk image, the command fails and returns an error" (`CancelExportTask`);
//! 3. A CANCEL THAT IS ADMITTED MOVES ONLY THE STATE OF ONLY THE TASK IT NAMES, into the
//!    non-terminal `cancelling`, and the clock owes it `cancelled` — so a caller acting
//!    straight after the call still sees `cancelling`.
//!
//! Each row is a distinct input along what the rules generalise over — which minting
//! prefix, which id length, which way an id departs from that form, which state the task is
//! in — so no row is the specification. The id rows are GENERATED from the crate's own
//! prefix catalog, so a prefix added there is covered here without a new literal.

use std::time::{Duration, Instant};

use ec2::ops::cancel_export_task::{
    EXPORT_TASK_ID_PREFIXES, IMAGE_EXPORT_TASK_ID_PREFIX, INSTANCE_EXPORT_TASK_ID_PREFIX,
    export_task_id_is_well_formed,
};
use ec2::ops::types::{ExportTask, ExportToS3Task, InstanceExportDetails};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, EXPORT_TASK_STATE_ACTIVE,
    EXPORT_TASK_STATE_CANCELLED, EXPORT_TASK_STATE_CANCELLING, EXPORT_TASK_STATE_COMPLETED,
    ExportTaskRecord, INCORRECT_STATE, INVALID_EXPORT_TASK_ID_NOT_FOUND,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, unescaped};
use super::volume_lifecycle::{REGION, p, past_the_transition};

/// The code EC2 answers for an id it cannot parse as any resource id, as
/// `crate::scaffolded::Scaffolded::invalid_id` raises it.
const INVALID_ID: &str = "InvalidID";

/// The suffix lengths an EC2 resource id is minted with: the long (17 hex digit) form EC2
/// mints today and the short (8 hex digit) form it minted before long ids, both of which
/// `crate::state::prefixed_id_is_well_formed` accepts.
const MINTED_SUFFIXES: [(&str, &str); 2] = [
    ("the long (17 hex digit) form", "0123456789abcdef0"),
    ("the short (8 hex digit) form", "0123abcd"),
];

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `CancelExportTask` call for `export_task_id`, signed for the Region the suite works
/// in.
async fn cancel(router: &MultiServiceRouter, export_task_id: &str) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "CancelExportTask",
        &[p("ExportTaskId", export_task_id)],
    )
    .await
}

/// Every id form an export task EC2 issued can carry: each minting prefix the crate
/// publishes crossed with each minted suffix length.
fn minted_id_rows() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for prefix in EXPORT_TASK_ID_PREFIXES {
        for (length, suffix) in MINTED_SUFFIXES {
            rows.push((
                format!("a {prefix} id in {length}"),
                format!("{prefix}-{suffix}"),
            ));
        }
    }
    rows
}

/// Values that are NOT export task ids: each row departs from the minted form in one way —
/// a suffix one digit short of a minted length or one past it, a digit outside lowercase
/// hexadecimal, a missing or unknown kind segment, another resource's id, and a value of no
/// resource-id shape at all.
fn malformed_id_rows() -> Vec<(&'static str, &'static str)> {
    vec![
        ("one digit short of the short form", "export-i-0123abc"),
        ("one digit past the short form", "export-i-0123abcde"),
        ("one digit short of the long form", "export-i-0123456789abcdef"),
        ("one digit past the long form", "export-i-0123456789abcdef01"),
        ("an uppercase suffix", "export-i-0123ABCD"),
        ("a suffix that is not hexadecimal", "export-i-0123abcg"),
        ("an empty suffix", "export-i-"),
        ("no kind segment", "export-0123abcd"),
        ("a kind EC2 issues no export task under", "export-snap-0123abcd"),
        ("another resource's id", "vol-0123456789abcdef0"),
        ("a value of no resource-id shape", "not an export task"),
    ]
}

/// Store one export task under `export_task_id` in `state`, with every member a task
/// carries filled in, so an assertion that the cancel moved ONLY the state has something
/// to compare.
fn seed(engine: &engine::InMemoryEngine, export_task_id: &str, state: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard.export_tasks.insert(
        export_task_id.to_string(),
        ExportTaskRecord {
            view: ExportTask {
                export_task_id: Some(export_task_id.to_string()),
                description: Some("an export of one instance".to_string()),
                state: Some(state.to_string()),
                status_message: None,
                export_to_s3_task: Some(ExportToS3Task {
                    s3_bucket: Some("my-vm-exports".to_string()),
                    s3_key: Some("vms/instance.ova".to_string()),
                    container_format: Some("ova".to_string()),
                    disk_image_format: Some("VMDK".to_string()),
                }),
                instance_export_details: Some(InstanceExportDetails {
                    instance_id: Some("i-0123456789abcdef0".to_string()),
                    target_environment: Some("vmware".to_string()),
                }),
                tags: Some(Vec::new()),
            },
            cancelled_at: None,
        },
    );
}

/// The stored export task `export_task_id`, as the store holds it.
fn stored(engine: &engine::InMemoryEngine, export_task_id: &str) -> ExportTaskRecord {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .export_tasks
        .get(export_task_id)
        .cloned()
        .unwrap_or_else(|| panic!("the store holds the export task {export_task_id}"))
}

/// The export task ids the store holds, in its own order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.export_tasks.keys().cloned().collect()
}

/// Every member of a task but its `State`, as `(member, value)` pairs: what a cancel must
/// leave exactly as it found it.
///
/// Named pairs rather than one rendering of the whole shape, so a failure names the member
/// that moved — `ExportTask` derives no `PartialEq` of its own.
fn members_apart_from_state(task: &ExportTask) -> Vec<(&'static str, Option<String>)> {
    let s3_task = task.export_to_s3_task.as_ref();
    let details = task.instance_export_details.as_ref();
    vec![
        ("ExportTaskId", task.export_task_id.clone()),
        ("Description", task.description.clone()),
        ("StatusMessage", task.status_message.clone()),
        (
            "ExportToS3Task.S3Bucket",
            s3_task.and_then(|s3_task| s3_task.s3_bucket.clone()),
        ),
        (
            "ExportToS3Task.S3Key",
            s3_task.and_then(|s3_task| s3_task.s3_key.clone()),
        ),
        (
            "ExportToS3Task.ContainerFormat",
            s3_task.and_then(|s3_task| s3_task.container_format.clone()),
        ),
        (
            "ExportToS3Task.DiskImageFormat",
            s3_task.and_then(|s3_task| s3_task.disk_image_format.clone()),
        ),
        (
            "InstanceExportDetails.InstanceId",
            details.and_then(|details| details.instance_id.clone()),
        ),
        (
            "InstanceExportDetails.TargetEnvironment",
            details.and_then(|details| details.target_environment.clone()),
        ),
        (
            "Tags",
            task.tags.as_ref().map(|tags| tags.len().to_string()),
        ),
    ]
}

/// Start a cancellation of `export_task_id` that will not fall due during the test, so a
/// row about the `cancelling` STATE never depends on how long the test itself took. The
/// span the clock actually owes is what
/// [`every_minted_export_task_id_form_cancels_and_finishes_on_the_clock`] asserts.
fn hold_in_cancelling(engine: &engine::InMemoryEngine, export_task_id: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .export_tasks
        .get_mut(export_task_id)
        .unwrap_or_else(|| panic!("the store holds the export task {export_task_id}"))
        .cancelled_at = Some(Instant::now() + Duration::from_secs(60));
}

/// Rule 3: an active task cancels whatever form its id was minted in, the cancel reports
/// `cancelling` straight away and `cancelled` only once the clock has reached it, and it
/// moves no other member of the task.
#[tokio::test]
async fn every_minted_export_task_id_form_cancels_and_finishes_on_the_clock() {
    let (router, engine) = emulator();
    let rows = minted_id_rows();
    for (case, export_task_id) in &rows {
        assert!(
            export_task_id_is_well_formed(export_task_id),
            "case {case:?}: the minted id {export_task_id} must be one the handler accepts"
        );
        seed(&engine, export_task_id, EXPORT_TASK_STATE_ACTIVE);
    }

    for (case, export_task_id) in &rows {
        let before = stored(&engine, export_task_id);
        let (status, body) = cancel(&router, export_task_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let after = stored(&engine, export_task_id);
        let state = after.state_at(Instant::now());
        assert_eq!(
            state, EXPORT_TASK_STATE_CANCELLING,
            "case {case:?}: straight after the cancel the task is \
             {EXPORT_TASK_STATE_CANCELLING}, got {state:?}"
        );
        let unchanged = members_apart_from_state(&after.view_at(Instant::now()));
        assert_eq!(
            unchanged,
            members_apart_from_state(&before.view),
            "case {case:?}: the cancel moves only the task's state, got {unchanged:?}"
        );
    }

    past_the_transition().await;

    for (case, export_task_id) in &rows {
        let state = stored(&engine, export_task_id).state_at(Instant::now());
        assert_eq!(
            state, EXPORT_TASK_STATE_CANCELLED,
            "case {case:?}: once the cancellation falls due the task is \
             {EXPORT_TASK_STATE_CANCELLED}, got {state:?}"
        );
    }
}

/// Rule 3: a cancel moves the task it names and nothing else — the account's other tasks
/// keep the state they were in, and the cancelled task stays in the store, since a state
/// is only reportable while the record is there.
#[tokio::test]
async fn a_cancel_touches_only_the_task_it_names() {
    let (router, engine) = emulator();
    let cancelled = format!("{INSTANCE_EXPORT_TASK_ID_PREFIX}-0123456789abcdef0");
    let still_active = format!("{IMAGE_EXPORT_TASK_ID_PREFIX}-0123456789abcdef0");
    let still_completed = format!("{INSTANCE_EXPORT_TASK_ID_PREFIX}-0123abcd");
    seed(&engine, &cancelled, EXPORT_TASK_STATE_ACTIVE);
    seed(&engine, &still_active, EXPORT_TASK_STATE_ACTIVE);
    seed(&engine, &still_completed, EXPORT_TASK_STATE_COMPLETED);

    let (status, body) = cancel(&router, &cancelled).await;
    assert_eq!(
        status, 200,
        "cancelling one of three tasks: expected HTTP 200, got {status} with body {body}"
    );

    // `(case, the task, the state it must be in after the cancel of another task)`.
    let expected = [
        ("the task the cancel named", &cancelled, EXPORT_TASK_STATE_CANCELLING),
        ("another active task", &still_active, EXPORT_TASK_STATE_ACTIVE),
        ("a completed task", &still_completed, EXPORT_TASK_STATE_COMPLETED),
    ];
    for (case, export_task_id, expected_state) in expected {
        let state = stored(&engine, export_task_id).state_at(Instant::now());
        assert_eq!(
            state, expected_state,
            "case {case:?}: expected {expected_state}, got {state:?}"
        );
    }

    let mut expected_ids = vec![
        cancelled.clone(),
        still_active.clone(),
        still_completed.clone(),
    ];
    expected_ids.sort();
    assert_eq!(
        stored_ids(&engine),
        expected_ids,
        "a cancel neither removes a record nor re-keys one, so all three tasks stay \
         reportable under the ids they were stored under"
    );
}

/// Rule 2: a cancel the task's state does not admit is refused `IncorrectState` and leaves
/// the task exactly as it was — no state moved, no cancellation started.
#[tokio::test]
async fn a_cancel_the_tasks_state_refuses_changes_nothing() {
    let (router, engine) = emulator();
    let cancelling = format!("{INSTANCE_EXPORT_TASK_ID_PREFIX}-0123456789abcdef0");
    let cancelled = format!("{IMAGE_EXPORT_TASK_ID_PREFIX}-0123456789abcdef0");
    let completed = format!("{INSTANCE_EXPORT_TASK_ID_PREFIX}-0123abcd");
    seed(&engine, &cancelling, EXPORT_TASK_STATE_ACTIVE);
    seed(&engine, &cancelled, EXPORT_TASK_STATE_ACTIVE);
    seed(&engine, &completed, EXPORT_TASK_STATE_COMPLETED);

    // The two halves of the cancellation span: one task held in `cancelling` for however
    // long this test takes, and one whose cancellation is let fall due below.
    hold_in_cancelling(&engine, &cancelling);
    let (status, body) = cancel(&router, &cancelled).await;
    assert_eq!(
        status, 200,
        "the first cancel of {cancelled} must be admitted, got {status} with body {body}"
    );

    refused_cancel_leaves_the_task_alone(
        &router,
        &engine,
        "a task already cancelling",
        &cancelling,
        EXPORT_TASK_STATE_CANCELLING,
    )
    .await;
    refused_cancel_leaves_the_task_alone(
        &router,
        &engine,
        "a task that has completed",
        &completed,
        EXPORT_TASK_STATE_COMPLETED,
    )
    .await;

    past_the_transition().await;

    refused_cancel_leaves_the_task_alone(
        &router,
        &engine,
        "a task already cancelled",
        &cancelled,
        EXPORT_TASK_STATE_CANCELLED,
    )
    .await;
}

/// One row of rule 2: the task is in `expected_state`, the cancel is refused
/// `IncorrectState`, and the record is unchanged by the refusal.
async fn refused_cancel_leaves_the_task_alone(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    export_task_id: &str,
    expected_state: &str,
) {
    let before = stored(engine, export_task_id);
    let state_before = before.state_at(Instant::now());
    assert_eq!(
        state_before, expected_state,
        "case {case:?}: the row's task must start in {expected_state}, got {state_before:?}"
    );

    let (status, body) = cancel(router, export_task_id).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INCORRECT_STATE,
        "case {case:?}: expected {INCORRECT_STATE}, got {body}"
    );

    let after = stored(engine, export_task_id);
    let state_after = after.state_at(Instant::now());
    assert_eq!(
        state_after, expected_state,
        "case {case:?}: a refused cancel leaves the task in {expected_state}, got \
         {state_after:?}"
    );
    assert_eq!(
        after.cancelled_at, before.cancelled_at,
        "case {case:?}: a refused cancel starts no cancellation, got {:?}",
        after.cancelled_at
    );
}

/// Rule 1: an id outside the minted form is refused `InvalidID` with the message naming the
/// id it was sent, while an id EC2 could have issued that the account holds no task for is
/// `InvalidExportTaskID.NotFound` — and neither refusal writes anything.
#[tokio::test]
async fn an_id_ec2_could_not_have_issued_is_refused_before_any_lookup() {
    let (router, engine) = emulator();

    for (case, export_task_id) in malformed_id_rows() {
        assert!(
            !export_task_id_is_well_formed(export_task_id),
            "case {case:?}: {export_task_id} must not read as a minted export task id"
        );
        let (status, body) = cancel(&router, export_task_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_ID,
            "case {case:?}: expected {INVALID_ID}, got {body}"
        );
        // The message EC2 builds for an id it cannot parse names the id it was sent. The
        // XML body escapes the quotes around it (`&apos;`), so the text is compared.
        let message = element(&body, "Message").map(|message| unescaped(&message));
        assert_eq!(
            message,
            Some(format!("The ID '{export_task_id}' is not valid")),
            "case {case:?}: expected the {INVALID_ID} message to name the id, got {message:?}"
        );
    }

    for (case, export_task_id) in minted_id_rows() {
        let (status, body) = cancel(&router, &export_task_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_EXPORT_TASK_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_EXPORT_TASK_ID_NOT_FOUND}, got {body}"
        );
    }

    let held = stored_ids(&engine);
    assert!(
        held.is_empty(),
        "a refused cancel stores nothing, so the account still holds no export task, got \
         {held:?}"
    );
}

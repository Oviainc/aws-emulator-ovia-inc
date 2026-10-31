// SPDX-License-Identifier: CC0-1.0

//! What `CancelConversionTask` cancels and what it leaves alone, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test are:
//!
//! 1. AN ID THAT IS NOT ONE EC2 COULD HAVE ISSUED NAMES NOTHING TO LOOK UP, so its FORM is
//!    judged before the store is reached: it is refused 400 `InvalidConversionTaskId`
//!    naming the parameter and the value it was sent, while an id EC2 could have issued that
//!    the account holds no task for is the same code reported as a task that does not
//!    exist. The form of a conversion task id is the one the operations that mint one use —
//!    `import-i-<hex>` for the import of an instance and `import-vol-<hex>` for the import
//!    of a volume, "the import of an instance or volume" (`CancelConversionTask`) — in the
//!    8- and 17-hex-digit resource-id forms;
//! 2. A CANCEL THE TASK'S STATE DOES NOT ADMIT IS REFUSED with `IncorrectState` and writes
//!    nothing: "cancels an ACTIVE conversion task … if the conversion is complete or is in
//!    the process of transferring the final disk image, the command fails and returns an
//!    exception" (`CancelConversionTask`), so a task that is not converting any more has
//!    nothing left to cancel;
//! 3. A CANCEL THAT IS ADMITTED MOVES THE STATE OF ONLY THE TASK IT NAMES, into the
//!    non-terminal `cancelling` — which the next caller is shown — and the clock owes it
//!    `cancelled`, so a caller acting straight after the call still sees the task working
//!    through it. The reason the request gave is what the task reports, and every volume of
//!    an instance import stops where the task did;
//! 4. `DryRun` ANSWERS WHAT THE REQUEST WOULD HAVE DONE and writes nothing: the refusal a
//!    request would have been refused with, or `DryRunOperation` for one that would have
//!    worked.
//!
//! Each row is a distinct input along what the rules generalise over — which resource the
//! conversion imports, which minting prefix, which id length, which way an id departs from
//! that form, which state the task is in, how many volumes the import carries, whether a
//! reason was given and how long it is — so no row is the specification. The id rows are
//! GENERATED from the crate's own prefixes, so a kind added there is covered here without a
//! new literal.
//!
//! A conversion task is seeded through
//! [`ec2::state::ComAmazonawsEc2Amazonec2State::insert_conversion_task`], the writer
//! `ImportInstance` and `ImportVolume` will use once they are implemented (both still
//! scaffolds in this crate). What a cancel DID is read back over the wire: this operation
//! applies every clock-owed move before it judges a request, so the state it names in a
//! later refusal is the state a reader of the task is shown.

use std::time::Duration;

use ec2::ops::types::{
    ConversionTask, DiskImageDescription, DiskImageVolumeDescription, ImportInstanceTaskDetails,
    ImportInstanceVolumeDetailItem, ImportVolumeTaskDetails, Tag,
};
use ec2::state::{
    CONVERSION_TASK_ID_PARAMETER, CONVERSION_TASK_ID_PREFIXES, CONVERSION_TASK_STATE_ACTIVE,
    CONVERSION_TASK_STATE_CANCELLED, CONVERSION_TASK_STATE_CANCELLING,
    CONVERSION_TASK_STATE_COMPLETED, CONVERSION_TASK_STATES,
    ComAmazonawsEc2Amazonec2State as Ec2State, ConversionTaskDetail, ConversionTaskSpec,
    DRY_RUN_OPERATION, IMPORT_INSTANCE_CONVERSION_TASK_ID_PREFIX,
    IMPORT_VOLUME_CONVERSION_TASK_ID_PREFIX, INCORRECT_STATE, INVALID_CONVERSION_TASK_ID,
    INVALID_PARAMETER_VALUE, MAX_IMPORT_TASK_CANCEL_REASON_CHARS, MISSING_PARAMETER,
    TRANSITION_DELAY, conversion_task_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message, member};

/// The Region every call here is signed for. A conversion task carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// The suffix lengths an EC2 resource id is minted with: the long (17 hex digit) form EC2
/// mints today and the short (8 hex digit) form it minted before long ids, both of which
/// `ec2::state::prefixed_id_is_well_formed` accepts.
const MINTED_SUFFIXES: [(&str, &str); 2] = [
    ("the long (17 hex digit) form", "0123456789abcdef0"),
    ("the short (8 hex digit) form", "0123abcd"),
];

/// The two kinds of import one conversion task can be: "the task can be the import of an
/// instance or volume" (`CancelConversionTask`). Every rule here holds for both, so each
/// table row carries the kind it exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A whole machine, as `ImportInstance` imports one.
    Instance,
    /// A bare disk, as `ImportVolume` imports one.
    Volume,
}

impl Kind {
    /// The id prefix a task of this kind is minted under.
    fn id_prefix(self) -> &'static str {
        match self {
            Self::Instance => IMPORT_INSTANCE_CONVERSION_TASK_ID_PREFIX,
            Self::Volume => IMPORT_VOLUME_CONVERSION_TASK_ID_PREFIX,
        }
    }
}

/// One `CancelConversionTask` call carrying `params`.
async fn cancel(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CancelConversionTask", params).await
}

/// One `CancelConversionTask` call that names nothing but the task.
async fn cancel_task(router: &MultiServiceRouter, conversion_task_id: &str) -> (u16, String) {
    cancel(router, &[p("ConversionTaskId", conversion_task_id)]).await
}

/// The disk image one seeded import reads, filled in so an assertion that a cancel moved
/// only the state has something to compare.
fn disk_image(case: &str) -> DiskImageDescription {
    DiskImageDescription {
        checksum: Some("d41d8cd98f00b204e9800998ecf8427e".to_string()),
        format: Some("VMDK".to_string()),
        import_manifest_url: Some(format!("https://disk-images.s3.amazonaws.com/{case}.manifest")),
        size: Some(8),
    }
}

/// Store one conversion task of `kind`, over `volumes` disk images for an instance import,
/// and answer the id it was minted under.
///
/// Every seeded task starts `active`, which is the state the writer mints and the only state
/// a cancel is admitted from.
fn seed_task(engine: &engine::InMemoryEngine, case: &str, kind: Kind, volumes: usize) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let detail = match kind {
        Kind::Instance => ConversionTaskDetail::Instance(ImportInstanceTaskDetails {
            description: Some(format!("the import of {case}")),
            instance_id: Some("i-0123456789abcdef0".to_string()),
            platform: Some("Windows".to_string()),
            volumes: Some(
                (0..volumes)
                    .map(|index| ImportInstanceVolumeDetailItem {
                        availability_zone: Some(format!("{REGION}a")),
                        bytes_converted: Some(0),
                        description: Some(format!("disk {index}")),
                        image: Some(disk_image(&format!("{case}-{index}"))),
                        status: Some(CONVERSION_TASK_STATE_ACTIVE.to_string()),
                        volume: Some(DiskImageVolumeDescription {
                            id: Some(format!("vol-0123456789abcde{index}")),
                            size: Some(8),
                        }),
                        ..ImportInstanceVolumeDetailItem::default()
                    })
                    .collect(),
            ),
        }),
        Kind::Volume => ConversionTaskDetail::Volume(ImportVolumeTaskDetails {
            availability_zone: Some(format!("{REGION}a")),
            bytes_converted: Some(0),
            description: Some(format!("the import of {case}")),
            image: Some(disk_image(case)),
            volume: Some(DiskImageVolumeDescription {
                id: Some("vol-0123456789abcdef0".to_string()),
                size: Some(8),
            }),
            ..ImportVolumeTaskDetails::default()
        }),
    };
    guard
        .insert_conversion_task(ConversionTaskSpec {
            detail,
            expiration_time: Some("2026-11-30T00:00:00.000Z".to_string()),
            tags: vec![Tag {
                key: Some("Name".to_string()),
                value: Some(case.to_string()),
            }],
        })
        .unwrap_or_else(|| panic!("case {case:?}: the store must store the conversion task"))
}

/// Store one conversion task of `kind` already in `state`.
///
/// The state is moved on the stored record, because the writer mints the `active` state a
/// task AWS hands back: a task in any other state got there through this operation or
/// through the import finishing, neither of which a row ABOUT that state can use to set it
/// up. `None` seeds a task carrying no state at all.
fn seed_task_in_state(
    engine: &engine::InMemoryEngine,
    case: &str,
    kind: Kind,
    state: Option<&str>,
) -> String {
    let conversion_task_id = seed_task(engine, case, kind, 1);
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let task = guard
        .conversion_tasks
        .get_mut(&conversion_task_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds the task it just stored"));
    task.state = state.map(str::to_string);
    for item in task
        .import_instance
        .iter_mut()
        .flat_map(|detail| detail.volumes.iter_mut().flatten())
    {
        item.status = state.map(str::to_string);
    }
    conversion_task_id
}

/// The task `conversion_task_id` as the store holds it.
fn stored(engine: &engine::InMemoryEngine, case: &str, conversion_task_id: &str) -> ConversionTask {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .conversion_tasks
        .get(conversion_task_id)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold the task {conversion_task_id}"))
}

/// The status of every volume the task reports, in its own order — an instance import's
/// disks, a volume import carrying none of its own.
fn volume_states(task: &ConversionTask) -> Vec<Option<String>> {
    task.import_instance
        .iter()
        .flat_map(|detail| detail.volumes.iter().flatten())
        .map(|item| item.status.clone())
        .collect()
}

/// Every conversion task id the store holds, in its own order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.conversion_tasks.keys().cloned().collect()
}

/// Whether the clock owes `conversion_task_id` the end of a cancellation.
fn cancellation_owed(engine: &engine::InMemoryEngine, conversion_task_id: &str) -> bool {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .conversion_task_cancelled_at
        .contains_key(conversion_task_id)
}

/// The state a later caller is shown for `conversion_task_id`, read over the WIRE: a cancel
/// of a task that is not `active` is refused with the state the task is in, and this
/// operation applies every clock-owed move before it judges the request, so the state it
/// names is the one a reader of the task would be shown.
async fn state_over_the_wire(
    router: &MultiServiceRouter,
    case: &str,
    conversion_task_id: &str,
) -> String {
    let (status, body) = cancel_task(router, conversion_task_id).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a task that is not active refuses a second cancel, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        INCORRECT_STATE,
        "case {case:?}: expected {INCORRECT_STATE}, got {body}"
    );
    let message = error_message(&body);
    let (_, after) = message.split_once("is in the '").unwrap_or_else(|| {
        panic!("case {case:?}: the {INCORRECT_STATE} message must name the state, got {message:?}")
    });
    after
        .split_once('\'')
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the {INCORRECT_STATE} message must quote the state, got \
                 {message:?}"
            )
        })
        .0
        .to_string()
}

// ---------------------------------------------------------------------------
// RULE 1: the id's FORM is judged before any lookup
// ---------------------------------------------------------------------------

/// Values that are NOT conversion task ids: each row departs from the minted form in one
/// way — a suffix a digit short of a minted length or a digit past it, a digit outside
/// lowercase hexadecimal, an empty suffix, a missing or unknown kind segment, another
/// resource's id, and a value of no resource-id shape at all. Generated over BOTH minting
/// prefixes, so neither kind's form is asserted only through the other's.
fn malformed_id_rows() -> Vec<(String, String)> {
    let departures = [
        ("one digit short of the short form", "0123abc"),
        ("one digit past the short form", "0123abcde"),
        ("one digit short of the long form", "0123456789abcdef"),
        ("one digit past the long form", "0123456789abcdef01"),
        ("an uppercase suffix", "0123ABCD"),
        ("a suffix that is not hexadecimal", "0123abcg"),
        ("an empty suffix", ""),
    ];
    let mut rows: Vec<(String, String)> = Vec::new();
    for prefix in CONVERSION_TASK_ID_PREFIXES {
        for (case, suffix) in departures {
            rows.push((
                format!("an {prefix}- id with {case}"),
                format!("{prefix}-{suffix}"),
            ));
        }
    }
    for (case, id) in [
        ("no kind segment at all", "import-00000000"),
        ("no kind segment, in the long form", "import-0123456789abcdef0"),
        ("a kind EC2 issues no conversion task under", "import-ami-0123abcd"),
        ("another resource's id", "export-i-0123abcd"),
        ("a value of no resource-id shape", "not a conversion task"),
    ] {
        rows.push((case.to_string(), id.to_string()));
    }
    rows
}

/// Every id form a conversion task EC2 issued can carry: each kind's minting prefix crossed
/// with each minted suffix length.
fn minted_id_rows() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for kind in [Kind::Instance, Kind::Volume] {
        for (length, suffix) in MINTED_SUFFIXES {
            rows.push((
                format!("a {kind:?} conversion task id in {length}"),
                format!("{}-{suffix}", kind.id_prefix()),
            ));
        }
    }
    rows
}

/// Rule 1: an id outside the minted form is refused for its FORMAT, naming the parameter and
/// the value the request sent, while an id EC2 could have issued that the account holds no
/// task for is reported as a task that does not exist — both under
/// `InvalidConversionTaskId`, and neither writing anything.
#[tokio::test]
async fn an_id_ec2_could_not_have_issued_is_refused_before_any_lookup() {
    let (router, engine) = emulator();

    for (case, conversion_task_id) in malformed_id_rows() {
        assert!(
            !conversion_task_id_is_well_formed(&conversion_task_id),
            "case {case:?}: {conversion_task_id} must not read as a minted conversion task id"
        );
        let (status, body) = cancel_task(&router, &conversion_task_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_CONVERSION_TASK_ID,
            "case {case:?}: expected {INVALID_CONVERSION_TASK_ID}, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!(
                "Parameter {CONVERSION_TASK_ID_PARAMETER} ({conversion_task_id}) has an invalid \
                 format."
            ),
            "case {case:?}: expected the format refusal to name the parameter and the value, \
             got {message:?}"
        );
    }

    for (case, conversion_task_id) in minted_id_rows() {
        let (status, body) = cancel_task(&router, &conversion_task_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_CONVERSION_TASK_ID,
            "case {case:?}: expected {INVALID_CONVERSION_TASK_ID}, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("The conversion task '{conversion_task_id}' does not exist"),
            "case {case:?}: expected the not-found message to name the id, got {message:?}"
        );
    }

    let held = stored_ids(&engine);
    assert!(
        held.is_empty(),
        "a refused cancel stores nothing, so the account still holds no conversion task, got \
         {held:?}"
    );
}

/// Rule 1: a request that names no task at all names nothing to cancel, so it is refused
/// `MissingParameter` rather than acted on — `ConversionTaskId` is `@required` AND
/// `@clientOptional`, so a field sent EMPTY reaches the handler, where an absent one is
/// refused by the boundary validator in front of it (asserted below).
#[tokio::test]
async fn a_cancel_that_names_no_task_is_refused_as_a_missing_parameter() {
    let (router, engine) = emulator();
    let seeded = seed_task(&engine, "an-untouched-import", Kind::Volume, 0);
    // `(case, the parameters under test)`: each a value an ec2Query body cannot tell from
    // absent, beside the other members a cancel may carry.
    let rows: Vec<(&str, Params)> = vec![
        ("an empty ConversionTaskId", vec![p("ConversionTaskId", "")]),
        (
            "an empty ConversionTaskId beside a reason",
            vec![
                p("ConversionTaskId", ""),
                p("ReasonMessage", "no longer needed"),
            ],
        ),
        (
            "an empty ConversionTaskId on a dry run",
            vec![p("ConversionTaskId", ""), p("DryRun", "true")],
        ),
    ];
    for (case, params) in rows {
        let (status, body) = cancel(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            MISSING_PARAMETER,
            "case {case:?}: expected {MISSING_PARAMETER}, got {body}"
        );
        let task = stored(&engine, case, &seeded);
        assert_eq!(
            task.state.as_deref(),
            Some(CONVERSION_TASK_STATE_ACTIVE),
            "case {case:?}: a refused cancel moves no task, got {task:?}"
        );
    }

    // A request carrying no `ConversionTaskId` field at all never reaches the handler: the
    // modelled `@required` member is refused in front of it, which is a 400 of its own and
    // writes nothing either.
    let case = "no ConversionTaskId field at all";
    let (status, body) = cancel(&router, &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let task = stored(&engine, case, &seeded);
    assert_eq!(
        task.state.as_deref(),
        Some(CONVERSION_TASK_STATE_ACTIVE),
        "case {case:?}: a request refused at the boundary moves no task, got {task:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: an admitted cancel moves the task into `cancelling`, the clock finishes it
// ---------------------------------------------------------------------------

/// One row of the admitted-cancel table: the case, which resource the conversion imports,
/// how many volumes an instance import carries, and the reason the cancel names (if any).
struct CancelRow {
    case: &'static str,
    kind: Kind,
    volumes: usize,
    reason: Option<String>,
}

/// The admitted-cancel rows: both kinds of conversion, imports of one and of several
/// volumes, a cancel with a reason and one without, and a reason at the width this service
/// publishes.
fn cancel_rows() -> Vec<CancelRow> {
    vec![
        CancelRow {
            case: "one-volume-instance-import-with-a-reason",
            kind: Kind::Instance,
            volumes: 1,
            reason: Some("the wrong disk image was uploaded".to_string()),
        },
        CancelRow {
            case: "three-volume-instance-import-without-a-reason",
            kind: Kind::Instance,
            volumes: 3,
            reason: None,
        },
        CancelRow {
            case: "two-volume-instance-import-with-a-reason-at-the-published-width",
            kind: Kind::Instance,
            volumes: 2,
            reason: Some("r".repeat(MAX_IMPORT_TASK_CANCEL_REASON_CHARS)),
        },
        CancelRow {
            case: "volume-import-with-a-reason",
            kind: Kind::Volume,
            volumes: 0,
            reason: Some("cancelled by the nightly job".to_string()),
        },
        CancelRow {
            case: "volume-import-without-a-reason",
            kind: Kind::Volume,
            volumes: 0,
            reason: None,
        },
    ]
}

/// Rule 3: a cancel of an `active` task of either kind is admitted, leaves the task in the
/// non-terminal `cancelling` the next caller is shown, carries the reason the request gave
/// onto the task and its volumes, and only the CLOCK takes it to `cancelled`.
#[tokio::test]
async fn an_active_conversion_task_of_either_kind_cancels_and_finishes_on_the_clock() {
    let (router, engine) = emulator();
    let rows = cancel_rows();
    let mut ids: Vec<(&CancelRow, String)> = Vec::new();
    for row in &rows {
        let conversion_task_id = seed_task(&engine, row.case, row.kind, row.volumes);
        let before = stored(&engine, row.case, &conversion_task_id);
        assert_eq!(
            before.state.as_deref(),
            Some(CONVERSION_TASK_STATE_ACTIVE),
            "case {:?}: the row's task must start {CONVERSION_TASK_STATE_ACTIVE}, got {before:?}",
            row.case
        );
        ids.push((row, conversion_task_id));
    }

    for (row, conversion_task_id) in &ids {
        let mut params = vec![p("ConversionTaskId", conversion_task_id.as_str())];
        if let Some(reason) = row.reason.as_deref() {
            params.push(p("ReasonMessage", reason));
        }
        let (status, body) = cancel(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            row.case
        );
        // The operation's modelled output is `Unit`, so the envelope's success flag is the
        // whole body.
        assert_eq!(
            member(&body, "return").as_deref(),
            Some("true"),
            "case {:?}: the response reports the cancel succeeded, got {body}",
            row.case
        );

        let after = stored(&engine, row.case, conversion_task_id);
        assert_eq!(
            after.state.as_deref(),
            Some(CONVERSION_TASK_STATE_CANCELLING),
            "case {:?}: straight after the cancel the task is \
             {CONVERSION_TASK_STATE_CANCELLING}, got {after:?}",
            row.case
        );
        assert_eq!(
            after.status_message, row.reason,
            "case {:?}: the task reports the reason the cancel gave, got {after:?}",
            row.case
        );
        let expected_volumes: Vec<Option<String>> =
            vec![Some(CONVERSION_TASK_STATE_CANCELLING.to_string()); row.volumes];
        assert_eq!(
            volume_states(&after),
            expected_volumes,
            "case {:?}: every volume of the import stops where the task did, got {after:?}",
            row.case
        );
        assert!(
            cancellation_owed(&engine, conversion_task_id),
            "case {:?}: the clock owes the cancelled task its {CONVERSION_TASK_STATE_CANCELLED} \
             state",
            row.case
        );
        // What the NEXT caller is shown, over the wire: the task is working through the
        // cancellation, so a second cancel is refused with that state.
        let shown = state_over_the_wire(&router, row.case, conversion_task_id).await;
        assert_eq!(
            shown, CONVERSION_TASK_STATE_CANCELLING,
            "case {:?}: the next caller is shown {CONVERSION_TASK_STATE_CANCELLING}, got \
             {shown:?}",
            row.case
        );
    }

    past_the_transition().await;

    for (row, conversion_task_id) in &ids {
        let shown = state_over_the_wire(&router, row.case, conversion_task_id).await;
        assert_eq!(
            shown, CONVERSION_TASK_STATE_CANCELLED,
            "case {:?}: once the cancellation falls due the task is \
             {CONVERSION_TASK_STATE_CANCELLED}, got {shown:?}",
            row.case
        );
        let settled = stored(&engine, row.case, conversion_task_id);
        assert_eq!(
            settled.status_message, row.reason,
            "case {:?}: the reason survives the move to {CONVERSION_TASK_STATE_CANCELLED}, got \
             {settled:?}",
            row.case
        );
        let expected_volumes: Vec<Option<String>> =
            vec![Some(CONVERSION_TASK_STATE_CANCELLED.to_string()); row.volumes];
        assert_eq!(
            volume_states(&settled),
            expected_volumes,
            "case {:?}: every volume of the import ends where the task did, got {settled:?}",
            row.case
        );
        assert!(
            !cancellation_owed(&engine, conversion_task_id),
            "case {:?}: a cancellation the clock has applied is owed no longer",
            row.case
        );
    }
}

/// Rule 3: a cancel moves the task it names and nothing else — the account's other tasks
/// keep the state they were in, of either kind, and every record stays in the store, since a
/// state is only reportable while the task it belongs to is there.
#[tokio::test]
async fn a_cancel_touches_only_the_task_it_names() {
    let (router, engine) = emulator();
    let cancelled = seed_task(&engine, "the-cancelled-import", Kind::Instance, 1);
    let other_instance = seed_task(&engine, "another-instance-import", Kind::Instance, 2);
    let other_volume = seed_task(&engine, "another-volume-import", Kind::Volume, 0);
    let completed = seed_task_in_state(
        &engine,
        "a-completed-import",
        Kind::Volume,
        Some(CONVERSION_TASK_STATE_COMPLETED),
    );

    let (status, body) = cancel_task(&router, &cancelled).await;
    assert_eq!(
        status, 200,
        "cancelling one of four tasks: expected HTTP 200, got {status} with body {body}"
    );

    // `(case, the task, the state it must be in after the cancel of another)`.
    let expected = [
        (
            "the task the cancel named",
            &cancelled,
            CONVERSION_TASK_STATE_CANCELLING,
        ),
        (
            "another active instance import",
            &other_instance,
            CONVERSION_TASK_STATE_ACTIVE,
        ),
        (
            "an active volume import",
            &other_volume,
            CONVERSION_TASK_STATE_ACTIVE,
        ),
        (
            "an import that had completed",
            &completed,
            CONVERSION_TASK_STATE_COMPLETED,
        ),
    ];
    for (case, conversion_task_id, expected_state) in expected {
        let task = stored(&engine, case, conversion_task_id);
        assert_eq!(
            task.state.as_deref(),
            Some(expected_state),
            "case {case:?}: expected {expected_state}, got {task:?}"
        );
    }

    let mut held = stored_ids(&engine);
    held.sort();
    let mut expected_ids = vec![
        cancelled.clone(),
        other_instance.clone(),
        other_volume.clone(),
        completed.clone(),
    ];
    expected_ids.sort();
    assert_eq!(
        held, expected_ids,
        "a cancel neither removes a record nor re-keys one, so every task stays reportable \
         under the id it was stored under"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a cancel the task's state does not admit is refused and writes nothing
// ---------------------------------------------------------------------------

/// The states a cancel is NOT admitted from: every member of the `ConversionTaskState`
/// catalog ([`CONVERSION_TASK_STATES`], the model's own value list) except the `active` one
/// it is, plus a task carrying no state at all — which is no state of that catalog and so
/// admits no cancel either. Derived from the catalog rather than listed here, so a value the
/// model gains is covered without a new literal.
fn unadmitted_states() -> Vec<(String, Option<&'static str>)> {
    let mut rows: Vec<(String, Option<&'static str>)> = CONVERSION_TASK_STATES
        .iter()
        .filter(|state| **state != CONVERSION_TASK_STATE_ACTIVE)
        .map(|state| (format!("a task in the '{state}' state"), Some(*state)))
        .collect();
    rows.push(("a task carrying no state".to_string(), None));
    rows
}

/// Rule 2: a task that is not converting any more has nothing left to cancel, so the cancel
/// is refused `IncorrectState` naming the state it IS in, and leaves that state alone — over
/// every non-`active` state of the catalog, for both kinds of import.
#[tokio::test]
async fn a_cancel_the_tasks_state_does_not_admit_changes_nothing() {
    let (router, engine) = emulator();
    for kind in [Kind::Instance, Kind::Volume] {
        for (state_case, state) in unadmitted_states() {
            let case = format!("{state_case} ({kind:?} import)");
            let conversion_task_id = seed_task_in_state(&engine, &case, kind, state);
            let (status, body) = cancel_task(&router, &conversion_task_id).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                INCORRECT_STATE,
                "case {case:?}: expected {INCORRECT_STATE}, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                format!(
                    "Conversion task '{conversion_task_id}' is in the '{}' state.",
                    state.unwrap_or_default()
                ),
                "case {case:?}: expected the refusal to name the task and its state, got \
                 {message:?}"
            );
            let task = stored(&engine, &case, &conversion_task_id);
            assert_eq!(
                task.state.as_deref(),
                state,
                "case {case:?}: the refused cancel leaves the state alone, got {task:?}"
            );
            assert!(
                !cancellation_owed(&engine, &conversion_task_id),
                "case {case:?}: a refused cancel owes the clock nothing"
            );
        }
    }
}

/// Rule 2: a reason wider than this service keeps on a task is refused
/// `InvalidParameterValue` before anything is read or written, while one AT that width is
/// accepted — the bound, not one length, is what is asserted.
#[tokio::test]
async fn a_reason_past_the_published_width_is_refused_and_writes_nothing() {
    let (router, engine) = emulator();
    // `(case, how long the reason is, whether the cancel is admitted)`.
    let rows = [
        ("one character past the width", MAX_IMPORT_TASK_CANCEL_REASON_CHARS + 1, false),
        ("far past the width", MAX_IMPORT_TASK_CANCEL_REASON_CHARS * 4, false),
        ("at the width", MAX_IMPORT_TASK_CANCEL_REASON_CHARS, true),
        ("one character short of the width", MAX_IMPORT_TASK_CANCEL_REASON_CHARS - 1, true),
    ];
    for (case, length, admitted) in rows {
        let conversion_task_id = seed_task(&engine, case, Kind::Volume, 0);
        let reason = "r".repeat(length);
        let (status, body) = cancel(
            &router,
            &[
                p("ConversionTaskId", conversion_task_id.as_str()),
                p("ReasonMessage", reason.as_str()),
            ],
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let task = stored(&engine, case, &conversion_task_id);
            assert_eq!(
                task.status_message.as_deref(),
                Some(reason.as_str()),
                "case {case:?}: the accepted reason is what the task reports, got {task:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
            let task = stored(&engine, case, &conversion_task_id);
            assert_eq!(
                task.state.as_deref(),
                Some(CONVERSION_TASK_STATE_ACTIVE),
                "case {case:?}: a refused cancel moves nothing, got {task:?}"
            );
            assert_eq!(
                task.status_message, None,
                "case {case:?}: a refused reason is not stored, got {task:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: `DryRun` answers what the request would have done, and writes nothing
// ---------------------------------------------------------------------------

/// What a dry run of one request is expected to answer.
enum DryRunOutcome {
    /// The request would have succeeded: `DryRunOperation`, HTTP 412.
    WouldHaveWorked,
    /// The request would have been refused, with this code, HTTP 400.
    WouldHaveBeenRefused(&'static str),
}

/// Rule 4: a dry run answers what the request WOULD have done — the refusal it would have
/// been refused with, or `DryRunOperation` for one that would have worked — and writes
/// nothing either way.
#[tokio::test]
async fn a_dry_run_answers_what_the_request_would_have_done_and_writes_nothing() {
    let (router, engine) = emulator();
    let active = seed_task(&engine, "an-active-import", Kind::Instance, 2);
    let completed = seed_task_in_state(
        &engine,
        "a-completed-import",
        Kind::Instance,
        Some(CONVERSION_TASK_STATE_COMPLETED),
    );
    // `(case, the id the dry run names, what it must answer)`.
    let rows: Vec<(&str, String, DryRunOutcome)> = vec![
        (
            "a dry run that would have cancelled an active task",
            active.clone(),
            DryRunOutcome::WouldHaveWorked,
        ),
        (
            "a dry run of a task whose state admits no cancel",
            completed.clone(),
            DryRunOutcome::WouldHaveBeenRefused(INCORRECT_STATE),
        ),
        (
            "a dry run naming an id EC2 could not have issued",
            "import-00000000".to_string(),
            DryRunOutcome::WouldHaveBeenRefused(INVALID_CONVERSION_TASK_ID),
        ),
        (
            "a dry run naming a task that does not exist",
            format!("{IMPORT_VOLUME_CONVERSION_TASK_ID_PREFIX}-0123abcd"),
            DryRunOutcome::WouldHaveBeenRefused(INVALID_CONVERSION_TASK_ID),
        ),
    ];
    for (case, conversion_task_id, outcome) in rows {
        let (status, body) = cancel(
            &router,
            &[
                p("ConversionTaskId", conversion_task_id.as_str()),
                p("DryRun", "true"),
            ],
        )
        .await;
        let (expected_status, expected_code) = match outcome {
            DryRunOutcome::WouldHaveWorked => (412, DRY_RUN_OPERATION),
            DryRunOutcome::WouldHaveBeenRefused(code) => (400, code),
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Nothing the dry runs named moved, and the clock owes neither task anything.
    for (case, conversion_task_id, expected_state) in [
        (
            "the active task a dry run would have cancelled",
            &active,
            CONVERSION_TASK_STATE_ACTIVE,
        ),
        (
            "the completed task a dry run named",
            &completed,
            CONVERSION_TASK_STATE_COMPLETED,
        ),
    ] {
        let task = stored(&engine, case, conversion_task_id);
        assert_eq!(
            task.state.as_deref(),
            Some(expected_state),
            "case {case:?}: a dry run writes nothing, got {task:?}"
        );
        assert!(
            !cancellation_owed(&engine, conversion_task_id),
            "case {case:?}: a dry run owes the clock nothing"
        );
    }

    // And the cancel the dry run reported as admissible really is: the same request without
    // the flag is served, which is what makes `DryRunOperation` an answer about THIS request.
    let (status, body) = cancel_task(&router, &active).await;
    assert_eq!(
        status, 200,
        "the dry run's subject cancels for real: expected HTTP 200, got {status} with body \
         {body}"
    );
    let task = stored(&engine, "the dry run's subject", &active);
    assert_eq!(
        task.state.as_deref(),
        Some(CONVERSION_TASK_STATE_CANCELLING),
        "the dry run's subject is {CONVERSION_TASK_STATE_CANCELLING} once cancelled for real, \
         got {task:?}"
    );
}

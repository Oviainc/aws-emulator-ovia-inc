// SPDX-License-Identifier: CC0-1.0

//! `UpdateJobPriority` over the stored S3 Batch Operations job, driven end to
//! end over HTTP.
//!
//! The rule under test: a priority update REPLACES the named job's priority
//! with the value the request carries, answers that job id with the priority
//! now stored, and changes nothing else about the job — so every later
//! `DescribeJob` of that job reports the new priority and the same job it
//! reported before; a job id the named account owns no job for is refused
//! rather than answered, and a refused update leaves the stored priority
//! alone.
//!
//! Every row below differs along something that rule generalises over: the
//! priority asked for (including each published bound of `JobPriority` and one
//! past each), whether it is above, below or equal to the priority the job
//! already carries, how many updates the job has had, which shape of create
//! recorded the job, and the account the job belongs to.
//!
//! The requests go through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes
//! it per the Smithy model (the priority rides in the `priority` query
//! parameter), runs the handler and serializes the restXml response. The
//! router, the create and describe helpers and the job request shapes are the
//! batch job lifecycle suite's, reused rather than copied.

use std::time::Duration;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_PREPARATION, JOB_STATUS_READY,
};

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT,
};
use crate::batch_job_lifecycle::{
    create_job, describe_job, JobCase, ACTION_LAMBDA_INVOKE, ACTION_PUT_OBJECT_COPY,
    ACTION_PUT_OBJECT_TAGGING, GENERATED_MANIFEST, MANIFEST_CSV, MANIFEST_INVENTORY,
};

/// The priority every job below is created with, so a row can ask for a value
/// above it, below it or equal to it. Within the modelled `JobPriority` range
/// (0..2147483647, from `UpdateJobPriorityRequest$Priority`).
const CREATED_PRIORITY: i64 = 10;

/// The bounds the model publishes for `JobPriority`
/// (`smithy_query(query="operation",operation="UpdateJobPriority")`: range
/// 0..2147483647).
const PRIORITY_MIN: i64 = 0;
const PRIORITY_MAX: i64 = 2_147_483_647;

/// Ask for `priority` on `job_id`, exactly as the model binds it: a POST to the
/// job's `priority` sub-resource with the value in the `priority` query
/// parameter and no body.
///
/// `priority` is rendered verbatim, so a value outside the modelled range can
/// be a row of its own.
async fn update_job_priority(
    emulator: &Emulator,
    account_id: &str,
    job_id: &str,
    priority: &str,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v20180820/jobs/{job_id}/priority?priority={priority}"),
            account_id,
            "",
        )
        .await
}

/// Create one job at [`CREATED_PRIORITY`] and answer the job id the service
/// assigned, with the clock's own `New` -> `Ready` transition already made due,
/// so a status this suite reads afterwards is not one the clock was about to
/// change under it.
async fn create_prepared_job(
    emulator: &Emulator,
    account_id: &str,
    case: &JobCase,
    token: &str,
) -> String {
    let created = create_job(emulator, account_id, case, token).await;
    assert_eq!(
        created.status, 200,
        "case {token:?}: expected CreateJob to answer 200, got {} with body {}",
        created.status, created.body
    );
    let job_id = created
        .text("JobId")
        .unwrap_or_else(|| panic!("case {token:?}: CreateJob answers a JobId: {}", created.body));
    tokio::time::sleep(JOB_PREPARATION + Duration::from_millis(50)).await;
    job_id
}

/// The priority `DescribeJob` reports for `job_id`, read through the implemented
/// reader rather than from the update's own answer.
async fn described_priority(emulator: &Emulator, account_id: &str, job_id: &str) -> String {
    let described = describe_job(emulator, account_id, job_id).await;
    assert_eq!(
        described.status, 200,
        "expected DescribeJob of {job_id:?} to answer 200, got {} with body {}",
        described.status, described.body
    );
    described
        .text("Priority")
        .unwrap_or_else(|| panic!("DescribeJob reports a Priority: {}", described.body))
}

/// The create shapes the rows below are run over: the priority update is a
/// member of the job whatever action the job runs and wherever its object list
/// comes from.
fn job_shapes() -> Vec<JobCase> {
    vec![
        JobCase::new(
            "copy from a CSV manifest",
            ACTION_PUT_OBJECT_COPY,
            MANIFEST_CSV,
        ),
        JobCase::new(
            "tagging from an inventory manifest",
            ACTION_PUT_OBJECT_TAGGING,
            MANIFEST_INVENTORY,
        ),
        JobCase::new(
            "lambda invoke from a generated manifest",
            ACTION_LAMBDA_INVOKE,
            GENERATED_MANIFEST,
        ),
    ]
}

#[tokio::test]
async fn an_update_replaces_the_stored_priority_and_changes_nothing_else_about_the_job() {
    let emulator = Emulator::start();

    // Each row is a priority the update asks for, spanning the class: each
    // published bound of the modelled range, a value above and a value below
    // the one the job carries, and the value it already carries.
    let rows: [(&str, i64); 6] = [
        ("the minimum the modelled range admits", PRIORITY_MIN),
        ("one above the minimum", PRIORITY_MIN + 1),
        ("below the priority the job carries", CREATED_PRIORITY - 1),
        ("the priority the job already carries", CREATED_PRIORITY),
        ("one below the maximum", PRIORITY_MAX - 1),
        ("the maximum the modelled range admits", PRIORITY_MAX),
    ];

    for (shape_row, shape) in job_shapes().into_iter().enumerate() {
        for (row, (case, priority)) in rows.iter().enumerate() {
            let token = format!("token-{shape_row}-{row}");
            let job_id = create_prepared_job(&emulator, ACCOUNT, &shape, &token).await;

            // The job exactly as it reads before the update, so what the
            // update changed can be compared member by member.
            let before = describe_job(&emulator, ACCOUNT, &job_id).await;
            assert_eq!(
                before.text("Priority").as_deref(),
                Some(CREATED_PRIORITY.to_string().as_str()),
                "case {case:?}: expected the created job to carry the priority the create \
                 submitted, got body {}",
                before.body
            );

            let updated = update_job_priority(&emulator, ACCOUNT, &job_id, &priority.to_string())
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected UpdateJobPriority to answer 200, got {} with body {}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.text("JobId").as_deref(),
                Some(job_id.as_str()),
                "case {case:?}: expected the answer to name job {job_id:?}, got body {}",
                updated.body
            );
            assert_eq!(
                updated.text("Priority").as_deref(),
                Some(priority.to_string().as_str()),
                "case {case:?}: expected the answer to carry the new priority {priority}, got \
                 body {}",
                updated.body
            );

            // What the reader reports is what the update stored.
            let after = describe_job(&emulator, ACCOUNT, &job_id).await;
            assert_eq!(
                after.text("Priority").as_deref(),
                Some(priority.to_string().as_str()),
                "case {case:?}: expected DescribeJob to report priority {priority}, got body {}",
                after.body
            );

            // And the rest of the job is the job the create recorded: the
            // described document differs from the one before the update in the
            // priority alone — the status included, which this operation does
            // not move.
            let expected_after = before.body.replace(
                &format!("<Priority>{CREATED_PRIORITY}</Priority>"),
                &format!("<Priority>{priority}</Priority>"),
            );
            assert_eq!(
                after.body, expected_after,
                "case {case:?}: expected the update to change the priority alone, got {} from {}",
                after.body, before.body
            );
            assert_eq!(
                after.text("Status").as_deref(),
                Some(JOB_STATUS_READY),
                "case {case:?}: expected the prepared job to stay {JOB_STATUS_READY}, got body {}",
                after.body
            );
        }
    }
}

#[tokio::test]
async fn the_last_update_wins_and_each_job_keeps_its_own_priority() {
    let emulator = Emulator::start();

    let shape = JobCase::new("copy from a CSV manifest", ACTION_PUT_OBJECT_COPY, MANIFEST_CSV);
    let first = create_prepared_job(&emulator, ACCOUNT, &shape, "sequence-first").await;
    let second = create_prepared_job(&emulator, ACCOUNT, &shape, "sequence-second").await;

    // A run of updates on one job: each replaces the last, rising and then
    // falling, so neither "the highest wins" nor "the first wins" passes.
    for priority in ["5", "900", "41", "0", "2147483647", "7"] {
        let updated = update_job_priority(&emulator, ACCOUNT, &first, priority).await;
        assert_eq!(
            updated.status, 200,
            "case {priority:?}: expected UpdateJobPriority to answer 200, got {} with body {}",
            updated.status, updated.body
        );
        let described = described_priority(&emulator, ACCOUNT, &first).await;
        assert_eq!(
            described, priority,
            "case {priority:?}: expected the last update to win, got {described}"
        );
    }

    // The job nobody updated still carries what its create submitted.
    let described = described_priority(&emulator, ACCOUNT, &second).await;
    assert_eq!(
        described,
        CREATED_PRIORITY.to_string(),
        "expected the job nobody updated to keep priority {CREATED_PRIORITY}, got {described}"
    );

    // And an update addressed in another account is refused before the job is
    // looked up: the same id named for an account the caller does not hold is
    // not a bad request about the job but an authorization failure.
    let refused = update_job_priority(&emulator, OTHER_ACCOUNT, &first, "3").await;
    assert_foreign_account_refused("UpdateJobPriority for another account", OTHER_ACCOUNT, &refused);
    let described = described_priority(&emulator, ACCOUNT, &first).await;
    assert_eq!(
        described, "7",
        "expected the refused update to leave the stored priority alone, got {described}"
    );
}

#[tokio::test]
async fn an_update_that_names_no_job_or_no_admitted_priority_is_refused_and_stores_nothing() {
    let emulator = Emulator::start();

    let shape = JobCase::new(
        "tagging from an inventory manifest",
        ACTION_PUT_OBJECT_TAGGING,
        MANIFEST_INVENTORY,
    );
    let job_id = create_prepared_job(&emulator, ACCOUNT, &shape, "refusals").await;

    // A well-formed `JobId` (5..36 of ^[a-zA-Z0-9\-\_]+$) that names no job.
    let unknown_ids = [
        (
            "a well-formed id no job has",
            "00000000-0000-0000-0000-000000".to_string(),
        ),
        ("the shortest id the model admits", "aaaaa".to_string()),
        (
            "an id with the underscores the model admits",
            "job_id_000".to_string(),
        ),
        ("the existing id with one character changed", {
            let mut characters: Vec<char> = job_id.chars().collect();
            let last = characters.len() - 1;
            characters[last] = if characters[last] == 'z' { 'y' } else { 'z' };
            characters.into_iter().collect()
        }),
    ];
    for (case, unknown) in unknown_ids {
        let refused = update_job_priority(&emulator, ACCOUNT, &unknown, "5").await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 for {unknown:?}, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "InvalidRequest",
            "case {case:?}: expected InvalidRequest for {unknown:?}, got body {}",
            refused.body
        );
    }

    // One past each published bound of `JobPriority` is refused by the boundary
    // validator, before the handler runs, so only the status is this suite's to
    // pin — and the stored priority must survive it.
    for (case, priority) in [
        ("one below the minimum priority", "-1"),
        ("one above the maximum priority", "2147483648"),
    ] {
        let refused = update_job_priority(&emulator, ACCOUNT, &job_id, priority).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a priority outside the modelled range to be refused 400, \
             got {} with body {}",
            refused.status, refused.body
        );
    }

    let described = described_priority(&emulator, ACCOUNT, &job_id).await;
    assert_eq!(
        described,
        CREATED_PRIORITY.to_string(),
        "expected every refused update to leave the stored priority at {CREATED_PRIORITY}, \
         got {described}"
    );
}

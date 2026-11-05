// SPDX-License-Identifier: CC0-1.0

//! `UpdateJobStatus` over the stored S3 Batch Operations job, driven end to end
//! over HTTP.
//!
//! The rule under test: a status update moves the named job to the status the
//! request asks for when the status the clock has the job in admits that move —
//! a job waiting for confirmation is confirmed into `Ready`, and a job that has
//! neither ended nor begun ending is cancelled into the terminal `Cancelled`
//! with that instant as its termination date — records the reason the request
//! carried as the job's status update reason, and answers the job id with the
//! status and reason now stored, so every later read of that job reports them.
//! A move the job's status does not admit is refused with the code this
//! service's job surface answers a well-formed request it cannot carry out with
//! and leaves the job exactly as it was, and a job id the named account owns no
//! job for is refused rather than answered.
//!
//! Every row below differs along something that rule generalises over: which
//! status the job is in when the update arrives (still being prepared, prepared,
//! awaiting confirmation, confirmed, already cancelled), which status is asked
//! for, whether a reason is carried and of what length (including each published
//! bound of `JobStatusUpdateReason` and one past the maximum) and which
//! characters it carries, how many updates the job has had, which shape of
//! create recorded the job, and the account the job belongs to.
//!
//! The requests go through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it
//! per the Smithy model (the requested status and the reason ride in the
//! `requestedJobStatus` and `statusUpdateReason` query parameters), runs the
//! handler and serializes the restXml response. The router, the create, describe
//! and list helpers, the query escaping and the job request shapes are the
//! sibling suites', reused rather than copied.

use std::time::Duration;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_PREPARATION, JOB_STATUS_CANCELLED, JOB_STATUS_NEW, JOB_STATUS_READY, JOB_STATUS_SUSPENDED,
    SUSPENDED_AWAITING_CONFIRMATION,
};

use crate::access_grants_data_plane::query_escape;
use crate::access_point_lifecycle::{
    assert_foreign_account_refused, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT,
};
use crate::batch_job_lifecycle::{
    create_job, describe_job, JobCase, ACTION_COMPUTE_CHECKSUM, ACTION_LAMBDA_INVOKE,
    ACTION_PUT_OBJECT_COPY, ACTION_PUT_OBJECT_TAGGING, GENERATED_MANIFEST, MANIFEST_CSV,
    MANIFEST_INVENTORY,
};
use crate::batch_job_list::list_jobs;

/// The published bounds of `JobStatusUpdateReason`
/// (`smithy_query(query="operation",operation="UpdateJobStatus")`: len 1..256).
const REASON_MIN_LENGTH: usize = 1;
const REASON_MAX_LENGTH: usize = 256;

/// The `<Code>` a move the job's status does not admit is refused with: the
/// S3-style envelope the whole job surface answers a well-formed request it
/// cannot carry out with, which is what this service was recorded answering
/// rather than the modelled `JobStatusException`.
const JOB_STATUS_REFUSAL: &str = "InvalidRequest";

/// The members a status update is allowed to change, which is what the rows
/// below leave out of the described document when they compare the rest of the
/// job against what the create recorded.
const UPDATED_MEMBERS: [&str; 3] = ["Status", "StatusUpdateReason", "TerminationDate"];

/// A reason of exactly `length` characters, for the rows that drive each
/// published bound of `JobStatusUpdateReason`.
fn reason_of_length(length: usize) -> String {
    "reason-".chars().cycle().take(length).collect()
}

/// `body` with every `<element>…</element>` of `elements` removed, so two
/// documents can be compared on the members neither call was allowed to touch.
fn without_elements(body: &str, elements: &[&str]) -> String {
    let mut stripped = body.to_string();
    for element in elements {
        let open = format!("<{element}>");
        let close = format!("</{element}>");
        while let Some(start) = stripped.find(&open) {
            let Some(end) = stripped[start..].find(&close) else {
                panic!("<{element}> with no closing tag in {body}");
            };
            stripped.replace_range(start..start + end + close.len(), "");
        }
    }
    stripped
}

/// Ask for `requested_job_status` on `job_id`, exactly as the model binds it: a
/// POST to the job's `status` sub-resource with the requested status and the
/// optional reason in the query string and no body.
///
/// `requested_job_status` is rendered verbatim, so a value the modelled
/// `RequestedJobStatus` enum does not carry can be a row of its own; the reason
/// is percent-encoded, so one carrying spaces and punctuation reaches the
/// handler as the caller wrote it.
async fn update_job_status(
    emulator: &Emulator,
    account_id: &str,
    job_id: &str,
    requested_job_status: &str,
    reason: Option<&str>,
) -> Reply {
    let mut uri =
        format!("/v20180820/jobs/{job_id}/status?requestedJobStatus={requested_job_status}");
    if let Some(reason) = reason {
        uri.push_str(&format!("&statusUpdateReason={}", query_escape(reason)));
    }
    emulator.call("POST", &uri, account_id, "").await
}

/// Create one job from `case` and answer the job id the service assigned.
async fn create_one(emulator: &Emulator, account_id: &str, case: &JobCase, token: &str) -> String {
    let created = create_job(emulator, account_id, case, token).await;
    assert_eq!(
        created.status, 200,
        "case {token:?}: expected CreateJob to answer 200, got {} with body {}",
        created.status, created.body
    );
    created
        .text("JobId")
        .unwrap_or_else(|| panic!("case {token:?}: CreateJob answers a JobId: {}", created.body))
}

/// The job as `DescribeJob` reports it — the reader every assertion below reads
/// the stored status back through.
async fn described(emulator: &Emulator, account_id: &str, job_id: &str) -> Reply {
    let reply = describe_job(emulator, account_id, job_id).await;
    assert_eq!(
        reply.status, 200,
        "expected DescribeJob of {job_id:?} to answer 200, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

/// Wait out the clock's own `New` -> `Ready` transition, so a status read after
/// this is not one the clock was about to change under the assertion.
async fn wait_for_preparation() {
    tokio::time::sleep(JOB_PREPARATION + Duration::from_millis(50)).await;
}

/// The create shapes the rows below are run over: a status update is a member of
/// the job whatever action the job runs and wherever its object list comes from.
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

/// The reasons the rows below carry: none at all, each published bound of
/// `JobStatusUpdateReason`, and one carrying the characters a query value has to
/// escape.
fn reasons() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("no reason at all", None),
        (
            "the shortest reason the model admits",
            Some(reason_of_length(REASON_MIN_LENGTH)),
        ),
        (
            "a sentence carrying characters a query value escapes",
            Some("stopped: the manifest named the wrong bucket (50% done) & re-queued".to_string()),
        ),
        (
            "the longest reason the model admits",
            Some(reason_of_length(REASON_MAX_LENGTH)),
        ),
    ]
}

#[tokio::test]
async fn a_confirmation_moves_a_job_awaiting_it_to_ready_and_records_the_reason_given() {
    let emulator = Emulator::start();

    for (shape_row, shape) in job_shapes().into_iter().enumerate() {
        let shape = shape.awaiting_confirmation();
        for (row, (case, reason)) in reasons().into_iter().enumerate() {
            let token = format!("confirm-{shape_row}-{row}");
            let job_id = create_one(&emulator, ACCOUNT, &shape, &token).await;

            // A create that requires confirmation leaves the job waiting for
            // it, which is the one status a confirmation has anything to do.
            let before = described(&emulator, ACCOUNT, &job_id).await;
            assert_eq!(
                before.text("Status").as_deref(),
                Some(JOB_STATUS_SUSPENDED),
                "case {case:?}: expected a job created with confirmation required to be \
                 {JOB_STATUS_SUSPENDED}, got body {}",
                before.body
            );

            let confirmed = update_job_status(
                &emulator,
                ACCOUNT,
                &job_id,
                JOB_STATUS_READY,
                reason.as_deref(),
            )
            .await;
            assert_eq!(
                confirmed.status, 200,
                "case {case:?}: expected UpdateJobStatus to answer 200, got {} with body {}",
                confirmed.status, confirmed.body
            );
            assert_eq!(
                confirmed.text("JobId").as_deref(),
                Some(job_id.as_str()),
                "case {case:?}: expected the answer to name job {job_id:?}, got body {}",
                confirmed.body
            );
            assert_eq!(
                confirmed.text("Status").as_deref(),
                Some(JOB_STATUS_READY),
                "case {case:?}: expected the confirmed job to answer {JOB_STATUS_READY}, got \
                 body {}",
                confirmed.body
            );
            assert_eq!(
                confirmed.text("StatusUpdateReason"),
                reason,
                "case {case:?}: expected the answer to carry the reason the request gave, got \
                 body {}",
                confirmed.body
            );

            // What the reader reports is what the update stored.
            let after = described(&emulator, ACCOUNT, &job_id).await;
            assert_eq!(
                after.text("Status").as_deref(),
                Some(JOB_STATUS_READY),
                "case {case:?}: expected DescribeJob to report {JOB_STATUS_READY}, got body {}",
                after.body
            );
            assert_eq!(
                after.text("StatusUpdateReason"),
                reason,
                "case {case:?}: expected DescribeJob to report the stored reason, got body {}",
                after.body
            );
            // A confirmed job has not ended, and what it WAS suspended at and
            // for is still part of the job the create recorded.
            assert!(
                after.text("TerminationDate").is_none(),
                "case {case:?}: expected a confirmed job to carry no TerminationDate, got body {}",
                after.body
            );
            assert_eq!(
                after.text("SuspendedCause").as_deref(),
                Some(SUSPENDED_AWAITING_CONFIRMATION),
                "case {case:?}: expected the record of the suspension to stand, got body {}",
                after.body
            );
            assert_eq!(
                after.text("SuspendedDate"),
                before.text("SuspendedDate"),
                "case {case:?}: expected the suspension's timestamp to stand, got body {}",
                after.body
            );
            // And nothing the update is not about moved: on every other member
            // the described job is the job the create recorded.
            assert_eq!(
                without_elements(&after.body, &UPDATED_MEMBERS),
                without_elements(&before.body, &UPDATED_MEMBERS),
                "case {case:?}: expected the update to move the status and the reason alone, \
                 got {} from {}",
                after.body,
                before.body
            );
        }
    }
}

/// One row of the cancellation table: a status a job can be cancelled from, and
/// how the suite gets a job into it.
struct CancelFrom {
    case: &'static str,
    /// Whether the create requires confirmation, which is what leaves the job
    /// waiting for one.
    confirmation_required: bool,
    /// Whether an earlier `UpdateJobStatus` confirms it.
    confirm_first: bool,
    /// Whether the suite waits the job's preparation out before cancelling.
    prepared: bool,
    /// The statuses the job may be in when the cancellation arrives: more than
    /// one where the clock, not this suite, decides which.
    admits: &'static [&'static str],
}

fn cancel_rows() -> Vec<CancelFrom> {
    vec![
        CancelFrom {
            case: "a job still being prepared",
            confirmation_required: false,
            confirm_first: false,
            prepared: false,
            // Cancelled before its preparation falls due, the job is `New`; if
            // the due instant passed first, the clock has already made it
            // `Ready`. Both admit a cancellation, so the row asserts the pair
            // rather than racing the clock.
            admits: &[JOB_STATUS_NEW, JOB_STATUS_READY],
        },
        CancelFrom {
            case: "a prepared job",
            confirmation_required: false,
            confirm_first: false,
            prepared: true,
            admits: &[JOB_STATUS_READY],
        },
        CancelFrom {
            case: "a job awaiting confirmation",
            confirmation_required: true,
            confirm_first: false,
            prepared: true,
            admits: &[JOB_STATUS_SUSPENDED],
        },
        CancelFrom {
            case: "a job confirmed by an earlier update",
            confirmation_required: true,
            confirm_first: true,
            prepared: true,
            admits: &[JOB_STATUS_READY],
        },
    ]
}

#[tokio::test]
async fn a_cancellation_terminates_a_job_from_every_status_that_admits_one() {
    let emulator = Emulator::start();
    let all_reasons = reasons();

    for (shape_row, shape) in job_shapes().into_iter().enumerate() {
        for (row, from) in cancel_rows().into_iter().enumerate() {
            // The reasons are cycled over the status rows rather than crossed
            // with them: the reason is a member of the request, not of the
            // status the job is in.
            let (reason_case, reason) = all_reasons[row % all_reasons.len()].clone();
            let case = format!("{} with {reason_case}", from.case);
            let token = format!("cancel-{shape_row}-{row}");
            let shape = if from.confirmation_required {
                shape.clone().awaiting_confirmation()
            } else {
                shape.clone()
            };
            let job_id = create_one(&emulator, ACCOUNT, &shape, &token).await;

            if from.prepared {
                wait_for_preparation().await;
            }
            if from.confirm_first {
                let confirmed =
                    update_job_status(&emulator, ACCOUNT, &job_id, JOB_STATUS_READY, None).await;
                assert_eq!(
                    confirmed.status, 200,
                    "case {case:?}: expected the confirmation to answer 200, got {} with body {}",
                    confirmed.status, confirmed.body
                );
            }

            let before = described(&emulator, ACCOUNT, &job_id).await;
            let before_status = before.text("Status").unwrap_or_else(|| {
                panic!(
                    "case {case:?}: DescribeJob reports a Status: {}",
                    before.body
                )
            });
            assert!(
                from.admits.contains(&before_status.as_str()),
                "case {case:?}: expected the job to be one of {:?} before the cancellation, got \
                 {before_status:?} in body {}",
                from.admits,
                before.body
            );
            assert!(
                before.text("TerminationDate").is_none(),
                "case {case:?}: expected a job that has not ended to carry no TerminationDate, \
                 got body {}",
                before.body
            );

            let cancelled = update_job_status(
                &emulator,
                ACCOUNT,
                &job_id,
                JOB_STATUS_CANCELLED,
                reason.as_deref(),
            )
            .await;
            assert_eq!(
                cancelled.status, 200,
                "case {case:?}: expected UpdateJobStatus to answer 200, got {} with body {}",
                cancelled.status, cancelled.body
            );
            assert_eq!(
                cancelled.text("JobId").as_deref(),
                Some(job_id.as_str()),
                "case {case:?}: expected the answer to name job {job_id:?}, got body {}",
                cancelled.body
            );
            assert_eq!(
                cancelled.text("Status").as_deref(),
                Some(JOB_STATUS_CANCELLED),
                "case {case:?}: expected the cancelled job to answer {JOB_STATUS_CANCELLED}, \
                 got body {}",
                cancelled.body
            );
            assert_eq!(
                cancelled.text("StatusUpdateReason"),
                reason,
                "case {case:?}: expected the answer to carry the reason the request gave, got \
                 body {}",
                cancelled.body
            );

            // What the readers report is what the cancellation stored: the
            // terminal status, the reason, and a termination date the job did
            // not carry before.
            let after = described(&emulator, ACCOUNT, &job_id).await;
            assert_eq!(
                after.text("Status").as_deref(),
                Some(JOB_STATUS_CANCELLED),
                "case {case:?}: expected DescribeJob to report {JOB_STATUS_CANCELLED}, got \
                 body {}",
                after.body
            );
            assert_eq!(
                after.text("StatusUpdateReason"),
                reason,
                "case {case:?}: expected DescribeJob to report the stored reason, got body {}",
                after.body
            );
            assert!(
                after.text("TerminationDate").is_some(),
                "case {case:?}: expected a cancelled job to carry a TerminationDate, got body {}",
                after.body
            );
            assert_eq!(
                without_elements(&after.body, &UPDATED_MEMBERS),
                without_elements(&before.body, &UPDATED_MEMBERS),
                "case {case:?}: expected the cancellation to move the status, the reason and \
                 the termination date alone, got {} from {}",
                after.body,
                before.body
            );

            // The listing reads the same record: a cancelled job is reported,
            // and reported under the status it now has.
            let listed = list_jobs(
                &emulator,
                ACCOUNT,
                &format!("jobStatuses={JOB_STATUS_CANCELLED}"),
            )
            .await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected ListJobs to answer 200, got {} with body {}",
                listed.status, listed.body
            );
            assert!(
                listed.texts("JobId").contains(&job_id),
                "case {case:?}: expected the {JOB_STATUS_CANCELLED} listing to report \
                 {job_id:?}, got body {}",
                listed.body
            );
        }
    }
}

#[tokio::test]
async fn a_move_the_jobs_status_does_not_admit_is_refused_and_changes_nothing() {
    let emulator = Emulator::start();

    let shape = JobCase::new(
        "checksum from a CSV manifest",
        ACTION_COMPUTE_CHECKSUM,
        MANIFEST_CSV,
    );

    // A prepared job awaits no confirmation, so there is nothing to confirm; a
    // cancelled job has ended, so neither move is left to make.
    let prepared = create_one(&emulator, ACCOUNT, &shape, "refuse-prepared").await;
    let ended = create_one(&emulator, ACCOUNT, &shape, "refuse-ended").await;
    wait_for_preparation().await;
    let cancelled = update_job_status(
        &emulator,
        ACCOUNT,
        &ended,
        JOB_STATUS_CANCELLED,
        Some("cancelled before the rows below"),
    )
    .await;
    assert_eq!(
        cancelled.status, 200,
        "expected the first cancellation to answer 200, got {} with body {}",
        cancelled.status, cancelled.body
    );

    // Each row is a move the job's status does not admit: the status it would be
    // moved to, and the status it stays in instead.
    let rows: [(&str, &str, &str, &str, Option<&str>); 5] = [
        (
            "confirming a job that awaits no confirmation",
            &prepared,
            JOB_STATUS_READY,
            JOB_STATUS_READY,
            None,
        ),
        (
            "confirming a job that awaits no confirmation, with a reason",
            &prepared,
            JOB_STATUS_READY,
            JOB_STATUS_READY,
            Some("a reason a refused update must not record"),
        ),
        (
            "confirming a job that has ended",
            &ended,
            JOB_STATUS_READY,
            JOB_STATUS_CANCELLED,
            None,
        ),
        (
            "cancelling a job that has already been cancelled",
            &ended,
            JOB_STATUS_CANCELLED,
            JOB_STATUS_CANCELLED,
            None,
        ),
        (
            "cancelling a job that has already been cancelled, with a reason",
            &ended,
            JOB_STATUS_CANCELLED,
            JOB_STATUS_CANCELLED,
            Some("a second reason a refused update must not record"),
        ),
    ];

    for (case, job_id, requested, stays, reason) in rows {
        let before = described(&emulator, ACCOUNT, job_id).await;
        let refused = update_job_status(&emulator, ACCOUNT, job_id, requested, reason).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a move the status does not admit to be refused 400, got \
             {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            JOB_STATUS_REFUSAL,
            "case {case:?}: expected {JOB_STATUS_REFUSAL}, got body {}",
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(format!("Job status cannot be updated from {stays} to {requested}").as_str()),
            "case {case:?}: expected the refusal to name the status the job is in and the one \
             asked for, got body {}",
            refused.body
        );

        // A refused update recorded nothing: not the status, and not the reason
        // it carried.
        let after = described(&emulator, ACCOUNT, job_id).await;
        assert_eq!(
            after.text("Status").as_deref(),
            Some(stays),
            "case {case:?}: expected the job to stay {stays}, got body {}",
            after.body
        );
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected a refused update to leave the job as it was, got {} from {}",
            after.body, before.body
        );
    }
}

#[tokio::test]
async fn an_update_that_names_no_job_or_no_admitted_value_is_refused_and_stores_nothing() {
    let emulator = Emulator::start();

    let shape = JobCase::new(
        "tagging from an inventory manifest",
        ACTION_PUT_OBJECT_TAGGING,
        MANIFEST_INVENTORY,
    );
    let job_id = create_one(&emulator, ACCOUNT, &shape, "refusals").await;
    wait_for_preparation().await;

    // A well-formed `JobId` (5..36 of ^[a-zA-Z0-9\-\_]+$) that names no job of
    // this account is refused through the one job-id refusal the whole job
    // surface shares.
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
        for requested in [JOB_STATUS_READY, JOB_STATUS_CANCELLED] {
            let refused = update_job_status(&emulator, ACCOUNT, &unknown, requested, None).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} to {requested}: expected 400 for {unknown:?}, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.code(),
                "InvalidRequest",
                "case {case:?} to {requested}: expected InvalidRequest for {unknown:?}, got \
                 body {}",
                refused.body
            );
        }
    }

    // An update addressed in another account is refused before the job is
    // looked up, so the same id named for an account the caller does not hold
    // is an authorization failure rather than a bad request about the job.
    let refused =
        update_job_status(&emulator, OTHER_ACCOUNT, &job_id, JOB_STATUS_CANCELLED, None).await;
    assert_foreign_account_refused("UpdateJobStatus for another account", OTHER_ACCOUNT, &refused);

    // A value the modelled enum and lengths do not admit is refused by the
    // boundary validator, before the handler runs, so only the status is this
    // suite's to pin — and the stored job must survive each one.
    let outside_the_model = [
        (
            "a status the RequestedJobStatus enum does not carry",
            "Paused".to_string(),
            None,
        ),
        (
            "a requested status in the wrong case",
            "cancelled".to_string(),
            None,
        ),
        (
            "one character past the longest reason the model admits",
            JOB_STATUS_CANCELLED.to_string(),
            Some(reason_of_length(REASON_MAX_LENGTH + 1)),
        ),
    ];
    for (case, requested, reason) in outside_the_model {
        let refused =
            update_job_status(&emulator, ACCOUNT, &job_id, &requested, reason.as_deref()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a value outside the model to be refused 400, got {} with \
             body {}",
            refused.status, refused.body
        );
    }

    let after = described(&emulator, ACCOUNT, &job_id).await;
    assert_eq!(
        after.text("Status").as_deref(),
        Some(JOB_STATUS_READY),
        "expected every refused update to leave the job {JOB_STATUS_READY}, got body {}",
        after.body
    );
    assert!(
        after.text("StatusUpdateReason").is_none(),
        "expected every refused update to record no reason, got body {}",
        after.body
    );
    assert!(
        after.text("TerminationDate").is_none(),
        "expected every refused update to leave the job unterminated, got body {}",
        after.body
    );
}

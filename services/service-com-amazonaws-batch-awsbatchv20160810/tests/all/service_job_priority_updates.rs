// SPDX-License-Identifier: CC0-1.0

//! `UpdateServiceJob`, end to end over the wire.
//!
//! The RULES under test ("Updates the priority of a specified service job in an Batch job
//! queue"):
//!
//! * An `UpdateServiceJob` the service accepts writes the request's
//!   `schedulingPriority` onto the stored service job and changes NOTHING else about it,
//!   so the next `DescribeServiceJob` differs in that one member and the answer the
//!   update itself returns identifies the job it wrote (`jobId`, `jobArn`, `jobName` of
//!   the stored record). The last accepted update is the one that sticks.
//! * `schedulingPriority` is bounded by its own doc — "The minimum supported value is 0
//!   and the maximum supported value is 9999" — and a value outside that range answers
//!   the modelled `ClientException` (HTTP 400) and persists nothing.
//! * A `jobId` the service holds no service job for answers the modelled
//!   `ClientException` (HTTP 400) and persists nothing.
//! * A service job in a terminal `ServiceJobStatus` — one it never leaves, so nothing is
//!   left to schedule for it — answers the modelled `ClientException` (HTTP 400) and is
//!   left exactly as it is.
//!
//! Each test asserts its rule over a SET of inputs: every non-terminal `ServiceJobStatus`
//! the model declares, both published bounds of the priority range and values past each
//! of them, records carrying every optional member beside ones carrying only the required
//! ones, the forms an unknown id can take, and every terminal status (plus a job an
//! earlier `TerminateServiceJob` ended).
//!
//! A seeded record is how a row reaches a status `SubmitServiceJob` cannot mint; it is
//! written through the same state API the handlers use, into the same store they read
//! (`tests/all/service_job_termination.rs::seed`). One test drives the real
//! `SubmitServiceJob` path instead, so the update is also shown against a record this
//! emulator minted itself.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::cancel_job::flip_last_hex_digit;
use crate::job_lifecycle::{error_code, error_message, Batch, REGION};
use crate::service_job_submission::{environment_and_queue, submitted};
use crate::service_job_termination::{
    describe, described, live_statuses, seed, terminate, Filled, TERMINAL_STATUSES,
};

/// `schedulingPriority` values the service accepts, from the member's own doc
/// (`smithy_query(query="doc", shape="UpdateServiceJobRequest",
/// member="schedulingPriority")`): "The minimum supported value is 0 and the maximum
/// supported value is 9999." Both published bounds are rows, with ordinary values between
/// them.
const ACCEPTED_PRIORITIES: &[i64] = &[0, 1, 7, 500, 9998, 9999];

/// `schedulingPriority` values outside the documented range: one past each published
/// bound, and the extremes the `Integer` wire type still carries.
const REJECTED_PRIORITIES: &[i64] = &[-1, 10_000, -10_000, i32::MIN as i64, i32::MAX as i64];

/// The only `ServiceJobType` the model declares
/// (`smithy_query(query="shape", shape="ServiceJobType")`: SAGEMAKER_TRAINING).
const SERVICE_JOB_TYPE: &str = "SAGEMAKER_TRAINING";

/// `UpdateServiceJob` over the wire: the status and the decoded body.
async fn update(batch: &Batch, job_id: &str, priority: i64) -> (u16, Value) {
    batch
        .call(
            "/v1/updateservicejob",
            json!({"jobId": job_id, "schedulingPriority": priority}),
        )
        .await
}

/// `UpdateServiceJob` over the wire, requiring the 200.
async fn updated(batch: &Batch, case: &str, job_id: &str, priority: i64) -> Value {
    let (status, body) = update(batch, job_id, priority).await;
    assert_eq!(
        status, 200,
        "case {case}: expected UpdateServiceJob to answer 200, got {status} {body}"
    );
    body
}

/// `UpdateServiceJob` over the wire, requiring the modelled `ClientException` (HTTP 400).
async fn refused(batch: &Batch, case: &str, job_id: &str, priority: i64) -> Value {
    let (status, body) = update(batch, job_id, priority).await;
    assert_eq!(
        status, 400,
        "case {case}: expected UpdateServiceJob to be refused with 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ClientException",
        "case {case}: expected the modelled ClientException, got {body}"
    );
    body
}

/// `detail` with the one member an update is allowed to change removed, so two of these
/// compare everything ELSE the `DescribeServiceJob` answer carries.
fn without_priority(detail: &Value) -> Value {
    let mut copy = detail.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("schedulingPriority");
    }
    copy
}

/// RULE: an accepted `UpdateServiceJob` writes the request's `schedulingPriority` onto the
/// stored service job, answers with that job's own identity, and changes nothing else
/// about the record. One row per non-terminal `ServiceJobStatus` the model declares, each
/// with a different priority — both published bounds included — and alternating between a
/// record carrying every optional member and one carrying only the required ones.
#[tokio::test]
async fn update_service_job_sets_the_scheduling_priority_and_describe_reads_it_back() {
    let statuses = live_statuses();
    assert!(
        statuses.len() > 1,
        "case the status catalog: the model declares more than one non-terminal \
         ServiceJobStatus, got {statuses:?}"
    );

    for (index, status) in statuses.iter().enumerate() {
        let priority = ACCEPTED_PRIORITIES[index % ACCEPTED_PRIORITIES.len()];
        let filled = if index % 2 == 0 {
            Filled::Everything
        } else {
            Filled::Required
        };
        let case = format!("a {status} service job ({filled:?}) updated to {priority}");
        let batch = Batch::new();
        let seeded = seed(&batch, status, filled);

        let before = described(&batch, &case, &seeded.job_id).await;
        assert_eq!(
            before["status"].as_str(),
            Some(*status),
            "case {case}: the seeded job must report its own status first, got {before}"
        );

        let answer = updated(&batch, &case, &seeded.job_id, priority).await;
        assert_eq!(
            answer["jobId"].as_str(),
            Some(seeded.job_id.as_str()),
            "case {case}: the answer names the job that was updated, got {answer}"
        );
        assert_eq!(
            answer["jobArn"].as_str(),
            Some(seeded.job_arn.as_str()),
            "case {case}: expected the stored jobArn, got {answer}"
        );
        assert_eq!(
            answer["jobName"].as_str(),
            Some(seeded.job_name.as_str()),
            "case {case}: expected the stored jobName, got {answer}"
        );

        let after = described(&batch, &case, &seeded.job_id).await;
        assert_eq!(
            after["schedulingPriority"].as_i64(),
            Some(priority),
            "case {case}: DescribeServiceJob must report the priority the update set, \
             got {after}"
        );
        assert_eq!(
            without_priority(&after),
            without_priority(&before),
            "case {case}: an update must change schedulingPriority alone, got {after} \
             from {before}"
        );
    }
}

/// RULE: the LAST accepted `UpdateServiceJob` is the one that sticks, and setting the
/// priority a job already carries leaves it at that value. One row per shape of sequence:
/// rising, falling, the same value twice, and a single update.
#[tokio::test]
async fn the_last_scheduling_priority_an_update_sets_is_the_one_that_sticks() {
    for (case, sequence) in [
        ("rising", vec![0, 1, 9999]),
        ("falling", vec![9999, 500, 0]),
        ("the same value twice", vec![77, 77]),
        ("one update only", vec![1234]),
    ] {
        let batch = Batch::new();
        let seeded = seed(&batch, state::SERVICE_JOB_SUBMITTED, Filled::Everything);

        for priority in &sequence {
            updated(&batch, case, &seeded.job_id, *priority).await;
            let detail = described(&batch, case, &seeded.job_id).await;
            assert_eq!(
                detail["schedulingPriority"].as_i64(),
                Some(*priority),
                "case {case}: after updating to {priority} the job must report it, got \
                 {detail}"
            );
        }

        let detail = described(&batch, case, &seeded.job_id).await;
        let last = *sequence.last().expect("every row updates at least once");
        assert_eq!(
            detail["schedulingPriority"].as_i64(),
            Some(last),
            "case {case}: expected the last priority of {sequence:?}, got {detail}"
        );
    }
}

/// RULE: the same holds for a service job this emulator minted itself — an update
/// replaces whatever priority `SubmitServiceJob` stored, including none at all.
#[tokio::test]
async fn updating_a_submitted_service_job_replaces_the_priority_it_was_submitted_with() {
    for (case, submit_priority, new_priority) in [
        ("submitted without a priority", None, 9999),
        ("submitted at the minimum", Some(0), 4321),
        ("submitted at the maximum", Some(9999), 0),
        ("updated to the priority it already had", Some(100), 100),
    ] {
        let batch = Batch::new();
        let (queue, _queue_arn) = environment_and_queue(&batch).await;
        let mut body = json!({
            "jobName": "sj-priority",
            "jobQueue": queue,
            "serviceJobType": SERVICE_JOB_TYPE,
            "serviceRequestPayload": r#"{"TrainingJobName":"t-1"}"#,
        });
        if let Some(priority) = submit_priority {
            body["schedulingPriority"] = json!(priority);
        }
        let submit = submitted(&batch, case, body).await;
        let job_id = submit["jobId"]
            .as_str()
            .expect("SubmitServiceJob answers a jobId")
            .to_string();

        let answer = updated(&batch, case, &job_id, new_priority).await;
        assert_eq!(
            answer["jobId"].as_str(),
            Some(job_id.as_str()),
            "case {case}: the answer names the submitted job, got {answer}"
        );
        assert_eq!(
            answer["jobArn"].as_str(),
            submit["jobArn"].as_str(),
            "case {case}: expected the jobArn the submit minted, got {answer}"
        );
        assert_eq!(
            answer["jobName"].as_str(),
            Some("sj-priority"),
            "case {case}: expected the submitted jobName, got {answer}"
        );

        let detail = described(&batch, case, &job_id).await;
        assert_eq!(
            detail["schedulingPriority"].as_i64(),
            Some(new_priority),
            "case {case}: expected the updated priority, got {detail}"
        );
    }
}

/// RULE: a `schedulingPriority` outside the range the member documents is refused with the
/// modelled `ClientException` (HTTP 400) and persists nothing. One row per value past a
/// published bound, each checked against a job whose stored priority must survive the
/// refusal.
#[tokio::test]
async fn update_service_job_refuses_a_scheduling_priority_outside_the_documented_range() {
    for priority in REJECTED_PRIORITIES {
        let case = format!("schedulingPriority {priority}");
        let batch = Batch::new();
        // `Filled::Everything` seeds a record that already carries a priority, so the
        // refusal can be shown to leave it alone.
        let seeded = seed(&batch, state::SERVICE_JOB_SUBMITTED, Filled::Everything);
        let before = described(&batch, &case, &seeded.job_id).await;

        refused(&batch, &case, &seeded.job_id, *priority).await;

        let after = described(&batch, &case, &seeded.job_id).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must persist nothing, got {after} from {before}"
        );
    }

    // The published bounds themselves are accepted, so the refusal is the range and not
    // the comparison's edge.
    for priority in [
        i64::from(state::MIN_SCHEDULING_PRIORITY),
        i64::from(state::MAX_SCHEDULING_PRIORITY),
    ] {
        let case = format!("the published bound {priority}");
        let batch = Batch::new();
        let seeded = seed(&batch, state::SERVICE_JOB_SUBMITTED, Filled::Required);
        updated(&batch, &case, &seeded.job_id, priority).await;
        let detail = described(&batch, &case, &seeded.job_id).await;
        assert_eq!(
            detail["schedulingPriority"].as_i64(),
            Some(priority),
            "case {case}: a published bound is inside the range, got {detail}"
        );
    }
}

/// RULE: an `UpdateServiceJob` for a `jobId` the service holds no service job for answers
/// the modelled `ClientException` (HTTP 400), persists nothing — the id stays
/// unresolvable and no record is minted for it — and leaves the service jobs the store
/// does hold untouched. One row per form an unknown id can take, none of which this
/// service minted.
#[tokio::test]
async fn update_service_job_refuses_a_job_id_the_service_does_not_hold() {
    let batch = Batch::new();
    let live = seed(&batch, state::SERVICE_JOB_SUBMITTED, Filled::Everything);
    let before = described(&batch, "the live service job", &live.job_id).await;
    let stored = batch.store().lock().service_jobs.len();

    let unknown: Vec<(&str, String)> = vec![
        ("a word", "does-not-exist".to_string()),
        (
            "a UUID this service never minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "a service job ARN",
            state::service_job_arn(REGION, "00000000-0000-4000-8000-000000000001"),
        ),
        ("one character", "x".to_string()),
        ("a 128-character id", "9".repeat(128)),
        (
            "the live job's id with one digit changed",
            flip_last_hex_digit(&live.job_id),
        ),
    ];

    for (case, id) in &unknown {
        assert_ne!(
            id, &live.job_id,
            "case {case}: the row must not name the live service job, got {id:?}"
        );

        refused(&batch, case, id, 1).await;

        let (status, body) = describe(&batch, id).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused update mints nothing, so the id stays unresolvable, \
             got {status} {body}"
        );
        assert_eq!(
            batch.store().lock().service_jobs.len(),
            stored,
            "case {case}: a refused update must not add a service job to the store"
        );
    }

    let after = described(&batch, "the live service job", &live.job_id).await;
    assert_eq!(
        after, before,
        "case the live service job: updating other ids must not touch it, got {after}"
    );
}

/// What had ended the service job an update then arrives for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The record was stored in this terminal status.
    Stored(&'static str),
    /// A `TerminateServiceJob` is what ended it.
    Terminated,
}

/// RULE: a service job in a terminal `ServiceJobStatus` has nothing left to schedule, so
/// an `UpdateServiceJob` for it answers the modelled `ClientException` (HTTP 400) and the
/// record is left EXACTLY as it is — the whole `DescribeServiceJob` answer, priority
/// included. One row per terminal status the model declares, plus a job an earlier
/// `TerminateServiceJob` ended.
#[tokio::test]
async fn update_service_job_refuses_a_service_job_that_has_already_ended() {
    let mut rows: Vec<Ended> = TERMINAL_STATUSES.iter().copied().map(Ended::Stored).collect();
    rows.push(Ended::Terminated);
    assert!(
        rows.len() > 2,
        "case the status catalog: the model declares more than one terminal \
         ServiceJobStatus, got {rows:?}"
    );

    for row in rows {
        let case = format!("{row:?}");
        let batch = Batch::new();
        let seeded = match row {
            Ended::Stored(status) => seed(&batch, status, Filled::Everything),
            Ended::Terminated => {
                let seeded = seed(&batch, state::SERVICE_JOB_RUNNABLE, Filled::Everything);
                terminate(&batch, &case, &seeded.job_id, "ended before the update").await;
                seeded
            }
        };

        let before = described(&batch, &case, &seeded.job_id).await;
        assert!(
            before["status"]
                .as_str()
                .is_some_and(state::is_terminal_service_job_status),
            "case {case}: this row must start in a terminal status, got {before}"
        );

        let body = refused(&batch, &case, &seeded.job_id, 4242).await;
        // The refusal names the precondition, never the job: one sentence for either
        // terminal status and whatever ended the job.
        assert_eq!(
            error_message(&body),
            state::SERVICE_JOB_COMPLETED_NOT_UPDATABLE,
            "case {case}: expected the completed-service-job refusal, got {body}"
        );

        let after = described(&batch, &case, &seeded.job_id).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must change nothing at all, got {after} from \
             {before}"
        );
        assert_ne!(
            after["schedulingPriority"].as_i64(),
            Some(4242),
            "case {case}: the refused priority must not have been written, got {after}"
        );
    }
}

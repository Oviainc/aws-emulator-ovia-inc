// SPDX-License-Identifier: CC0-1.0

//! `CancelJob`, end to end over the wire.
//!
//! The RULE under test, from the operation's own doc
//! (`smithy_query(query="doc", shape="CancelJob")`): `CancelJob` always succeeds, and
//! what it does to the job it names is decided ONLY by the status that job is in when
//! the call arrives — a job that has not progressed past `RUNNABLE` ends `FAILED`
//! carrying the caller's `reason` and `isCancelled`; a `PENDING` job's cancel is deferred
//! until the jobs it depends on complete; a job in `STARTING` or `RUNNING` "aren't
//! cancelled"; a job already in a terminal status is left exactly as it is; and an id the
//! service holds no job for cancels nothing. "However, the API operation still succeeds,
//! even if no job is cancelled."
//!
//! Each test asserts that rule over a SET of inputs: the status the job is in when the
//! cancel arrives, the size and alphabet of the `reason`, whether a cancel was asked for
//! at all, and the form of an unknown `jobId`.
//!
//! `STARTING` and `RUNNING` are modelled but unreachable in this emulator (nothing runs a
//! container, so a job never leaves `RUNNABLE` except to fail), so no row can put a job
//! in them; the handler's own case for them is covered by the state machinery it shares
//! with `TerminateJob`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_job, job_definition, job_queue, Batch,
};

/// The longest `reason` these rows submit: the limit the member documents — "This
/// parameter has a limit of 1024 characters"
/// (`smithy_query(query="doc", shape="CancelJobRequest", member="reason")`).
const MAX_REASON_CHARS: usize = 1024;

/// One emulator with a queue and a job definition to submit into.
struct Fixture {
    batch: Batch,
    queue_arn: String,
    definition: String,
}

/// A fresh emulator carrying a compute environment, a queue over it and one container
/// job definition, all named after `prefix`.
async fn fixture(prefix: &str) -> Fixture {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, &format!("ce-{prefix}")).await;
    let queue_arn = job_queue(&batch, &format!("q-{prefix}"), &environment_arn).await;
    let (definition, _) =
        job_definition(&batch, &format!("jd-{prefix}"), container_properties()).await;
    Fixture {
        batch,
        queue_arn,
        definition,
    }
}

/// Submit one job called `name`, optionally depending on other jobs; answer its id.
async fn submit(fixture: &Fixture, name: &str, depends_on: Value) -> String {
    let mut body = json!({
        "jobName": name,
        "jobQueue": &fixture.queue_arn,
        "jobDefinition": &fixture.definition,
    });
    if !depends_on.is_null() {
        body["dependsOn"] = depends_on;
    }
    let answered = fixture.batch.ok("/v1/submitjob", body).await;
    answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit {name}: expected a jobId, got {answered}"))
        .to_string()
}

/// `CancelJob` on `job_id` with `reason`, asserting the 200 and the empty body the
/// operation models (`CancelJobResponse` has no members).
async fn cancel(batch: &Batch, case: &str, job_id: &str, reason: &str) {
    let (status, body) = batch
        .call("/v1/canceljob", json!({"jobId": job_id, "reason": reason}))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected CancelJob to succeed with 200, got {status} {body}"
    );
    assert_eq!(
        body,
        json!({}),
        "case {case}: CancelJob models no output members, got {body}"
    );
}

/// Poll `job_id` until it reaches a terminal status, and answer its last `JobDetail`.
async fn poll_until_terminal(batch: &Batch, job_id: &str) -> Value {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    let mut detail = describe_job(batch, job_id).await;
    for _ in 0..24 {
        if state::is_terminal_status(detail["status"].as_str().unwrap_or_default()) {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(span / 4)).await;
        detail = describe_job(batch, job_id).await;
    }
    detail
}

/// Poll `job_id` until it reports `status`, and answer its last `JobDetail`. Driven off
/// OBSERVED state rather than a fixed sleep, so a slow machine delays a case instead of
/// breaking it.
async fn poll_until_status(batch: &Batch, job_id: &str, status: &str) -> Value {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    let mut detail = describe_job(batch, job_id).await;
    for _ in 0..40 {
        if detail["status"].as_str() == Some(status) {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(span / 20)).await;
        detail = describe_job(batch, job_id).await;
    }
    detail
}

/// Cancel `job_id` with `reason` and assert the rule against the status the job was
/// OBSERVED in immediately before the call — so what is checked is the rule, not a
/// guess about where the clock had got to.
///
/// Never called on a `PENDING` job: that case is deferred rather than immediate and has
/// its own test below.
async fn cancel_and_check_rule(batch: &Batch, case: &str, job_id: &str, reason: &str) {
    let before = describe_job(batch, job_id).await;
    let status_before = before["status"].as_str().unwrap_or_default().to_string();
    assert_ne!(
        status_before, state::JOB_PENDING,
        "case {case}: this row must not be PENDING, got {before}"
    );

    cancel(batch, case, job_id, reason).await;

    let after = describe_job(batch, job_id).await;
    if state::is_terminal_status(&status_before) {
        // A job already in a terminal status is left EXACTLY as it is — the whole
        // JobDetail, so the reason it already carries is not overwritten either.
        assert_eq!(
            after, before,
            "case {case}: cancelling a {status_before} job must change nothing, got {after}"
        );
        return;
    }

    assert_eq!(
        after["status"].as_str(),
        Some(state::JOB_FAILED),
        "case {case}: a {status_before} job is cancelled and ends FAILED, got {after}"
    );
    assert_eq!(
        after["statusReason"].as_str(),
        Some(reason),
        "case {case}: the caller's reason is what DescribeJobs reports, got {after}"
    );
    assert_eq!(
        after["isCancelled"].as_bool(),
        Some(true),
        "case {case}: a cancelled job reports isCancelled, got {after}"
    );
    assert_eq!(
        after["isTerminated"].as_bool(),
        Some(false),
        "case {case}: a cancelled job is not a terminated one, got {after}"
    );
    let created_at = before["createdAt"].as_i64().unwrap_or_default();
    assert!(
        after["stoppedAt"]
            .as_i64()
            .is_some_and(|stopped| stopped >= created_at),
        "case {case}: expected stoppedAt at or after createdAt {created_at}, got {after}"
    );
    assert!(
        after["startedAt"].as_i64().is_some_and(|started| started == 0),
        "case {case}: nothing ran, so no startedAt is invented, got {after}"
    );
}

/// What the job had already done when the cancel arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// Just submitted: the job has not progressed past `RUNNABLE`.
    NotPastRunnable,
    /// Polled until the clock took it to a terminal status.
    EndedByTheClock,
    /// Already cancelled by an earlier `CancelJob` (the idempotence row).
    AlreadyCancelled,
}

/// RULE: `CancelJob` ends a job that has not progressed past `RUNNABLE` — `FAILED`,
/// `isCancelled`, the caller's `reason` as its `statusReason` — and leaves a job that is
/// already in a terminal status exactly as it is; either way the call succeeds. One row
/// per starting point, and the reasons differ in size (a short one, one at the documented
/// 1024-character limit) and alphabet (ASCII, non-ASCII).
#[tokio::test]
async fn cancel_job_ends_a_job_that_has_not_progressed_past_runnable() {
    let rows: Vec<(&str, Start, String)> = vec![
        (
            "a submitted job, plain reason",
            Start::NotPastRunnable,
            "stop it".to_string(),
        ),
        (
            "a submitted job, a reason at the documented 1024-character limit",
            Start::NotPastRunnable,
            "r".repeat(MAX_REASON_CHARS),
        ),
        (
            "a submitted job, a non-ASCII reason",
            Start::NotPastRunnable,
            "arrêt immédiat ✋ 停止".to_string(),
        ),
        (
            "a job the clock already ended",
            Start::EndedByTheClock,
            "too late".to_string(),
        ),
        (
            "a job an earlier CancelJob already ended",
            Start::AlreadyCancelled,
            "the second reason".to_string(),
        ),
    ];

    for (case, start, reason) in rows {
        let fixture = fixture("cancel").await;
        let job_id = submit(&fixture, "j-cancel", Value::Null).await;
        match start {
            Start::NotPastRunnable => {}
            Start::EndedByTheClock => {
                let ended = poll_until_terminal(&fixture.batch, &job_id).await;
                // The clock's own reason, which the cancel below must not overwrite.
                assert_eq!(
                    ended["statusReason"].as_str(),
                    Some(state::NO_CONTAINER_RUNTIME_REASON),
                    "case {case}: the clock ends the job with its own reason, got {ended}"
                );
            }
            Start::AlreadyCancelled => {
                cancel(&fixture.batch, case, &job_id, "the first reason").await;
                let cancelled = describe_job(&fixture.batch, &job_id).await;
                assert_eq!(
                    cancelled["statusReason"].as_str(),
                    Some("the first reason"),
                    "case {case}: the first cancel is what the job records, got {cancelled}"
                );
            }
        }

        cancel_and_check_rule(&fixture.batch, case, &job_id, &reason).await;

        if reason.chars().count() == MAX_REASON_CHARS {
            let after = describe_job(&fixture.batch, &job_id).await;
            assert_eq!(
                after["statusReason"].as_str().map(|text| text.chars().count()),
                Some(MAX_REASON_CHARS),
                "case {case}: a reason at the limit is stored whole, got {after}"
            );
        }
    }
}

/// Whether the dependent job in the table below is cancelled while it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    /// `CancelJob` is called while the job is `PENDING` on its dependency.
    CancelledWhilePending,
    /// Nothing cancels it: the dependency's own end is what ends it.
    LeftAlone,
}

/// RULE: a cancel asked for while a job is `PENDING` on the jobs it depends on does not
/// end it there and then — "A PENDING job is cancelled after all dependency jobs are
/// completed" — and when they complete the job ends `FAILED` carrying the CANCEL's reason
/// and `isCancelled`, never passing through `RUNNABLE`. Two rows over one dependency: the
/// dependent that was cancelled while waiting, and the one that was left alone (which
/// ends with the dependency's own reason instead).
#[tokio::test]
async fn a_cancel_asked_for_while_pending_falls_due_with_the_dependencies() {
    const REASON: &str = "cancel me when my dependency is done";

    let fixture = fixture("pending").await;
    let dependency = submit(&fixture, "j-dependency", Value::Null).await;
    let rows = [
        (Waiting::CancelledWhilePending, "d-cancelled"),
        (Waiting::LeftAlone, "d-left-alone"),
    ];

    let mut dependents = Vec::new();
    for (row, name) in rows {
        let id = submit(
            &fixture,
            name,
            json!([{"jobId": &dependency, "type": "SEQUENTIAL"}]),
        )
        .await;
        dependents.push((row, id));
    }

    for (row, id) in &dependents {
        let waiting = poll_until_status(&fixture.batch, id, state::JOB_PENDING).await;
        assert_eq!(
            waiting["status"].as_str(),
            Some(state::JOB_PENDING),
            "case {row:?}: the dependent must be waiting in PENDING first, got {waiting}"
        );
        if *row != Waiting::CancelledWhilePending {
            continue;
        }

        cancel(&fixture.batch, "cancel a PENDING job", id, REASON).await;
        // DEFERRED: the job is still waiting, unless its dependency finished in the
        // meantime — in which case the cancel has already fallen due.
        let immediately = describe_job(&fixture.batch, id).await;
        let status = immediately["status"].as_str().unwrap_or_default();
        assert!(
            status == state::JOB_PENDING || status == state::JOB_FAILED,
            "case {row:?}: a cancelled PENDING job waits for its dependency or is \
             already FAILED, never anything else, got {immediately}"
        );
        if status == state::JOB_PENDING {
            assert_eq!(
                immediately["statusReason"],
                Value::Null,
                "case {row:?}: nothing is recorded on the job until the cancel falls due, \
                 got {immediately}"
            );
            assert_eq!(
                immediately["isCancelled"].as_bool(),
                Some(false),
                "case {row:?}: the job is not cancelled yet, got {immediately}"
            );
            assert_eq!(
                immediately["stoppedAt"],
                Value::Null,
                "case {row:?}: a job that has not ended has no stoppedAt, got {immediately}"
            );
        }
    }

    for (row, id) in &dependents {
        let ended = poll_until_terminal(&fixture.batch, id).await;
        let expected_reason = match row {
            Waiting::CancelledWhilePending => REASON,
            Waiting::LeftAlone => state::DEPENDENT_JOB_FAILED_REASON,
        };
        assert_eq!(
            ended["status"].as_str(),
            Some(state::JOB_FAILED),
            "case {row:?}: the dependent ends FAILED, got {ended}"
        );
        assert_eq!(
            ended["statusReason"].as_str(),
            Some(expected_reason),
            "case {row:?}: expected the reason {expected_reason:?}, got {ended}"
        );
        assert_eq!(
            ended["isCancelled"].as_bool(),
            Some(*row == Waiting::CancelledWhilePending),
            "case {row:?}: isCancelled reports whether a cancel ended it, got {ended}"
        );
        assert_eq!(
            ended["isTerminated"].as_bool(),
            Some(false),
            "case {row:?}: nothing terminated this job, got {ended}"
        );
    }

    // The dependency itself ended on the clock, untouched by the cancel of a job that
    // merely waited on it.
    let dependency_detail = describe_job(&fixture.batch, &dependency).await;
    assert_eq!(
        dependency_detail["statusReason"].as_str(),
        Some(state::NO_CONTAINER_RUNTIME_REASON),
        "case the dependency: cancelling a dependent must not touch it, got {dependency_detail}"
    );
    assert_eq!(
        dependency_detail["isCancelled"].as_bool(),
        Some(false),
        "case the dependency: it was never cancelled, got {dependency_detail}"
    );
}

/// RULE: `CancelJob` for a `jobId` the service holds no job for succeeds and cancels
/// nothing — "the API operation still succeeds, even if no job is cancelled" — and it
/// persists nothing for that id. One row per form an unknown id can take, none of which
/// this service minted.
#[tokio::test]
async fn cancelling_a_job_the_service_does_not_hold_succeeds_and_changes_nothing() {
    let fixture = fixture("unknown").await;
    let live = submit(&fixture, "j-live", Value::Null).await;

    let account = engine::EMULATOR_ACCOUNT_ID;
    let unknown: Vec<(&str, String)> = vec![
        ("a word", "does-not-exist".to_string()),
        (
            "a UUID this service never minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "a job ARN",
            format!("arn:aws:batch:us-east-1:{account}:job/00000000-0000-4000-8000-000000000001"),
        ),
        ("one character", "x".to_string()),
        ("a 128-character id", "9".repeat(128)),
        (
            "the live job's id with one digit changed",
            flip_last_hex_digit(&live),
        ),
    ];

    for (case, id) in &unknown {
        assert_ne!(
            id, &live,
            "case {case}: the row must not name the live job, got {id:?}"
        );
        cancel(&fixture.batch, case, id, "no such job").await;

        // Nothing was persisted for the id: DescribeJobs reports no job for it.
        let described = fixture
            .batch
            .ok("/v1/describejobs", json!({"jobs": [id]}))
            .await;
        assert_eq!(
            described["jobs"].as_array().map(Vec::len),
            Some(0),
            "case {case}: cancelling an unknown id must persist nothing, got {described}"
        );
    }

    // And the one job the service does hold was not touched by any of them.
    let detail = describe_job(&fixture.batch, &live).await;
    assert_ne!(
        detail["statusReason"].as_str(),
        Some("no such job"),
        "case the live job: cancelling other ids must not touch it, got {detail}"
    );
    assert_eq!(
        detail["isCancelled"].as_bool(),
        Some(false),
        "case the live job: it was never cancelled, got {detail}"
    );
}

/// `id` with its last hex digit changed, so the result has the shape of a job id this
/// service mints but names no job.
pub(crate) fn flip_last_hex_digit(id: &str) -> String {
    let mut flipped: Vec<char> = id.chars().collect();
    if let Some(last) = flipped.last_mut() {
        *last = if *last == '0' { '1' } else { '0' };
    }
    flipped.into_iter().collect()
}

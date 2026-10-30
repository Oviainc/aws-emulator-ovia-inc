// SPDX-License-Identifier: CC0-1.0

//! The body of the modelled `ConflictException` a job operation is refused with when
//! the job's `State` does not admit it.
//!
//! RULE under test: a job operation that acts on a job that has not started yet is
//! admitted only while the job is `WAITING`, and in any other `State` it is refused
//! with the modelled 409 whose `Message` states what cannot be done to the requested
//! job and the `State` a job must be in for it to be done — fixed text per operation,
//! varying only in that operation's own verb — while the identity of the job travels
//! in the exception's own `ResourceId` and `ResourceType` members
//! (`smithy_query(query="shape", shape="ConflictException")` documents them as "The
//! unique identifier for the resource that couldn't be found." and "The type of the
//! resource with the conflict."). So neither the job's id nor the state it is
//! actually in appears in the sentence.
//!
//! The rows span what the rule generalises over: both operations the rule covers
//! (`CancelJob` and `StartJob`), every `State` a stored job can be in that does not
//! admit them — the one it passes through while running and each terminal one it can
//! reach — and every `Type` of job the model admits and a request can carry, so the
//! wording is keyed to the operation and not to a job's details.
//!
//! Every test drives the service through `MultiServiceRouter::dispatch` with a real
//! restJson1 request, so the route, the status, the error code and the serialized
//! body are all exercised.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::data_sets::{harness, Answer};
use crate::job_lifecycle::{
    cancel_job, get_job, job_of_type, settle_past_the_execution_span, start_job,
};
use crate::jobs::{data_set_with_revision, job_cases};

/// The whole `Message` AWS answers a cancel of a job that is not waiting with —
/// stated here independently of the crate's own constants, so a change to them is a
/// failure rather than a tautology.
const CANCEL_CONFLICT: &str =
    "Cannot cancel the requested Job. To cancel a Job, it must be in the WAITING state.";

/// The same sentence for the other operation the rule covers, in its own verb.
const START_CONFLICT: &str =
    "Cannot start the requested Job. To start a Job, it must be in the WAITING state.";

/// `ResourceType` for a job: a value of the model's `ResourceType` enum
/// (`smithy_query(query="shape", shape="ResourceType")`).
const JOB: &str = "JOB";

/// The `State` values a stored job can be in that admit neither operation: the one a
/// running job carries, and the terminal ones a job reaches here — a cancelled job,
/// and one whose execution finished.
///
/// Source: `smithy_query(query="shape", shape="State")` reports
/// `[WAITING, IN_PROGRESS, ERROR, COMPLETED, CANCELLED, TIMED_OUT]`, of which
/// `WAITING` is the only one either operation is admitted in.
const REFUSING_STATES: &[&str] = &["IN_PROGRESS", "CANCELLED", "ERROR"];

/// Put the job `job_id` into `state`, through the API where the API can reach it, and
/// return nothing but the assertion that it got there.
async fn move_job_to(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &str,
    job_id: &str,
    state: &str,
) {
    match state {
        "IN_PROGRESS" => {
            let started = start_job(router, job_id).await;
            assert_eq!(
                started.status, 202,
                "case {case}: expected 202 from StartJob, got {} body {:?}",
                started.status, started.body
            );
        }
        "CANCELLED" => {
            let cancelled = cancel_job(router, job_id).await;
            assert_eq!(
                cancelled.status, 204,
                "case {case}: expected 204 from CancelJob, got {} body {:?}",
                cancelled.status, cancelled.body
            );
        }
        "ERROR" => {
            let started = start_job(router, job_id).await;
            assert_eq!(
                started.status, 202,
                "case {case}: expected 202 from StartJob, got {} body {:?}",
                started.status, started.body
            );
            // The terminal state is owed by the clock one execution span after the
            // start; reading past that instant materialises it without the test
            // spending the span.
            settle_past_the_execution_span(engine);
        }
        other => panic!("case {case}: the table names no way to reach State {other:?}"),
    }

    let reached = get_job(router, job_id).await;
    assert_eq!(
        reached.string("State"),
        state,
        "case {case}: expected the job to be in State {state}, got {:?}",
        reached.member("State")
    );
}

/// Assert that `answer` is the modelled conflict for `operation` on a job whose
/// `State` does not admit it: the 409, the modelled error, the operation's own fixed
/// sentence, and the job's identity in the exception's own members rather than in the
/// text.
fn assert_state_conflict(case: &str, answer: &Answer, expected: &str, job_id: &str, state: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case}: expected 409, got {} body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ConflictException"),
        "case {case}: expected ConflictException, got {:?}",
        answer.error
    );
    let message = answer.string("Message");
    assert_eq!(
        message, expected,
        "case {case}: expected the fixed refusal message, got {message:?}"
    );
    assert!(
        !message.contains(job_id),
        "case {case}: expected no request value in the message, got {message:?}"
    );
    assert!(
        !message.contains(state),
        "case {case}: expected the message not to report the job's current state, \
         got {message:?}"
    );
    assert_eq!(
        answer.member("ResourceId").as_str(),
        Some(job_id),
        "case {case}: expected ResourceId to report the job asked about, got {:?}",
        answer.member("ResourceId")
    );
    assert_eq!(
        answer.member("ResourceType").as_str(),
        Some(JOB),
        "case {case}: expected ResourceType={JOB}, got {:?}",
        answer.member("ResourceType")
    );
}

/// RULE: `CancelJob` and `StartJob` are admitted only on a `WAITING` job; in every
/// other `State` each is refused with the modelled 409 carrying that operation's
/// fixed sentence and the job's identity in `ResourceId`/`ResourceType`, and the
/// refused call changes nothing. Holds for every `Type` of job and every `State` a
/// stored job can be refused in.
#[tokio::test]
async fn a_job_operation_the_state_does_not_admit_carries_the_fixed_conflict_message() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "state-conflict").await;

    // Every `Message` seen per operation, so each sentence can be shown to be the
    // SAME one across states and job types rather than merely right row by row.
    let mut cancel_messages: BTreeSet<String> = BTreeSet::new();
    let mut start_messages: BTreeSet<String> = BTreeSet::new();
    let mut rows = 0usize;

    for state in REFUSING_STATES {
        for job in job_cases(&data_set_id, &revision_id) {
            let case = format!("{} in State {state}", job.job_type);
            let (id, _) = job_of_type(&router, job.job_type, job.member, job.detail.clone()).await;
            move_job_to(&router, &engine, &case, &id, state).await;

            let before = get_job(&router, &id).await;
            let before_updated_at = before.member("UpdatedAt").clone();

            let cancelled = cancel_job(&router, &id).await;
            assert_state_conflict(
                &format!("CancelJob on {case}"),
                &cancelled,
                CANCEL_CONFLICT,
                &id,
                state,
            );
            cancel_messages.insert(cancelled.string("Message"));

            let started = start_job(&router, &id).await;
            assert_state_conflict(
                &format!("StartJob on {case}"),
                &started,
                START_CONFLICT,
                &id,
                state,
            );
            start_messages.insert(started.string("Message"));

            // A refused call is not a write: the record it refused is as it was.
            let after = get_job(&router, &id).await;
            assert_eq!(
                after.string("State"),
                *state,
                "case {case}: expected the refused calls to leave State alone, got {:?}",
                after.member("State")
            );
            assert_eq!(
                after.member("UpdatedAt"),
                &before_updated_at,
                "case {case}: expected the refused calls to leave UpdatedAt alone, got {:?}",
                after.member("UpdatedAt")
            );
            rows += 1;
        }
    }

    assert_eq!(
        cancel_messages.iter().cloned().collect::<Vec<String>>(),
        vec![CANCEL_CONFLICT.to_string()],
        "expected one fixed cancel message across all {rows} cases, got {cancel_messages:?}"
    );
    assert_eq!(
        start_messages.iter().cloned().collect::<Vec<String>>(),
        vec![START_CONFLICT.to_string()],
        "expected one fixed start message across all {rows} cases, got {start_messages:?}"
    );
}

/// RULE (the admitted side of the same rule): a `WAITING` job admits both operations,
/// so the refusal above is keyed to the job's `State` and not to the operation. The
/// cancel moves the job to the terminal `CANCELLED` a following read reports.
#[tokio::test]
async fn a_waiting_job_admits_both_operations() {
    let (router, _engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "state-admits").await;

    for job in job_cases(&data_set_id, &revision_id) {
        let case = job.job_type;
        let (id, _) = job_of_type(&router, job.job_type, job.member, job.detail.clone()).await;
        let cancelled = cancel_job(&router, &id).await;
        assert_eq!(
            cancelled.status, 204,
            "case {case}: expected a WAITING job to admit CancelJob, got {} body {:?}",
            cancelled.status, cancelled.body
        );
        assert_eq!(
            cancelled.body,
            Value::Null,
            "case {case}: expected an empty body from CancelJob, got {:?}",
            cancelled.body
        );

        let (other, _) = job_of_type(&router, job.job_type, job.member, job.detail).await;
        let started = start_job(&router, &other).await;
        assert_eq!(
            started.status, 202,
            "case {case}: expected a WAITING job to admit StartJob, got {} body {:?}",
            started.status, started.body
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a start acts on: the ONE job its `JobId` label
//! names, and the execution instant that one start fixed.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//! The harness (`emulator`, `harness`, `seconds`, `is_modelled_id`) is the one
//! `data_sets.rs` already uses, the job catalog is `jobs.rs`'s, and the job calls are
//! `job_lifecycle.rs`'s.
//!
//! `job_lifecycle.rs` pins what a start does to the job it names — `IN_PROGRESS`, then
//! the terminal `State` the clock owes it one execution span later. The rules here are
//! the other half of that contract, which no suite stated:
//!
//! 1. A start is a mutation of exactly one stored job: every other stored job — one
//!    still waiting, one already started — is left exactly as it was, and the 202
//!    carries no response members, the model giving `StartJob` an output with none.
//! 2. A `JobId` outside the modelled `Id` pattern is refused by the boundary as a 400
//!    `ValidationException` before any job is reached, while one inside it that names
//!    nothing stored is the 404; neither changes a stored job.
//! 3. The terminal `State` a start sets going is owed at an instant THAT start fixed,
//!    so two jobs started a measurable gap apart come due that same gap apart: nothing
//!    is owed before a job's own due instant, and a settlement lands on it.
//!
//! The rows span what the rules generalise over: every `Type` a request can carry (so
//! no rule is keyed to one job's details), each published bound of the `JobId` pattern
//! and one character past it, and both start orders of a pair of jobs.

use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use aws_smithy_types::DateTime;
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;

use crate::data_sets::{emulator, harness, is_modelled_id, seconds, Answer};
use crate::job_lifecycle::{get_job, job_of_type, start_job};
use crate::jobs::{data_set_with_revision, job_cases, ABSENT_ID};

/// The `State` values a job cannot leave on its own.
///
/// Source: `smithy_query(query="shape", shape="State")` reports
/// `[WAITING, IN_PROGRESS, ERROR, COMPLETED, CANCELLED, TIMED_OUT]`, mirrored by
/// `crate::ops::types::STATE_VALUES`; `WAITING` and `IN_PROGRESS` are the two a job
/// still moves out of.
const TERMINAL_STATES: &[&str] = &["ERROR", "COMPLETED", "CANCELLED", "TIMED_OUT"];

/// The gap left between the two starts whose due instants are compared. Long enough to
/// be visible at the resolution the wire encoding carries, and far shorter than the
/// execution span, so an instant between the two due ones exists.
const START_GAP: Duration = Duration::from_millis(300);

/// How far past (or short of) a due instant a settlement is driven: enough to keep the
/// read off the boundary, and less than [`START_GAP`], so an instant just past one
/// job's due instant is still short of the other's.
const DUE_MARGIN_SECONDS: f64 = 0.05;

/// How closely a settled `UpdatedAt` must sit on the instant the execution came due.
const DUE_INSTANT_TOLERANCE_SECONDS: f64 = 1.0;

/// Materialise, through the store, every transition the clock owes AT `instant` — the
/// reading a caller would get at that instant, without a test spending it in wall
/// clock time. `job_lifecycle.rs`'s `settle_past_the_execution_span` drives the store
/// past every due instant; these tests need a chosen one, so that a job whose
/// execution is not due yet can be told apart from one whose is.
fn settle_at(engine: &Arc<engine::InMemoryEngine>, instant: f64) {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    state.lock().settle_jobs(DateTime::from_secs_f64(instant));
}

/// Identifiers for the `JobId` label, spanning how one can satisfy or break the
/// modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$`
/// (`smithy_query(query="operation", operation="StartJob")` reports it on the
/// `@httpLabel` member): each published length bound, one character past each, and a
/// character the pattern's class does not admit.
fn job_id_cases() -> Vec<(&'static str, String)> {
    vec![
        ("one short of the published minimum", "a".repeat(29)),
        ("at the published minimum", "a".repeat(30)),
        ("at the published maximum", "z".repeat(40)),
        ("one past the published maximum", "z".repeat(41)),
        (
            "mixed case and digits inside the bounds",
            format!("{}{}", "aB9".repeat(10), "7"),
        ),
        (
            "a hyphen is not alphanumeric",
            format!("{}-{}", "a".repeat(16), "b".repeat(15)),
        ),
        (
            "an underscore is not alphanumeric",
            format!("{}_{}", "9".repeat(20), "Z".repeat(11)),
        ),
    ]
}

/// RULE: a start mutates exactly the one job its `JobId` names — every other stored
/// job keeps the `State` and the timestamps it had, whether it is still waiting or has
/// already been started — and answers the modelled 202 with no response members.
#[tokio::test]
async fn starting_one_job_touches_only_the_job_it_names() {
    let router = emulator();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "start-one-of-many").await;

    // One waiting job per `Type` a request can carry, so every start below has every
    // other kind of job standing beside the one it names.
    let mut jobs: Vec<(&'static str, String, f64)> = Vec::new();
    for case in job_cases(&data_set_id, &revision_id) {
        let (id, created_at) = job_of_type(&router, case.job_type, case.member, case.detail).await;
        jobs.push((case.job_type, id, created_at));
    }

    for index in 0..jobs.len() {
        let (label, id, _) = jobs[index].clone();
        let started = start_job(&router, &id).await;
        assert_eq!(
            started.status, 202,
            "case {label}: expected 202 from StartJob, got {} body {:?}",
            started.status, started.body
        );
        // The model gives `StartJob` an output shape with no members, so a 202 carries
        // none either.
        assert!(
            started
                .body
                .as_object()
                .is_none_or(|members| members.is_empty()),
            "case {label}: expected no response members on the 202, got {:?}",
            started.body
        );

        for (other, (other_label, other_id, other_created_at)) in jobs.iter().enumerate() {
            let fetched = get_job(&router, other_id).await;
            assert_eq!(
                fetched.status, 200,
                "case {label} then {other_label}: expected 200 from GetJob, got {} body {:?}",
                fetched.status, fetched.body
            );
            let state = fetched.string("State");
            if other == index {
                // The execution span is never zero, so the job just started is running.
                assert_eq!(
                    state, "IN_PROGRESS",
                    "case {label}: expected the started job to be IN_PROGRESS, got {state:?}"
                );
                continue;
            }
            if other > index {
                assert_eq!(
                    state, "WAITING",
                    "case {label} then {other_label}: expected a job no start named to \
                     stay WAITING, got {state:?}"
                );
                assert_eq!(
                    seconds(fetched.member("UpdatedAt"), other_label),
                    *other_created_at,
                    "case {label} then {other_label}: expected UpdatedAt to stay at the \
                     creation instant"
                );
                assert!(
                    fetched.member("Errors").is_null(),
                    "case {label} then {other_label}: expected no Errors on a job nothing \
                     started, got {:?}",
                    fetched.member("Errors")
                );
            } else {
                // An earlier start of a DIFFERENT job is not undone by this one: that
                // job is running or has reached its terminal state, never waiting again.
                assert_ne!(
                    state, "WAITING",
                    "case {label} then {other_label}: expected an already started job to \
                     stay off WAITING, got {state:?}"
                );
            }
            assert_eq!(
                seconds(fetched.member("CreatedAt"), other_label),
                *other_created_at,
                "case {label} then {other_label}: expected CreatedAt to stay at the \
                 creation instant"
            );
        }
    }
}

/// RULE: a `JobId` outside the modelled pattern is a 400 `ValidationException` — the
/// request is malformed, so no job is looked at; one inside the pattern that names
/// nothing stored is the 404 `ResourceNotFoundException` naming the job. Neither
/// answer changes a stored job.
#[tokio::test]
async fn a_start_naming_a_malformed_or_absent_job_changes_nothing() {
    let router = emulator();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "start-bad-id").await;
    let case = job_cases(&data_set_id, &revision_id).remove(0);
    let (stored_id, _) = job_of_type(&router, case.job_type, case.member, case.detail).await;
    let before = get_job(&router, &stored_id).await;

    let mut rows = job_id_cases();
    rows.push(("a well-formed id nothing holds", ABSENT_ID.to_string()));
    for (label, id) in rows {
        let answer = start_job(&router, &id).await;
        if is_modelled_id(&id) {
            assert_eq!(
                answer.status, 404,
                "case {label}: expected 404 for the well-formed but unknown {id:?}, got {} \
                 body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {label}: expected ResourceNotFoundException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.member("ResourceId").as_str(),
                Some(id.as_str()),
                "case {label}: expected the refusal to name the job, got {:?}",
                answer.member("ResourceId")
            );
            assert_eq!(
                answer.member("ResourceType").as_str(),
                Some("JOB"),
                "case {label}: expected ResourceType=JOB, got {:?}",
                answer.member("ResourceType")
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {label}: expected 400 for {id:?}, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {label}: expected ValidationException for {id:?}, got {:?}",
                answer.error
            );
        }

        let after = get_job(&router, &stored_id).await;
        assert_eq!(
            after.body, before.body,
            "case {label}: expected the refused start to leave the stored job untouched, \
             got {:?} instead of {:?}",
            after.body, before.body
        );
    }
}

/// RULE: the terminal `State` a start sets going is owed at an instant that start
/// fixed, so two jobs started a gap apart come due that same gap apart: at an instant
/// short of a job's own due one it is still running, and the settlement that
/// materialises lands on that job's due instant rather than on the reader's clock.
#[tokio::test]
async fn each_started_job_is_owed_its_terminal_state_at_its_own_due_instant() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "own-due-instant").await;
    let span = dataexchange::state::JOB_EXECUTION_SECONDS;

    // Two jobs of DIFFERENT `Type`, started in the order the rows give them, so the
    // rule is read off the start instants rather than off either job's details.
    let mut cases = job_cases(&data_set_id, &revision_id);
    let second = cases.remove(1);
    let first = cases.remove(0);
    let label = format!("{} then {}", first.job_type, second.job_type);

    let (early_id, _) = job_of_type(&router, first.job_type, first.member, first.detail).await;
    let (late_id, _) = job_of_type(&router, second.job_type, second.member, second.detail).await;

    let early_running = start_and_read_start_instant(&router, &early_id, &label).await;
    let early_started = seconds(early_running.member("UpdatedAt"), &label);
    sleep(START_GAP);
    let late_running = start_and_read_start_instant(&router, &late_id, &label).await;
    let late_started = seconds(late_running.member("UpdatedAt"), &label);
    assert!(
        late_started > early_started,
        "case {label}: expected the second start to be recorded later, got {late_started} \
         after {early_started}"
    );

    // Short of either due instant, neither execution is owed anything.
    settle_at(&engine, early_started + span - DUE_MARGIN_SECONDS);
    for (which, id) in [("the first", &early_id), ("the second", &late_id)] {
        let running = get_job(&router, id).await;
        assert_eq!(
            running.string("State"),
            "IN_PROGRESS",
            "case {label}: expected {which} job to still be running before its due instant, \
             got {:?}",
            running.member("State")
        );
    }

    // Past the first job's due instant but short of the second's, exactly one of them
    // is owed its terminal state.
    settle_at(&engine, early_started + span + DUE_MARGIN_SECONDS);
    let settled_early = get_job(&router, &early_id).await;
    let early_state = settled_early.string("State");
    assert!(
        TERMINAL_STATES.contains(&early_state.as_str()),
        "case {label}: expected the first job to be terminal past its due instant, got \
         {early_state:?}"
    );
    let early_settled_at = seconds(settled_early.member("UpdatedAt"), &label);
    assert!(
        (early_settled_at - early_started - span).abs() < DUE_INSTANT_TOLERANCE_SECONDS,
        "case {label}: expected the first job to settle at its own due instant \
         ({early_started} + {span}), got {early_settled_at}"
    );
    let still_running = get_job(&router, &late_id).await;
    assert_eq!(
        still_running.string("State"),
        "IN_PROGRESS",
        "case {label}: expected the second job to still be running at an instant short of \
         its own due one, got {:?}",
        still_running.member("State")
    );

    // Past the second job's due instant, it too is terminal — at ITS due instant, a
    // start gap later than the first job's, and the first job's record has not moved.
    settle_at(&engine, late_started + span + DUE_MARGIN_SECONDS);
    let settled_late = get_job(&router, &late_id).await;
    let late_state = settled_late.string("State");
    assert!(
        TERMINAL_STATES.contains(&late_state.as_str()),
        "case {label}: expected the second job to be terminal past its due instant, got \
         {late_state:?}"
    );
    let late_settled_at = seconds(settled_late.member("UpdatedAt"), &label);
    assert!(
        (late_settled_at - late_started - span).abs() < DUE_INSTANT_TOLERANCE_SECONDS,
        "case {label}: expected the second job to settle at its own due instant \
         ({late_started} + {span}), got {late_settled_at}"
    );
    assert!(
        late_settled_at > early_settled_at,
        "case {label}: expected the two due instants to stand the start gap apart, got \
         {late_settled_at} and {early_settled_at}"
    );
    let reread_early = get_job(&router, &early_id).await;
    assert_eq!(
        reread_early.body, settled_early.body,
        "case {label}: expected the first job's settled record to stay as it was, got {:?} \
         instead of {:?}",
        reread_early.body, settled_early.body
    );
}

/// Start `job_id` and hand back the `GetJob` answer that reports the instant the start
/// was recorded at, which is the instant the execution's due one is measured from.
async fn start_and_read_start_instant(
    router: &smithy_server_runtime::MultiServiceRouter,
    job_id: &str,
    label: &str,
) -> Answer {
    let started = start_job(router, job_id).await;
    assert_eq!(
        started.status, 202,
        "case {label}: expected 202 from StartJob, got {} body {:?}",
        started.status, started.body
    );
    let running = get_job(router, job_id).await;
    assert_eq!(
        running.string("State"),
        "IN_PROGRESS",
        "case {label}: expected State=IN_PROGRESS straight after StartJob, got {:?}",
        running.member("State")
    );
    running
}

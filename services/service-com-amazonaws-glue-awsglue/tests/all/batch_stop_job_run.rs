// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchStopJobRun`: the stop a client submits for the runs of one
//! job definition, and what `GetJobRun` reports about them afterwards.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and the job
//! and run fixtures come from [`crate::job_runs`], so what is asserted is HTTP behaviour
//! and not a direct call into a handler.
//!
//! The rule under test: a stop is submitted PER NAMED RUN of the named job definition —
//! a run that has not finished moves into `STOPPING` and is reported in
//! `SuccessfulSubmissions`, with `STOPPED` owed it one span later, while a run the stop
//! cannot reach (one the named job does not hold, or one that has already finished) is
//! reported in `Errors` with its own `ErrorDetail` — so every named run gets exactly one
//! outcome and a per-run problem never fails the batch.
//!
//! The recorded case — a stop naming one in-flight run of one job — is ONE row of the
//! tables below; the rows differ along how many runs the batch names, how many of the
//! job's runs it names, the command kind of the job, which way each named run misses, and
//! whether the same run is named twice.
//!
//! The due instants the lifecycle turns on are read and moved THROUGH THE STORE rather
//! than slept for, so the window in which a run is stopping is deterministic.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::JOB_NOT_FOUND;
use service_com_amazonaws_glue_awsglue::state::{
    JOB_RUN_ID_PREFIX, JOB_RUN_STATE_FAILED, JOB_RUN_STATE_STARTING, JOB_RUN_STATE_STOPPED,
    JOB_RUN_STATE_STOPPING, TERMINAL_JOB_RUN_STATES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::JOB_RUN_NOT_FOUND_MESSAGE;
use crate::job_runs::{
    already_past, emulator_with_engine, far_future, get_run, job_body, set_due_instant, start_run,
    stored_runs, OWED_TRANSITION_WINDOW,
};
use crate::wire::{call, error_code, ok, ERROR_DETAIL_TAG};

/// How many run ids one `BatchStopJobRun` request may name.
///
/// Source: `BatchStopJobRunRequest$JobRunIds`, `length 1..25`
/// (`smithy_query(query="operation",operation="BatchStopJobRun")`). Both bounds are
/// exercised below, and so is one id past the maximum.
const JOB_RUN_IDS_BOUNDS: (usize, usize) = (1, 25);

/// The `ErrorDetail$ErrorCode` reported for a run the named job does not hold.
const ENTITY_NOT_FOUND_CODE: &str = "EntityNotFoundException";

/// The `ErrorDetail$ErrorCode` reported for a run that has already finished, so there is
/// nothing left to stop.
const INVALID_INPUT_CODE: &str = "InvalidInputException";

/// A run id of the shape a start mints (the prefix plus 64 lowercase hex characters)
/// that no start ever minted, distinct per `nonce`.
fn unminted_run_id(nonce: u64) -> String {
    format!("{JOB_RUN_ID_PREFIX}{nonce:064x}")
}

/// Submit a stop for `run_ids` and answer the body, which must be a 200.
async fn stop_runs(router: &MultiServiceRouter, job_name: &str, run_ids: &[String]) -> Value {
    ok(
        router,
        "BatchStopJobRun",
        &json!({ "JobName": job_name, "JobRunIds": run_ids }),
    )
    .await
}

/// The `SuccessfulSubmissions` of a stop, as the `JobRunId` of each entry — asserting
/// that every entry names the job the request named, since an entry that does not say
/// which job's run was stopped does not record a submission.
fn submitted_ids(case: &str, answer: &Value, job_name: &str) -> Vec<String> {
    let entries = answer
        .get("SuccessfulSubmissions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected SuccessfulSubmissions, got {answer}"));
    entries
        .iter()
        .map(|entry| {
            assert_eq!(
                entry.get("JobName").and_then(Value::as_str),
                Some(job_name),
                "case {case:?}: a submission names the job definition, got {entry}"
            );
            entry
                .get("JobRunId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a submission names the run it stopped, got {entry}")
                })
                .to_string()
        })
        .collect()
}

/// The `Errors` of a stop, as the `(JobRunId, ErrorCode, ErrorMessage)` of each entry —
/// asserting the members every entry carries: the job, the run, and the `__type` tag the
/// service writes onto a nested `ErrorDetail`.
fn error_entries(case: &str, answer: &Value, job_name: &str) -> Vec<(String, String, String)> {
    let entries = answer
        .get("Errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected Errors, got {answer}"));
    entries
        .iter()
        .map(|entry| {
            assert_eq!(
                entry.get("JobName").and_then(Value::as_str),
                Some(job_name),
                "case {case:?}: an error names the job definition, got {entry}"
            );
            let detail = entry
                .get("ErrorDetail")
                .unwrap_or_else(|| panic!("case {case:?}: an error carries a detail, got {entry}"));
            assert_eq!(
                detail.get("__type").and_then(Value::as_str),
                Some(ERROR_DETAIL_TAG),
                "case {case:?}: a nested ErrorDetail carries its own type tag, got {detail}"
            );
            let message = detail
                .get("ErrorMessage")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an error says what went wrong, got {entry}")
                });
            assert!(
                !message.is_empty(),
                "case {case:?}: an error says what went wrong, got {entry}"
            );
            (
                entry
                    .get("JobRunId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an error names the run in question, got {entry}")
                    })
                    .to_string(),
                detail
                    .get("ErrorCode")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an error carries a code, got {entry}")
                    })
                    .to_string(),
                message.to_string(),
            )
        })
        .collect()
}

/// Every named run gets exactly one outcome: the two lists together account for the
/// request's `JobRunIds`, one entry per id named.
fn assert_one_outcome_per_named_run(case: &str, answer: &Value, named: usize) {
    let submissions = answer
        .get("SuccessfulSubmissions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let errors = answer.get("Errors").and_then(Value::as_array).map_or(0, Vec::len);
    assert_eq!(
        submissions + errors,
        named,
        "case {case:?}: expected one outcome for each of the {named} runs named, got \
         {submissions} submissions and {errors} errors in {answer}"
    );
}

/// Start `count` runs of `name`, holding every owed transition open so they are all still
/// in flight however long the starts take.
async fn start_runs_in_flight(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        ids.push(start_run(router, &json!({ "JobName": name })).await);
        set_due_instant(engine, name, far_future());
    }
    ids
}

/// The `JobRunState` the store holds for one run.
fn stored_state(engine: &Arc<engine::InMemoryEngine>, job_name: &str, run_id: &str) -> String {
    stored_runs(engine, job_name)
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap_or_else(|| panic!("the store must hold run {run_id:?}"))
        .state
}

// ---------------------------------------------------------------------------
// The stop a client submits
// ---------------------------------------------------------------------------

/// A stop submitted for runs that have not finished is accepted for each of them: the run
/// is reported as stopping while the owed transition is held open, and as stopped once it
/// falls due — and a run of the same job the batch did not name is untouched.
///
/// The rows differ along the command kind, how many runs the job has in flight, how many
/// of them the batch names (including both published bounds of `JobRunIds`), and the
/// order the ids are named in.
#[tokio::test]
async fn a_submitted_stop_moves_each_named_run_to_stopped() {
    let (router, engine) = emulator_with_engine();
    let (min_ids, max_ids) = JOB_RUN_IDS_BOUNDS;

    // (case, job name, CreateJob members beyond the required three, runs to start, how
    //  many of them to stop, whether to name them newest first)
    let cases: Vec<(&str, &str, Value, usize, usize, bool)> = vec![
        (
            // The recorded case: one in-flight run of one job, named on its own.
            "the only run of a job",
            "stop-one",
            json!({}),
            min_ids,
            min_ids,
            false,
        ),
        (
            "every run of a job with several in flight",
            "stop-all",
            json!({
                "Command": { "Name": "pythonshell", "ScriptLocation": "s3://etl/x.py" },
                "ExecutionProperty": { "MaxConcurrentRuns": 4 },
            }),
            4,
            4,
            false,
        ),
        (
            "some of the runs in flight, named newest first",
            "stop-some",
            json!({
                "Command": { "Name": "gluestreaming", "ScriptLocation": "s3://etl/s.py" },
                "ExecutionProperty": { "MaxConcurrentRuns": 5 },
            }),
            5,
            2,
            true,
        ),
        (
            "the published maximum number of run ids",
            "stop-max",
            json!({ "ExecutionProperty": { "MaxConcurrentRuns": 25 } }),
            max_ids,
            max_ids,
            false,
        ),
    ];

    for (case, name, job, started, to_stop, newest_first) in cases {
        ok(&router, "CreateJob", &job_body(name, job)).await;
        let ids = start_runs_in_flight(&router, &engine, name, started).await;
        let named: Vec<String> = if newest_first {
            ids.iter().rev().take(to_stop).cloned().collect()
        } else {
            ids.iter().take(to_stop).cloned().collect()
        };
        let untouched: Vec<String> = ids
            .iter()
            .filter(|id| !named.contains(id))
            .cloned()
            .collect();

        let answer = stop_runs(&router, name, &named).await;
        assert_one_outcome_per_named_run(case, &answer, named.len());
        assert_eq!(
            submitted_ids(case, &answer, name)
                .into_iter()
                .collect::<BTreeSet<_>>(),
            named.iter().cloned().collect::<BTreeSet<_>>(),
            "case {case:?}: a stop of runs in flight is submitted for each of them, got \
             {answer}"
        );
        assert!(
            error_entries(case, &answer, name).is_empty(),
            "case {case:?}: no named run failed, got {answer}"
        );

        // While the owed stop is not due, every named run is reported as stopping, and
        // has neither finished nor failed.
        for run_id in &named {
            let run = get_run(&router, name, run_id).await;
            assert_eq!(
                run.get("JobRunState").and_then(Value::as_str),
                Some(JOB_RUN_STATE_STOPPING),
                "case {case:?}: a run a stop was submitted for is {JOB_RUN_STATE_STOPPING} \
                 until the stop falls due, got {run}"
            );
            assert!(
                run.get("CompletedOn").is_none(),
                "case {case:?}: a run that is still stopping has no CompletedOn, got {run}"
            );
            assert!(
                run.get("ErrorMessage").is_none(),
                "case {case:?}: a stopped run failed at nothing, got {run}"
            );
        }
        // A run of the same job the batch did not name keeps the state it was in.
        for run_id in &untouched {
            let run = get_run(&router, name, run_id).await;
            assert_eq!(
                run.get("JobRunState").and_then(Value::as_str),
                Some(JOB_RUN_STATE_STARTING),
                "case {case:?}: a run the stop did not name is untouched, got {run}"
            );
        }

        // The stop the clock owes falls due a non-zero span after the submission.
        let (lower, upper) = OWED_TRANSITION_WINDOW;
        for run in stored_runs(&engine, name) {
            if !named.contains(&run.id) {
                continue;
            }
            let due = run.settles_at.unwrap_or_else(|| {
                panic!("case {case:?}: run {} must owe its stop", run.id)
            });
            let span = due
                .duration_since(run.last_modified_on)
                .unwrap_or_else(|_| panic!("case {case:?}: the stop is due after the submission"));
            assert!(
                span >= lower && span <= upper,
                "case {case:?}: the owed stop must be due {lower:?}..={upper:?} after the \
                 submission, got {span:?}"
            );
        }

        // Once it is due, each named run is stopped — and a run nobody stopped settles
        // into the state the clock owes it instead.
        set_due_instant(&engine, name, already_past());
        for run_id in &named {
            let run = get_run(&router, name, run_id).await;
            assert_eq!(
                run.get("JobRunState").and_then(Value::as_str),
                Some(JOB_RUN_STATE_STOPPED),
                "case {case:?}: the stop the clock owed is {JOB_RUN_STATE_STOPPED}, got {run}"
            );
            assert!(
                run.get("CompletedOn").is_some(),
                "case {case:?}: a stopped run has finished, got {run}"
            );
            assert!(
                run.get("ErrorMessage").is_none(),
                "case {case:?}: a run brought to rest by a stop carries no ErrorMessage, got \
                 {run}"
            );
        }
        for run_id in &untouched {
            let run = get_run(&router, name, run_id).await;
            assert_eq!(
                run.get("JobRunState").and_then(Value::as_str),
                Some(JOB_RUN_STATE_FAILED),
                "case {case:?}: a run no stop was submitted for settles into \
                 {JOB_RUN_STATE_FAILED}, got {run}"
            );
            crate::job_runs::assert_script_was_unreadable(case, &run);
        }
    }

    assert!(
        TERMINAL_JOB_RUN_STATES.contains(&JOB_RUN_STATE_STOPPED),
        "the state a stop brings a run to rest in must be one it never leaves, got \
         {TERMINAL_JOB_RUN_STATES:?}"
    );
}

/// A run the stop cannot reach is reported per run, with its own code and message, and
/// never as a failure of the whole call: the runs it can reach in the same batch are
/// stopped.
///
/// The rows differ along how the named run misses: never minted, a run of another job, an
/// id without the prefix a run id carries, a run that already settled, and a run that a
/// previous stop already brought to rest.
#[tokio::test]
async fn a_run_the_stop_cannot_reach_is_reported_per_run() {
    let (router, engine) = emulator_with_engine();
    let name = "unreachable-runs";
    ok(
        &router,
        "CreateJob",
        &job_body(name, json!({ "ExecutionProperty": { "MaxConcurrentRuns": 6 } })),
    )
    .await;
    ok(&router, "CreateJob", &job_body("other-job", json!({}))).await;

    // A run of another job, held in flight so only its OWNER could stop it.
    let other_run = start_run(&router, &json!({ "JobName": "other-job" })).await;
    set_due_instant(&engine, "other-job", far_future());

    // A run of this job that the clock already finished, and one a stop already finished.
    let settled = start_runs_in_flight(&router, &engine, name, 1).await[0].clone();
    set_due_instant(&engine, name, already_past());
    let _ = get_run(&router, name, &settled).await;
    let already_stopped = start_runs_in_flight(&router, &engine, name, 1).await[0].clone();
    stop_runs(&router, name, std::slice::from_ref(&already_stopped)).await;
    set_due_instant(&engine, name, already_past());
    let _ = get_run(&router, name, &already_stopped).await;

    let cases: Vec<(&str, String, &str)> = vec![
        ("never minted", unminted_run_id(0), ENTITY_NOT_FOUND_CODE),
        ("a run of another job", other_run, ENTITY_NOT_FOUND_CODE),
        (
            "the id without its prefix",
            settled[JOB_RUN_ID_PREFIX.len()..].to_string(),
            ENTITY_NOT_FOUND_CODE,
        ),
        ("a run the clock finished", settled.clone(), INVALID_INPUT_CODE),
        (
            "a run an earlier stop finished",
            already_stopped.clone(),
            INVALID_INPUT_CODE,
        ),
    ];

    for (case, unreachable, expected_code) in cases {
        // One reachable run alongside the unreachable one, so the batch is not refused as
        // a whole for a per-run problem.
        let reachable = start_runs_in_flight(&router, &engine, name, 1).await[0].clone();
        let named = vec![unreachable.clone(), reachable.clone()];
        let answer = stop_runs(&router, name, &named).await;
        assert_one_outcome_per_named_run(case, &answer, named.len());
        assert_eq!(
            submitted_ids(case, &answer, name),
            vec![reachable.clone()],
            "case {case:?}: the run the stop could reach is stopped, got {answer}"
        );
        let errors = error_entries(case, &answer, name);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: exactly the unreachable run failed, got {answer}"
        );
        let (reported_id, code, message) = &errors[0];
        assert_eq!(
            reported_id, &unreachable,
            "case {case:?}: the error names the run the request named, got {answer}"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code}, got {answer}"
        );
        if code == ENTITY_NOT_FOUND_CODE {
            // A run the job holds none of is the job-run miss every lookup refuses with
            // one fixed sentence: the entry names the run in `JobRunId`, so the detail
            // echoes nothing back.
            assert_eq!(
                message, JOB_RUN_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the job-run miss sentence, got {message:?}"
            );
        } else {
            assert!(
                message.contains(&unreachable),
                "case {case:?}: the message must name the run that could not be stopped, \
                 got {message:?}"
            );
        }
        assert_eq!(
            stored_state(&engine, name, &reachable),
            JOB_RUN_STATE_STOPPING,
            "case {case:?}: the reachable run really is stopping"
        );
        set_due_instant(&engine, name, already_past());
        let _ = get_run(&router, name, &reachable).await;
    }

    // A job this account does not hold holds none of the named runs: every entry fails
    // the same way, and the call itself still succeeds.
    for unknown_job in ["absent", "UNREACHABLE-RUNS", "unreachable-run"] {
        let named = vec![unminted_run_id(1), unminted_run_id(2)];
        let answer = stop_runs(&router, unknown_job, &named).await;
        assert_one_outcome_per_named_run(unknown_job, &answer, named.len());
        assert!(
            submitted_ids(unknown_job, &answer, unknown_job).is_empty(),
            "case {unknown_job:?}: no run of an unknown job can be stopped, got {answer}"
        );
        let errors = error_entries(unknown_job, &answer, unknown_job);
        assert_eq!(
            errors.len(),
            named.len(),
            "case {unknown_job:?}: every named run fails, got {answer}"
        );
        for (reported_id, code, message) in &errors {
            assert!(
                named.contains(reported_id),
                "case {unknown_job:?}: the errors name the runs the request named, got {answer}"
            );
            assert_eq!(
                code, ENTITY_NOT_FOUND_CODE,
                "case {unknown_job:?}: expected {ENTITY_NOT_FOUND_CODE}, got {answer}"
            );
            // The job-definition miss is a fixed sentence that discloses no name: the
            // entry AROUND the detail is what names the job and the run.
            // `job_not_found_message.rs` pins that wording.
            assert_eq!(
                message, JOB_NOT_FOUND,
                "case {unknown_job:?}: expected the job-miss sentence, got {message:?}"
            );
        }
    }
}

/// Submitting the same stop again changes nothing: the submission is accepted again, and
/// the stop the clock already owed stays due at the instant the FIRST submission set — so
/// a client that retries a batch does not postpone the stop it asked for.
///
/// The rows differ along how the stop is repeated: a second call naming the same run, and
/// one call naming that run twice.
#[tokio::test]
async fn a_repeated_stop_keeps_the_stop_already_owed() {
    let (router, engine) = emulator_with_engine();
    let name = "repeated-stop";
    ok(
        &router,
        "CreateJob",
        &job_body(name, json!({ "ExecutionProperty": { "MaxConcurrentRuns": 2 } })),
    )
    .await;

    for case in ["a second call", "the same id named twice"] {
        let run_id = start_runs_in_flight(&router, &engine, name, 1).await[0].clone();
        let first = stop_runs(&router, name, std::slice::from_ref(&run_id)).await;
        assert_eq!(
            submitted_ids(case, &first, name),
            vec![run_id.clone()],
            "case {case:?}: the first stop is submitted, got {first}"
        );
        let owed = stored_runs(&engine, name)
            .into_iter()
            .find(|run| run.id == run_id)
            .and_then(|run| run.settles_at)
            .unwrap_or_else(|| panic!("case {case:?}: the stopping run must owe its stop"));

        let named = if case == "a second call" {
            vec![run_id.clone()]
        } else {
            vec![run_id.clone(), run_id.clone()]
        };
        let again = stop_runs(&router, name, &named).await;
        assert_one_outcome_per_named_run(case, &again, named.len());
        assert_eq!(
            submitted_ids(case, &again, name),
            named,
            "case {case:?}: a repeated stop is accepted for each id named, got {again}"
        );
        assert!(
            error_entries(case, &again, name).is_empty(),
            "case {case:?}: a repeated stop of a stopping run is no error, got {again}"
        );
        let owed_again = stored_runs(&engine, name)
            .into_iter()
            .find(|run| run.id == run_id)
            .and_then(|run| run.settles_at)
            .unwrap_or_else(|| panic!("case {case:?}: the stopping run must still owe its stop"));
        assert_eq!(
            owed_again, owed,
            "case {case:?}: the repeated stop must not move the owed stop, expected {owed:?}, \
             got {owed_again:?}"
        );

        set_due_instant(&engine, name, already_past());
        let run = get_run(&router, name, &run_id).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_STOPPED),
            "case {case:?}: the run still ends up {JOB_RUN_STATE_STOPPED}, got {run}"
        );
    }
}

/// A run a stop was submitted for is still in flight until it has stopped: it holds the
/// concurrency slot it occupied while it is stopping, and frees it once stopped.
///
/// The rows differ along the concurrency the job allows, including the documented default
/// a job that names no `ExecutionProperty` gets.
#[tokio::test]
async fn a_stopping_run_holds_its_slot_until_it_has_stopped() {
    let (router, engine) = emulator_with_engine();

    for (case, extra, allowed) in [
        ("the documented default concurrency", json!({}), 1usize),
        (
            "a job that allows three concurrent runs",
            json!({ "ExecutionProperty": { "MaxConcurrentRuns": 3 } }),
            3usize,
        ),
    ] {
        let name = format!("slot-{allowed}");
        ok(&router, "CreateJob", &job_body(&name, extra)).await;
        let ids = start_runs_in_flight(&router, &engine, &name, allowed).await;
        stop_runs(&router, &name, &ids).await;

        // Every slot is still taken: the runs are stopping, not stopped.
        let (status, answer) = call(&router, "StartJobRun", &json!({ "JobName": &name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a start past the limit is refused while the runs are stopping, \
             got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("ConcurrentRunsExceededException"),
            "case {case:?}: expected ConcurrentRunsExceededException, got {answer}"
        );

        // Once the stops fall due the slots are free again.
        set_due_instant(&engine, &name, already_past());
        let next = start_run(&router, &json!({ "JobName": &name })).await;
        set_due_instant(&engine, &name, far_future());
        let run = get_run(&router, &name, &next).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_STARTING),
            "case {case:?}: a stopped run frees its slot, got {run}"
        );
    }
}

/// `JobRunIds` is bounded: a list naming one id past the published maximum, and an empty
/// one, are refused at the request boundary rather than partly applied.
#[tokio::test]
async fn a_run_id_list_outside_its_published_bounds_is_refused() {
    let (router, engine) = emulator_with_engine();
    let name = "bounded-batch";
    ok(
        &router,
        "CreateJob",
        &job_body(name, json!({ "ExecutionProperty": { "MaxConcurrentRuns": 2 } })),
    )
    .await;
    let run_id = start_runs_in_flight(&router, &engine, name, 1).await[0].clone();
    let (min_ids, max_ids) = JOB_RUN_IDS_BOUNDS;

    for (case, count) in [("empty", 0usize), ("one past the maximum", max_ids + 1)] {
        let named: Vec<String> = (0..count as u64).map(unminted_run_id).collect();
        let (status, answer) = call(
            &router,
            "BatchStopJobRun",
            &json!({ "JobName": name, "JobRunIds": named }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a list of {count} run ids is outside {min_ids}..={max_ids}, got \
             {status} with {answer}"
        );
        assert_eq!(
            stored_state(&engine, name, &run_id),
            JOB_RUN_STATE_STARTING,
            "case {case:?}: a refused batch stops nothing"
        );
    }

    // The published maximum itself is served, however few of the ids name a real run.
    let mut named: Vec<String> = (0..max_ids as u64 - 1).map(unminted_run_id).collect();
    named.push(run_id.clone());
    let answer = stop_runs(&router, name, &named).await;
    assert_one_outcome_per_named_run("at the maximum", &answer, named.len());
    assert_eq!(
        submitted_ids("at the maximum", &answer, name),
        vec![run_id.clone()],
        "case at the maximum: the one real run is stopped, got {answer}"
    );
    assert_eq!(
        stored_state(&engine, name, &run_id),
        JOB_RUN_STATE_STOPPING,
        "case at the maximum: the real run is stopping"
    );
}

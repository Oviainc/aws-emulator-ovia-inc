// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wording of a JOB-RUN MISS: the refusal `GetJobRun`,
//! `StartJobRun` (retrying a previous run) and `BatchStopJobRun` answer with when the job
//! definition they address holds no run under the id the request named.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and the job
//! and run fixtures come from [`crate::job_runs`], so what is asserted is HTTP behaviour
//! and not a direct call into a handler.
//!
//! The rule under test: the refusal that a named job run does not exist is the service's
//! FIXED sentence for that miss — the same text for every request, carrying no part of
//! the request (neither the run id asked for nor the job name it was asked through) —
//! alongside HTTP 400 and `EntityNotFoundException`.
//!
//! Each test is a table whose rows differ along what the rule generalises over: the job
//! name's value, length, casing and character set, the job's command kind, how the named
//! run misses (never minted, a run of another job, an id stripped of the prefix a minted
//! id carries, an id at each bound of the member's documented length), how many runs the
//! request names, and which operation asks. The recorded case — a read-back of one bogus
//! id against a `pythonshell` job that does hold a run — is ONE row among them.
//!
//! The expected wording is written out here as a literal rather than read from the
//! service's own constant, so these tests pin the wording instead of restating it.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::JOB_RUN_ID_PREFIX;

use crate::error_messages::{assert_refusal, assert_withholds, JOB_RUN_NOT_FOUND_MESSAGE};
use crate::job_runs::{
    create_and_start, emulator_with_engine, far_future, get_run, job_body, set_due_instant,
    start_run,
};
use crate::wire::{error_message, ok, ERROR_DETAIL_TAG};

/// The code a job-run miss carries, modelled as a client error with HTTP 400
/// (`EntityNotFoundException`, "A specified entity does not exist" — from
/// `smithy_query(query="operation",operation="GetJobRun")`).
const ENTITY_NOT_FOUND_CODE: &str = "EntityNotFoundException";

/// Job definitions spanning the axis their name generalises over: the recorded fixture's
/// own shape, a single character, mixed case, punctuation and digits, and a name at the
/// documented ceiling of `GetJobRunRequest$JobName` (`NameString`, length 1..255, from
/// `smithy_query(query="operation",operation="GetJobRun")`). Each row also varies the
/// command kind, so the wording does not depend on what kind of job holds the runs.
fn jobs() -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "recorded fixture shape",
            "sim2real-behav-job-7c20fc2cf81d".to_string(),
            json!({
                "Command": {
                    "Name": "pythonshell",
                    "PythonVersion": "3.9",
                    "ScriptLocation": "s3://etl-scripts/etl.py",
                },
                "MaxCapacity": 0.0625,
                "Timeout": 10,
            }),
        ),
        ("single character", "x".to_string(), json!({})),
        (
            "mixed case",
            "MixedCaseJob".to_string(),
            json!({ "Command": { "Name": "gluestreaming", "ScriptLocation": "s3://b/s.py" } }),
        ),
        (
            "punctuation and digits",
            "team.data_lake-2026-raw".to_string(),
            json!({ "Command": { "Name": "glueray", "Runtime": "Ray2.4", "ScriptLocation": "s3://b/r.py" } }),
        ),
        ("at the 255 ceiling", "n".repeat(255), json!({})),
    ]
}

/// Run ids the job holds no run under, spanning the axis `GetJobRunRequest$RunId`
/// generalises over (`IdString`, length 1..255, same `smithy_query` call as above): a
/// well-formed id nothing minted, the shortest and longest id the member admits, and ids
/// that differ from one the job DOES hold only by its prefix, its case or one character.
fn absent_run_ids(minted: &str) -> Vec<(&'static str, String)> {
    vec![
        ("never minted", format!("{JOB_RUN_ID_PREFIX}{:064x}", 0u64)),
        ("at the 1-character floor", "r".to_string()),
        ("at the 255 ceiling", "r".repeat(255)),
        (
            "a minted id without its prefix",
            minted[JOB_RUN_ID_PREFIX.len()..].to_string(),
        ),
        ("a minted id upper-cased", minted.to_ascii_uppercase()),
        ("a minted id with one character more", format!("{minted}0")),
    ]
}

/// A read of a run the job does not hold is refused with the fixed sentence, whatever the
/// request named: every row of [`jobs`] crossed with every row of [`absent_run_ids`], plus
/// a run that belongs to ANOTHER job definition.
#[tokio::test]
async fn a_read_of_an_absent_run_is_refused_with_the_fixed_sentence() {
    let (router, engine) = emulator_with_engine();

    // A second job holding a run of its own, so one row can name a real run id that the
    // addressed job still does not hold.
    let other_run = create_and_start(&router, "other-job", json!({})).await;
    set_due_instant(&engine, "other-job", far_future());

    for (job_case, job_name, extra) in jobs() {
        let minted = create_and_start(&router, &job_name, extra).await;
        set_due_instant(&engine, &job_name, far_future());

        let mut rows = absent_run_ids(&minted);
        rows.push(("a run of another job", other_run.clone()));

        for (run_case, run_id) in rows {
            let case = format!("{job_case} / {run_case}");
            let body = assert_refusal(
                &router,
                &case,
                "GetJobRun",
                &json!({ "JobName": job_name, "RunId": run_id }),
                ENTITY_NOT_FOUND_CODE,
                JOB_RUN_NOT_FOUND_MESSAGE,
            )
            .await;
            let message = error_message(&body);
            assert_withholds(&case, &message, &run_id);
            assert_withholds(&case, &message, &job_name);
        }

        // The run the job DOES hold is still reported, so the refusals above answered a
        // miss rather than a broken lookup.
        let run = get_run(&router, &job_name, &minted).await;
        assert_eq!(
            run.get("Id").and_then(Value::as_str),
            Some(minted.as_str()),
            "case {job_case:?}: the run the job holds is still reported, got {run}"
        );
    }
}

/// A retry that names a previous run the job does not hold is the same miss, refused with
/// the same sentence: the rows are the ids of [`absent_run_ids`].
#[tokio::test]
async fn a_retry_of_an_absent_run_is_refused_with_the_fixed_sentence() {
    let (router, engine) = emulator_with_engine();
    let job_name = "retries";
    // The job admits several runs at once, so the retry below is refused (or served) for
    // the run it names and not for the concurrency the job allows.
    ok(
        &router,
        "CreateJob",
        &job_body(job_name, json!({ "ExecutionProperty": { "MaxConcurrentRuns": 10 } })),
    )
    .await;
    let minted = start_run(&router, &json!({ "JobName": job_name })).await;
    set_due_instant(&engine, job_name, far_future());

    for (run_case, run_id) in absent_run_ids(&minted) {
        let body = assert_refusal(
            &router,
            run_case,
            "StartJobRun",
            &json!({ "JobName": job_name, "JobRunId": run_id }),
            ENTITY_NOT_FOUND_CODE,
            JOB_RUN_NOT_FOUND_MESSAGE,
        )
        .await;
        let message = error_message(&body);
        assert_withholds(run_case, &message, &run_id);
        assert_withholds(run_case, &message, job_name);
    }

    // A retry of the run the job DOES hold is served, so the refusals above are about the
    // named run and not about retrying at all.
    let retried = start_run(
        &router,
        &json!({ "JobName": job_name, "JobRunId": minted }),
    )
    .await;
    assert_ne!(
        retried, minted,
        "a retry records a run of its own, got {retried}"
    );
}

/// A stop naming a run the job does not hold reports the same sentence per run, in the
/// `ErrorDetail` of that entry — while the runs it can reach in the same batch are still
/// stopped. The rows are the ids of [`absent_run_ids`], each submitted alongside a run
/// the job does hold.
#[tokio::test]
async fn a_stop_of_an_absent_run_reports_the_fixed_sentence_per_run() {
    let (router, engine) = emulator_with_engine();
    let job_name = "stops";
    ok(
        &router,
        "CreateJob",
        &job_body(job_name, json!({ "ExecutionProperty": { "MaxConcurrentRuns": 10 } })),
    )
    .await;

    for (run_case, absent) in absent_run_ids(&format!("{JOB_RUN_ID_PREFIX}{:064x}", 1u64)) {
        let reachable = start_run(&router, &json!({ "JobName": job_name })).await;
        set_due_instant(&engine, job_name, far_future());
        let answer = ok(
            &router,
            "BatchStopJobRun",
            &json!({ "JobName": job_name, "JobRunIds": [absent.clone(), reachable.clone()] }),
        )
        .await;

        let errors = answer
            .get("Errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {run_case:?}: expected Errors, got {answer}"));
        assert_eq!(
            errors.len(),
            1,
            "case {run_case:?}: only the absent run failed, got {answer}"
        );
        let detail = errors[0]
            .get("ErrorDetail")
            .unwrap_or_else(|| panic!("case {run_case:?}: an error carries a detail, got {answer}"));
        assert_eq!(
            detail.get("__type").and_then(Value::as_str),
            Some(ERROR_DETAIL_TAG),
            "case {run_case:?}: a nested ErrorDetail carries its own type tag, got {detail}"
        );
        assert_eq!(
            detail.get("ErrorCode").and_then(Value::as_str),
            Some(ENTITY_NOT_FOUND_CODE),
            "case {run_case:?}: expected {ENTITY_NOT_FOUND_CODE}, got {detail}"
        );
        let message = detail
            .get("ErrorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, JOB_RUN_NOT_FOUND_MESSAGE,
            "case {run_case:?}: expected ErrorMessage {JOB_RUN_NOT_FOUND_MESSAGE:?}, got \
             {message:?}"
        );
        assert_withholds(run_case, message, &absent);
        assert_withholds(run_case, message, job_name);

        // The entry still says WHICH run missed, in the member that reports it.
        assert_eq!(
            errors[0].get("JobRunId").and_then(Value::as_str),
            Some(absent.as_str()),
            "case {run_case:?}: the error names the run the request named, got {answer}"
        );
        // And the run the stop could reach was submitted, so a per-run miss did not fail
        // the batch.
        let submitted: Vec<&str> = answer
            .get("SuccessfulSubmissions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {run_case:?}: expected SuccessfulSubmissions, got {answer}")
            })
            .iter()
            .filter_map(|entry| entry.get("JobRunId").and_then(Value::as_str))
            .collect();
        assert_eq!(
            submitted,
            vec![reachable.as_str()],
            "case {run_case:?}: the reachable run is stopped, got {answer}"
        );
    }
}

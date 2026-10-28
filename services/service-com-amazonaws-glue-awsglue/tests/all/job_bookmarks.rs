// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue job-bookmark slice: `GetJobBookmark` and
//! `ResetJobBookmark`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a bookmark is "a point that a job can resume processing", which a
//! RUN establishes — so a job definition nothing has run yet has no bookmark and both
//! operations answer it with `EntityNotFoundException`, and a `RunId` naming no run of the
//! job is that same miss. Once a run has written it, the read reports it, each further run
//! advances it, and a reset clears the position the next run would have resumed from while
//! leaving the entry readable. The read never writes it: only a run and a reset do.
//!
//! The rows below differ along what the rule generalises over: whether the job exists, how
//! many runs it has taken, whether a `RunId` is named and whether that id is one of the
//! job's runs.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::{job_bookmark_not_found, JOB_NOT_FOUND};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{job_body, start_run};
use crate::wire::{call, emulator, error_code, error_message, ok};

/// Read the bookmark of `job`, answering the HTTP status and body. `run_id` addresses one
/// run's bookmark when the row names one.
async fn get_bookmark(
    router: &MultiServiceRouter,
    job: &str,
    run_id: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "JobName": job });
    if let Some(run_id) = run_id {
        body["RunId"] = json!(run_id);
    }
    call(router, "GetJobBookmark", &body).await
}

/// Reset the bookmark of `job`, answering the HTTP status and body.
async fn reset_bookmark(
    router: &MultiServiceRouter,
    job: &str,
    run_id: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "JobName": job });
    if let Some(run_id) = run_id {
        body["RunId"] = json!(run_id);
    }
    call(router, "ResetJobBookmark", &body).await
}

// ---------------------------------------------------------------------------
// Absent until a run writes it
// ---------------------------------------------------------------------------

/// A job that has never run has no bookmark: the read and the reset are both
/// `EntityNotFoundException`, and the reset leaves it absent.
///
/// The rows differ along the job definition's shape — the command kind it runs, whether it
/// configures bookmark arguments at all — because a bookmark follows from a RUN and from
/// nothing the definition says.
#[tokio::test]
async fn a_job_that_has_never_run_has_no_bookmark() {
    let router = emulator();

    // (case, the job name, the extra members of the definition)
    let cases: [(&str, &str, Value); 3] = [
        ("a plain Spark job", "plain_job", json!({})),
        (
            "a job asking for bookmarks in its default arguments",
            "bookmarked_job",
            json!({ "DefaultArguments": { "--job-bookmark-option": "job-bookmark-enable" } }),
        ),
        (
            "a Python shell job",
            "shell_job",
            json!({ "Command": { "Name": "pythonshell", "PythonVersion": "3.9" } }),
        ),
    ];

    for (case, job, extra) in cases {
        ok(&router, "CreateJob", &job_body(job, extra)).await;

        for (operation, (status, body)) in [
            ("GetJobBookmark", get_bookmark(&router, job, None).await),
            ("ResetJobBookmark", reset_bookmark(&router, job, None).await),
        ] {
            assert_eq!(
                status, 400,
                "case {case}: {operation} on a never-run job must be refused, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).ends_with("EntityNotFoundException"),
                "case {case}: expected EntityNotFoundException from {operation}, got {} in \
                 {body}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                job_bookmark_not_found(job),
                "case {case}: {operation} must name the job whose bookmark is missing, got \
                 {body}"
            );
            assert!(
                body["JobBookmarkEntry"].is_null(),
                "case {case}: {operation} answers no entry for a job that has none, got {body}"
            );
        }
    }
}

/// A job name the Region holds no definition under is the JOB's own miss, not the
/// bookmark's: the job is resolved before its bookmark.
#[tokio::test]
async fn a_job_that_does_not_exist_is_the_jobs_own_miss() {
    let router = emulator();

    for (operation, (status, body)) in [
        ("GetJobBookmark", get_bookmark(&router, "no_such_job", None).await),
        (
            "ResetJobBookmark",
            reset_bookmark(&router, "no_such_job", None).await,
        ),
    ] {
        assert_eq!(
            status, 400,
            "{operation} on an unknown job must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "{operation}: expected EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            JOB_NOT_FOUND,
            "{operation} must report the job's own miss, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// A run writes it, a reset clears it
// ---------------------------------------------------------------------------

/// Each run of a job advances its bookmark, and the read reports what the runs left: the
/// run count, the latest run and the one before it.
#[tokio::test]
async fn each_run_advances_the_bookmark() {
    let router = emulator();
    // Several runs of one job are in flight at once here, so the job allows them: the
    // documented default is one (`ExecutionProperty$MaxConcurrentRuns`).
    ok(
        &router,
        "CreateJob",
        &job_body(
            "etl_job",
            json!({ "ExecutionProperty": { "MaxConcurrentRuns": 5 } }),
        ),
    )
    .await;

    let mut previous: Option<String> = None;
    for round in 1..=3u32 {
        let run_id = start_run(&router, &json!({ "JobName": "etl_job" })).await;
        let (status, body) = get_bookmark(&router, "etl_job", None).await;
        assert_eq!(
            status, 200,
            "round {round}: a job that has run has a bookmark, got {status} with {body}"
        );
        let entry = &body["JobBookmarkEntry"];
        assert_eq!(
            entry["JobName"].as_str(),
            Some("etl_job"),
            "round {round}: the entry must name its job, got {body}"
        );
        assert_eq!(
            entry["Run"].as_i64(),
            Some(i64::from(round)),
            "round {round}: the entry must count the runs that wrote it, got {body}"
        );
        assert_eq!(
            entry["Version"].as_i64(),
            Some(1),
            "round {round}: no reset has happened, so the bookmark is still its first \
             version, got {body}"
        );
        assert_eq!(
            entry["RunId"].as_str(),
            Some(run_id.as_str()),
            "round {round}: the entry must name the run that last wrote it, got {body}"
        );
        assert_eq!(
            entry["PreviousRunId"].as_str(),
            previous.as_deref(),
            "round {round}: the entry must name the run before that one, got {body}"
        );
        assert!(
            entry["JobBookmark"].is_null(),
            "round {round}: nothing ran the script, so no position was written, got {body}"
        );

        // The read changes nothing: the same call twice answers the same entry.
        let (_, again) = get_bookmark(&router, "etl_job", None).await;
        assert_eq!(
            again, body,
            "round {round}: a read must not move the bookmark, got {again} after {body}"
        );
        previous = Some(run_id);
    }
}

/// A reset clears the position the next run would have resumed from, starts a new version
/// of the bookmark and leaves the entry readable: the read that follows reports exactly
/// what the reset answered.
#[tokio::test]
async fn a_reset_clears_the_bookmark_and_leaves_it_readable() {
    let router = emulator();
    ok(
        &router,
        "CreateJob",
        &job_body(
            "etl_job",
            json!({ "ExecutionProperty": { "MaxConcurrentRuns": 5 } }),
        ),
    )
    .await;
    start_run(&router, &json!({ "JobName": "etl_job" })).await;
    let second = start_run(&router, &json!({ "JobName": "etl_job" })).await;

    let before = get_bookmark(&router, "etl_job", None).await.1;
    assert_eq!(
        before["JobBookmarkEntry"]["RunId"].as_str(),
        Some(second.as_str()),
        "the bookmark must name the latest run before the reset, got {before}"
    );

    let (status, reset) = reset_bookmark(&router, "etl_job", None).await;
    assert_eq!(
        status, 200,
        "resetting the bookmark of a job that has run must be served, got {status} with {reset}"
    );
    let entry = &reset["JobBookmarkEntry"];
    assert_eq!(
        entry["JobName"].as_str(),
        Some("etl_job"),
        "the reset entry must name its job, got {reset}"
    );
    assert_eq!(
        entry["Run"].as_i64(),
        Some(0),
        "the reset clears the run counter, got {reset}"
    );
    assert_eq!(
        entry["Version"].as_i64(),
        Some(2),
        "the reset starts the next version of the bookmark, got {reset}"
    );
    assert!(
        entry["RunId"].is_null() && entry["PreviousRunId"].is_null(),
        "the reset drops the runs the cleared position came from, got {reset}"
    );

    let after = get_bookmark(&router, "etl_job", None).await.1;
    assert_eq!(
        after, reset,
        "the read must report what the reset left; expected {reset}, got {after}"
    );

    // A further run continues from the reset state rather than from what it cleared.
    let third = start_run(&router, &json!({ "JobName": "etl_job" })).await;
    let resumed = get_bookmark(&router, "etl_job", None).await.1;
    assert_eq!(
        resumed["JobBookmarkEntry"]["Run"].as_i64(),
        Some(1),
        "the run after a reset is the first of the new version, got {resumed}"
    );
    assert_eq!(
        resumed["JobBookmarkEntry"]["RunId"].as_str(),
        Some(third.as_str()),
        "the run after a reset must be the one the bookmark names, got {resumed}"
    );
    assert_eq!(
        resumed["JobBookmarkEntry"]["Version"].as_i64(),
        Some(2),
        "the run after a reset writes into the version the reset started, got {resumed}"
    );
}

// ---------------------------------------------------------------------------
// The run a request addresses
// ---------------------------------------------------------------------------

/// `RunId` addresses the run whose bookmark is wanted: one of the job's runs is served, and
/// an id the job holds no run for is the bookmark's miss — for the read and the reset alike.
#[tokio::test]
async fn a_run_id_the_job_does_not_hold_is_a_miss() {
    let router = emulator();
    ok(&router, "CreateJob", &job_body("etl_job", json!({}))).await;
    let run_id = start_run(&router, &json!({ "JobName": "etl_job" })).await;

    let (status, body) = get_bookmark(&router, "etl_job", Some(&run_id)).await;
    assert_eq!(
        status, 200,
        "a run the job holds must have its bookmark served, got {status} with {body}"
    );
    assert_eq!(
        body["JobBookmarkEntry"]["RunId"].as_str(),
        Some(run_id.as_str()),
        "the entry must name the run that was asked about, got {body}"
    );

    for (case, unknown) in [
        ("an id nothing minted", "jr_0000000000000000"),
        ("an id of another shape", "not-a-run-id"),
    ] {
        for (operation, (status, body)) in [
            (
                "GetJobBookmark",
                get_bookmark(&router, "etl_job", Some(unknown)).await,
            ),
            (
                "ResetJobBookmark",
                reset_bookmark(&router, "etl_job", Some(unknown)).await,
            ),
        ] {
            assert_eq!(
                status, 400,
                "case {case}: {operation} for a run the job does not hold must be refused, got \
                 {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("EntityNotFoundException"),
                "case {case}: expected EntityNotFoundException from {operation}, got {} in \
                 {body}",
                error_code(&body)
            );
        }
    }

    // None of those refusals moved the bookmark the job's run wrote.
    let after = get_bookmark(&router, "etl_job", None).await.1;
    assert_eq!(
        after["JobBookmarkEntry"]["RunId"].as_str(),
        Some(run_id.as_str()),
        "a refused call must leave the bookmark where the run left it, got {after}"
    );
    assert_eq!(
        after["JobBookmarkEntry"]["Version"].as_i64(),
        Some(1),
        "a refused reset must not start a new bookmark version, got {after}"
    );
}

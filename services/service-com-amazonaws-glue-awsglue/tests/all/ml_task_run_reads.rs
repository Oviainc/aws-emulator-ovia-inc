// SPDX-License-Identifier: CC0-1.0

//! `GetMLTaskRun`: the single-run read of ONE machine learning task run.
//!
//! RULE: a task run is addressed by the pair its parent transform and its own minted id
//! make up — "you can check the stats of any task run by calling GetMLTaskRun with the
//! TaskRunID and its parent transform's TransformID" — so the read resolves the transform
//! first and then the run among the runs started OF that transform, reports the WHOLE
//! stored run (the same projection the transform's listing reports element for element),
//! and refuses either half of the address it cannot resolve with the operation's modelled
//! `EntityNotFoundException`. Reading is reporting: the only state it writes is the status
//! the CLOCK already owed the run, so two reads of one run agree and a read leaves the
//! transform's run history exactly as it found it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The transform the runs
//! below belong to is registered through [`crate::ml_transforms::given_transform`] and each
//! run started through [`crate::ml_task_runs::start_labeling_run`], so this suite and the
//! listing suite beside it stock the store the same way.
//!
//! The rows differ along what the rule generalises over: how many runs the transform holds
//! and which of them is read back, which half of the address misses and in what form (an
//! id nothing minted, a name, a changed case, the other half's id, another transform's
//! run), the status the clock has reached by the time of the read, and the published
//! `HashString` length bounds of the two id members (1 and 255, and one past each).
//!
//! The due instant an owed status turns on is moved THROUGH THE STORE rather than slept
//! for, so each status row is deterministic.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ML_TASK_RUN_STATUS_STARTING, ML_TASK_RUN_STATUS_TIMEOUT, ML_TASK_RUN_TIMED_OUT_MESSAGE,
    ML_TASK_TYPE_LABELING_SET_GENERATION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, emulator_with_engine, far_future};
use crate::ml_task_runs::{set_due_instant, start_labeling_run};
use crate::ml_transforms::given_transform;
use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The published bounds of the two id members of this operation's input. Source: both
/// `GetMLTaskRunRequest$TransformId` and `$TaskRunId` target `HashString`, whose
/// `length` is 1..255 (`smithy_query operation=GetMLTaskRun`).
const HASH_STRING_MIN: usize = 1;
/// See [`HASH_STRING_MIN`].
const HASH_STRING_MAX: usize = 255;

/// One row of the round-trip table: the case and how many runs the transform holds.
type ReadRow<'a> = (&'a str, usize);

/// One row of a refusal table: the case, the `TransformId` and the `TaskRunId` the read
/// addresses.
type MissRow<'a> = (&'a str, String, String);

/// One row of the status table: the case, the instant the owed transition falls due, the
/// status the read must then report, and whether the run has finished by then.
type StatusRow<'a> = (&'a str, fn() -> SystemTime, &'a str, bool);

/// Read one task run of one transform.
async fn read_run(
    router: &MultiServiceRouter,
    transform_id: &str,
    task_run_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GetMLTaskRun",
        &json!({ "TransformId": transform_id, "TaskRunId": task_run_id }),
    )
    .await
}

/// The task runs of `transform_id`, as the transform's own listing reports them.
async fn listed_runs(router: &MultiServiceRouter, transform_id: &str) -> Vec<Value> {
    let page = ok(
        router,
        "GetMLTaskRuns",
        &json!({ "TransformId": transform_id }),
    )
    .await;
    page.get("TaskRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("GetMLTaskRuns must report a TaskRuns list, got {page}"))
}

/// The listed run carrying `task_run_id`.
fn listed_run(runs: &[Value], task_run_id: &str) -> Value {
    runs.iter()
        .find(|run| run["TaskRunId"].as_str() == Some(task_run_id))
        .cloned()
        .unwrap_or_else(|| panic!("the listing must hold run {task_run_id:?}, got {runs:?}"))
}

/// `id` with its last character replaced by `replacement`: a well-formed id of the same
/// length that names no stored resource. Counted in CHARACTERS, so the slice cannot land
/// inside one however the id is spelled.
fn replace_last_character(id: &str, replacement: char) -> String {
    let kept: String = id.chars().take(id.chars().count().saturating_sub(1)).collect();
    format!("{kept}{replacement}")
}

#[tokio::test]
async fn every_started_run_reads_back_through_get_ml_task_run() {
    // (case, how many runs the transform holds)
    let rows: &[ReadRow<'_>] = &[
        ("one run", 1),
        ("three runs", 3),
        ("five runs", 5),
    ];

    for (case, count) in rows {
        let (router, engine) = emulator_with_engine();
        let transform = given_transform(&router, "read-back", Value::Null).await;
        let mut started = Vec::with_capacity(*count);
        for index in 0..*count {
            started.push(
                start_labeling_run(&router, &transform, &format!("s3://labels/read-{index}/"))
                    .await,
            );
        }
        // Held in flight, so the status the read reports is the one the starts recorded and
        // not one the clock moved between the read and the listing it is compared with.
        set_due_instant(&engine, far_future());

        let listing = listed_runs(&router, &transform).await;
        for (index, run_id) in started.iter().enumerate() {
            let (status, read) = read_run(&router, &transform, run_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: run {index} must be read back, got {status} with {read}"
            );
            assert_eq!(
                read["TaskRunId"].as_str(),
                Some(run_id.as_str()),
                "case {case:?}: the read must report the run it was asked for, got {read}"
            );
            assert_eq!(
                read["TransformId"].as_str(),
                Some(transform.as_str()),
                "case {case:?}: run {index} must name its parent transform, got {read}"
            );
            // The whole stored run: the properties its own start described, and the
            // members only a run has.
            assert_eq!(
                read["Properties"]["TaskType"].as_str(),
                Some(ML_TASK_TYPE_LABELING_SET_GENERATION),
                "case {case:?}: run {index} must report the type of task it is, got {read}"
            );
            assert_eq!(
                read["Properties"]["LabelingSetGenerationTaskRunProperties"]["OutputS3Path"]
                    .as_str(),
                Some(format!("s3://labels/read-{index}/").as_str()),
                "case {case:?}: run {index} must report the path its start named, got {read}"
            );
            assert_eq!(
                read["Status"].as_str(),
                Some(ML_TASK_RUN_STATUS_STARTING),
                "case {case:?}: run {index} must stand in the status its start recorded, got \
                 {read}"
            );
            assert!(
                read["StartedOn"].as_f64().unwrap_or_default() > 0.0,
                "case {case:?}: run {index} must report the instant it started, got {read}"
            );
            assert_eq!(
                read["ExecutionTime"].as_i64(),
                Some(0),
                "case {case:?}: run {index} has consumed no whole second yet, got {read}"
            );
            assert!(
                read.get("CompletedOn").is_none(),
                "case {case:?}: run {index} has not finished, so it reports no CompletedOn, got \
                 {read}"
            );
            // One run, one projection: the single read and the listing of the same run
            // report it identically.
            assert_eq!(
                read,
                listed_run(&listing, run_id),
                "case {case:?}: run {index} must read back as the listing reports it"
            );
        }

        // Reporting only: the reads above wrote nothing a later caller can see.
        assert_eq!(
            listed_runs(&router, &transform).await,
            listing,
            "case {case:?}: the run history must be unchanged by reading it"
        );
    }
}

#[tokio::test]
async fn the_status_a_read_reports_is_the_one_the_clock_has_reached() {
    // (case, the instant the owed transition falls due, the status the read must report,
    //  whether the run has finished by then)
    let rows: &[StatusRow<'_>] = &[
        ("the owed transition is still ahead", far_future, ML_TASK_RUN_STATUS_STARTING, false),
        ("the owed transition is already due", already_past, ML_TASK_RUN_STATUS_TIMEOUT, true),
    ];

    for (case, due, expected_status, finished) in rows {
        let (router, engine) = emulator_with_engine();
        let transform = given_transform(&router, "settling", Value::Null).await;
        let run_id = start_labeling_run(&router, &transform, "s3://labels/settling/").await;
        set_due_instant(&engine, due());

        let (status, read) = read_run(&router, &transform, &run_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the run must be read back, got {status} with {read}"
        );
        assert_eq!(
            read["Status"].as_str(),
            Some(*expected_status),
            "case {case:?}: expected {expected_status}, got {read}"
        );
        if *finished {
            assert!(
                read["CompletedOn"].as_f64().unwrap_or_default() > 0.0,
                "case {case:?}: a finished run must report when it finished, got {read}"
            );
            assert_eq!(
                read["ErrorString"].as_str(),
                Some(ML_TASK_RUN_TIMED_OUT_MESSAGE),
                "case {case:?}: a terminated run must report why it ended, got {read}"
            );
        } else {
            assert!(
                read.get("CompletedOn").is_none() && read.get("ErrorString").is_none(),
                "case {case:?}: a run in flight has neither finished nor been terminated, got \
                 {read}"
            );
        }

        // Two reads of one run agree: the transition was owed by the clock, so it is not
        // the reader that moved it.
        let (_, again) = read_run(&router, &transform, &run_id).await;
        assert_eq!(
            again, read,
            "case {case:?}: a second read must report the same run"
        );
    }
}

#[tokio::test]
async fn a_transform_this_account_does_not_hold_is_refused() {
    let router = emulator();
    let stored = given_transform(&router, "addressed", Value::Null).await;
    let run = start_labeling_run(&router, &stored, "s3://labels/addressed/").await;

    // A run is held ON its transform, so an id addressing no transform addresses no run.
    // (case, the TransformId the read addresses, the TaskRunId it asks for)
    let rows: &[MissRow<'_>] = &[
        (
            "an id nothing has minted",
            "0123456789abcdef0123456789abcdef".to_string(),
            run.clone(),
        ),
        (
            // "Names are not guaranteed unique and can be changed at any time"
            // (`MLTransform$Name`), so a name addresses no transform.
            "the stored transform's name used as an id",
            "addressed".to_string(),
            run.clone(),
        ),
        (
            "the stored id in upper case",
            stored.to_uppercase(),
            run.clone(),
        ),
        (
            "the run's own id used as the transform id",
            run.clone(),
            run.clone(),
        ),
        (
            "an id at the published maximum length",
            "a".repeat(HASH_STRING_MAX),
            run.clone(),
        ),
    ];

    for (case, transform_id, task_run_id) in rows {
        let (status, body) = read_run(&router, transform_id, task_run_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled EntityNotFoundException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        // The crate's one wording for a transform this account holds none of, whose
        // variable parts are the account and the id the request itself sent.
        assert_eq!(
            error_message(&body),
            format!("Cannot find MLTransform in account {ACCOUNT_ID} with handle {transform_id}"),
            "case {case:?}: expected the refusal to name the requested transform, got {body}"
        );
    }
}

#[tokio::test]
async fn a_task_run_the_addressed_transform_does_not_hold_is_refused() {
    let router = emulator();
    let stored = given_transform(&router, "holder", Value::Null).await;
    let run = start_labeling_run(&router, &stored, "s3://labels/holder/").await;
    // A second transform with a run of its own, so a run of ONE transform is unreachable
    // through another: a run's address is the pair, not the run id alone.
    let other = given_transform(&router, "other-holder", Value::Null).await;
    let other_run = start_labeling_run(&router, &other, "s3://labels/other/").await;

    // (case, the TransformId the read addresses, the TaskRunId it asks for)
    let rows: &[MissRow<'_>] = &[
        (
            "an id nothing has minted",
            stored.clone(),
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        (
            "the stored run's id in upper case",
            stored.clone(),
            run.to_uppercase(),
        ),
        (
            // A minted `TaskRunId` is lowercase hexadecimal, so the character put in
            // place of the last one is never the character it replaces, whatever the
            // mint answered.
            "the stored run's id with its last character replaced",
            stored.clone(),
            replace_last_character(&run, 'x'),
        ),
        (
            "the transform's own id used as a run id",
            stored.clone(),
            stored.clone(),
        ),
        (
            "another transform's run",
            stored.clone(),
            other_run.clone(),
        ),
        (
            "an id at the published maximum length",
            stored.clone(),
            "a".repeat(HASH_STRING_MAX),
        ),
        (
            "an id at the published minimum length",
            stored.clone(),
            "a".repeat(HASH_STRING_MIN),
        ),
    ];

    for (case, transform_id, task_run_id) in rows {
        let (status, body) = read_run(&router, transform_id, task_run_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled EntityNotFoundException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("Cannot find MLTaskRun in account {ACCOUNT_ID} with handle {task_run_id}"),
            "case {case:?}: expected the refusal to name the requested run, got {body}"
        );
    }

    // Each transform still holds its own run, and only its own.
    for (transform_id, expected) in [(&stored, &run), (&other, &other_run)] {
        let (status, read) = read_run(&router, transform_id, expected).await;
        assert_eq!(
            status, 200,
            "case {:?}: the transform's own run must still read back, got {status} with {read}",
            "after the refusals"
        );
        assert_eq!(
            read["TransformId"].as_str(),
            Some(transform_id.as_str()),
            "case {:?}: the run must name the transform it was started of, got {read}",
            "after the refusals"
        );
    }
}

#[tokio::test]
async fn an_address_outside_the_modelled_constraints_is_refused_at_the_boundary() {
    let router = emulator();
    let stored = given_transform(&router, "bounded", Value::Null).await;
    let run = start_labeling_run(&router, &stored, "s3://labels/bounded/").await;

    // (case, the request body)
    let rows: &[(&str, Value)] = &[
        (
            "no TransformId at all",
            json!({ "TaskRunId": run }),
        ),
        (
            "no TaskRunId at all",
            json!({ "TransformId": stored }),
        ),
        (
            "a TransformId below the published minimum length",
            json!({ "TransformId": "", "TaskRunId": run }),
        ),
        (
            "a TaskRunId below the published minimum length",
            json!({ "TransformId": stored, "TaskRunId": "" }),
        ),
        (
            "a TransformId one past the published maximum length",
            json!({ "TransformId": "a".repeat(HASH_STRING_MAX + 1), "TaskRunId": run }),
        ),
        (
            "a TaskRunId one past the published maximum length",
            json!({ "TransformId": stored, "TaskRunId": "a".repeat(HASH_STRING_MAX + 1) }),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = call(&router, "GetMLTaskRun", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the modelled constraints is refused at the \
             boundary with 400, got {status} with {answered}"
        );
        assert!(
            error_code(&answered).ends_with("ValidationException"),
            "case {case:?}: expected the boundary's ValidationException, got {} with {}",
            error_code(&answered),
            error_message(&answered)
        );
    }
}

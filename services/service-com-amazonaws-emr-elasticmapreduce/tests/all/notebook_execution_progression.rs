// SPDX-License-Identifier: CC0-1.0

//! The status progression of a notebook execution, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, and reads every outcome back through `DescribeNotebookExecution` and
//! `ListNotebookExecutions`.
//!
//! THE RULE: a notebook execution advances through the statuses
//! `NotebookExecution$Status` names, in the order that documentation gives them —
//! "`START_PENDING` indicates that the cluster has received the execution request but
//! execution has not begun", "`STARTING` indicates that the execution is starting on the
//! cluster", "`RUNNING` indicates that the execution is being processed by the cluster",
//! then the phases the end it reaches — and never goes backwards. This emulator runs no
//! notebook, so an execution left alone ends in the modelled failure statuses ("`FAILING`
//! indicates that the execution is failing and will not finish successfully", "`FAILED`
//! indicates that the execution failed") carrying the reason that says the failure is the
//! emulator's own, and it reports no output notebook, because none was written. A caller
//! that asks it to stop takes it onto the stop progression instead: `STOP_PENDING`,
//! `STOPPING`, `STOPPED`.
//!
//! The rows span what the rule generalises over: the editor an execution belongs to, the
//! notebook it names and how (a relative path under an editor, an Amazon S3 location), the
//! output format and location it asks for, the parameters it is given, and whether it is
//! left alone or stopped.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, kept_alive_request, Emr};

/// How long a poll waits for an execution to reach a terminal status before it gives up.
const DEADLINE: Duration = Duration::from_secs(30);

/// How long a poll sleeps between reads: short enough that the phases of a progression
/// can be observed rather than jumped over.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The statuses of the progression an execution that is left alone walks, in order
/// (`NotebookExecution$Status`).
const PROGRESSION: [&str; 5] = [
    "START_PENDING",
    "STARTING",
    "RUNNING",
    "FAILING",
    "FAILED",
];

/// The statuses of the progression a stopped execution walks, in order.
const STOP_PROGRESSION: [&str; 3] = ["STOP_PENDING", "STOPPING", "STOPPED"];

/// The statuses an execution never leaves (`NotebookExecutionStatus`).
const TERMINAL: [&str; 3] = ["FINISHED", "FAILED", "STOPPED"];

/// One row: the `StartNotebookExecution` request, and whether the caller stops it.
struct Case {
    label: &'static str,
    request: Value,
    stopped: bool,
}

fn cases(engine_id: &str) -> Vec<Case> {
    vec![
        Case {
            label: "a notebook under an editor's relative path",
            request: json!({
                "EditorId": "e-FIRST",
                "RelativePath": "notebooks/demo.ipynb",
                "ExecutionEngine": { "Id": engine_id },
                "ServiceRole": "EMR_Notebooks_DefaultRole",
                "NotebookExecutionName": "relative-path",
            }),
            stopped: false,
        },
        Case {
            label: "a notebook named by its Amazon S3 location, with parameters",
            request: json!({
                "ExecutionEngine": { "Id": engine_id, "Type": "EMR" },
                "ServiceRole": "EMR_Notebooks_DefaultRole",
                "NotebookExecutionName": "s3-location",
                "NotebookS3Location": { "Bucket": "notebooks", "Key": "pipeline.ipynb" },
                "NotebookParams": "{\"threshold\":7}",
                "EnvironmentVariables": { "LOG_LEVEL": "DEBUG" },
            }),
            stopped: false,
        },
        Case {
            label: "a notebook writing its output as HTML",
            request: json!({
                "EditorId": "e-SECOND",
                "ExecutionEngine": { "Id": engine_id },
                "ServiceRole": "EMR_Notebooks_DefaultRole",
                "NotebookExecutionName": "html-output",
                "NotebookS3Location": { "Bucket": "notebooks", "Key": "report.ipynb" },
                "OutputNotebookS3Location": { "Bucket": "notebook-output", "Key": "report.html" },
                "OutputNotebookFormat": "HTML",
            }),
            stopped: false,
        },
        Case {
            label: "an execution the caller stops",
            request: json!({
                "EditorId": "e-THIRD",
                "RelativePath": "notebooks/long.ipynb",
                "ExecutionEngine": { "Id": engine_id },
                "ServiceRole": "EMR_Notebooks_DefaultRole",
                "NotebookExecutionName": "stopped",
            }),
            stopped: true,
        },
    ]
}

/// The execution `DescribeNotebookExecution` reports.
async fn described(emr: &Emr, id: &str) -> Value {
    emr.ok(
        "DescribeNotebookExecution",
        json!({ "NotebookExecutionId": id }),
    )
    .await["NotebookExecution"]
        .clone()
}

/// Poll `DescribeNotebookExecution` until the execution reaches a terminal status,
/// answering every status seen on the way (in the order they were seen, without repeats)
/// and the execution as it was last read.
async fn walked(case: &str, emr: &Emr, id: &str) -> (Vec<String>, Value) {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let execution = described(emr, id).await;
        let status = at(&execution, &["Status"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if TERMINAL.contains(&status.as_str()) {
            return (seen, execution);
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {case:?}: expected a terminal status, saw {seen:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// That `seen` is a walk of `progression`: every status belongs to it, and the walk never
/// goes backwards.
fn assert_walks(case: &str, seen: &[String], progression: &[&str]) {
    let mut last = 0usize;
    for status in seen {
        let position = progression
            .iter()
            .position(|step| step == status)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {status:?} is not one of {progression:?}, saw {seen:?}")
            });
        assert!(
            position >= last,
            "case {case:?}: the walk went backwards to {status:?}, saw {seen:?}"
        );
        last = position;
    }
    assert_eq!(
        seen.last().map(String::as_str),
        progression.last().copied(),
        "case {case:?}: expected the progression to end at {:?}, saw {seen:?}",
        progression.last()
    );
}

#[tokio::test]
async fn an_execution_walks_the_modelled_progression_to_its_terminal_status() {
    let emr = Emr::new();
    let engine_id = emr.run_job_flow(kept_alive_request("notebook-host")).await;

    for case in cases(&engine_id) {
        let label = case.label;
        let out = emr.ok("StartNotebookExecution", case.request.clone()).await;
        let id = out["NotebookExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no NotebookExecutionId in {out}"))
            .to_string();

        // The request has just been received, so the execution has not begun.
        let recorded = described(&emr, &id).await;
        assert_eq!(
            at(&recorded, &["Status"]).as_str(),
            Some(PROGRESSION[0]),
            "case {label:?}: a just-recorded execution has not begun, got {recorded}"
        );
        assert!(
            at(&recorded, &["EndTime"]).is_null(),
            "case {label:?}: an execution that has not ended reports no EndTime, got {recorded}"
        );
        let started_at = at(&recorded, &["StartTime"])
            .as_f64()
            .unwrap_or_else(|| panic!("case {label:?}: no StartTime in {recorded}"));

        let progression: &[&str] = if case.stopped {
            emr.ok(
                "StopNotebookExecution",
                json!({ "NotebookExecutionId": &id }),
            )
            .await;
            &STOP_PROGRESSION
        } else {
            &PROGRESSION
        };
        let (seen, ended) = walked(label, &emr, &id).await;
        if case.stopped {
            // The stop abandons the run, so only the stop progression is walked from
            // here; the status the execution was in when the stop arrived belongs to the
            // other one.
            let after_stop: Vec<String> = seen
                .iter()
                .filter(|status| STOP_PROGRESSION.contains(&status.as_str()))
                .cloned()
                .collect();
            assert!(
                !after_stop.is_empty(),
                "case {label:?}: a stopped execution walks the stop progression, saw {seen:?}"
            );
            assert_walks(label, &after_stop, progression);
        } else {
            assert_walks(label, &seen, progression);
        }

        let ended_at = at(&ended, &["EndTime"])
            .as_f64()
            .unwrap_or_else(|| panic!("case {label:?}: no EndTime in {ended}"));
        assert!(
            ended_at >= started_at,
            "case {label:?}: expected an EndTime at or after the StartTime, got {ended_at} \
             against {started_at}"
        );
        // Whichever progression it walked, the execution says why it ended — and it
        // reports no output notebook, because no cell of it ran.
        assert!(
            at(&ended, &["LastStateChangeReason"])
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {label:?}: an ended execution says why, got {ended}"
        );
        assert!(
            at(&ended, &["OutputNotebookURI"]).is_null(),
            "case {label:?}: no output notebook was written, so none is reported, got {ended}"
        );
        // A read reports; it does not move the execution on.
        let again = described(&emr, &id).await;
        assert_eq!(
            again, ended,
            "case {label:?}: a second read reports the same execution, got {again}"
        );

        // The listing's status filter selects the execution under the status it reached,
        // and under no other.
        let terminal = at(&ended, &["Status"]).as_str().unwrap_or_default();
        for status in TERMINAL {
            let page = emr
                .ok("ListNotebookExecutions", json!({ "Status": status }))
                .await;
            let selected: Vec<&str> = at(&page, &["NotebookExecutions"])
                .as_array()
                .map(|executions| {
                    executions
                        .iter()
                        .filter_map(|execution| execution["NotebookExecutionId"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                selected.contains(&id.as_str()),
                status == terminal,
                "case {label:?}: the {status} filter must select it only under {terminal}, \
                 got {page}"
            );
        }
    }
}

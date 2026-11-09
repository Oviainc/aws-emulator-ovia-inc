// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN the job a `.sync` task started is stopped once a SIBLING
//! `Parallel` branch fails, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through
//! the production `MultiServiceRouter`.
//!
//! The rule under test: a job a run-a-job (`.sync`) task started is stopped by the
//! failure of the state around it, and that failure happens when the clock reaches it
//! — the total of the `Wait` states the failing branch passes, which the walk computes
//! and sleeps none of. Until then the task is in flight and the job is a resource of
//! its own service: for the nested-execution integration that means a child execution
//! readable through this API in status `RUNNING`, exactly as a caller polling it would
//! find it. Only when the sibling's failure comes due is the child aborted, which is
//! what a caller reading it then sees — with no `WaitStateExited`, because the child's
//! own wait never finished.
//!
//! Rows vary along the delay the failing branch declares, which is the dimension the
//! rule generalises over: it is what decides how long the job is in flight, and a
//! delay of zero (the job stopped as soon as the run is planned) is the case
//! `parallel_run_a_job` already drives.
//!
//! The integration driven here is the nested execution
//! (`arn:aws:states:::states:startExecution.sync`), whose `StartExecution`,
//! `DescribeExecution` and `StopExecution` calls this service serves itself, so every
//! assertion is about work that really ran.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds and the history reader is `execution_history`'s;
//! `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    EVENT_EXECUTION_ABORTED, EVENT_EXECUTION_FAILED, EVENT_FAIL_STATE_ENTERED,
    EVENT_TASK_SUBMITTED, EVENT_TASK_SUCCEEDED,
};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{create_machine, string, Emulator};

/// How long a test waits for a status it expects before giving up: well past the
/// longest delay a row declares and the settle the outcome is owed after.
const PATIENCE: Duration = Duration::from_secs(20);

/// How much earlier than the sibling's own delay the child may leave `RUNNING`.
///
/// The delay is measured from the instant the submission was READ rather than from
/// the start of the execution, so the slack covers that reading and the poll interval
/// below — nothing more: the point of the assertion is that the child outlives the
/// delay rather than being torn down with the call that planned the run.
const SLACK: Duration = Duration::from_millis(250);

/// How often a test looks at a status it is waiting for.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// The child machine: one `Wait` far longer than any row needs, so the child is
/// `RUNNING` until something stops it and its own wait never finishes.
const CHILD_WAIT_SECONDS: u64 = 60;

/// The error and cause the failing branch declares, which the parent must end with.
const SIBLING_ERROR: &str = "SiblingBoom";
const SIBLING_CAUSE: &str = "the sibling branch failed on purpose";

/// `DescribeExecution`, as the API answers it.
async fn described(emulator: &Emulator, execution_arn: &str) -> Value {
    emulator
        .ok(
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await
}

/// The status `DescribeExecution` reports for `execution_arn`.
async fn status_of(emulator: &Emulator, execution_arn: &str) -> String {
    described(emulator, execution_arn)
        .await
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The child execution the parent's `TaskSubmitted` event names, waiting for that
/// event: the job identifier the invoked service answered, read off the event rather
/// than guessed at.
async fn submitted_child(emulator: &Emulator, case: &str, execution_arn: &str) -> String {
    let began = Instant::now();
    loop {
        let submitted = history(emulator, execution_arn, json!({}))
            .await
            .into_iter()
            .find(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_SUBMITTED));
        if let Some(submitted) = submitted {
            let details = submitted
                .get("taskSubmittedEventDetails")
                .unwrap_or_else(|| panic!("case {case:?}: expected details in {submitted:?}"));
            let output = string(details, "output");
            let reported: Value = serde_json::from_str(&output).unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON output, got {error} in {output:?}")
            });
            return string(&reported, "ExecutionArn");
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected a {EVENT_TASK_SUBMITTED} event within {PATIENCE:?}"
        );
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// How many events of type `kind` an execution's history holds.
fn count(events: &[Value], kind: &str) -> usize {
    events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(kind))
        .count()
}

/// The in-flight child of a `.sync` task is RUNNING for as long as the sibling branch
/// that fails the `Parallel` takes to reach its `Fail` state, and is aborted then.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_in_flight_child_runs_until_the_sibling_branch_fails() {
    // (case, the seconds the failing branch waits before it fails)
    let rows: [(&str, u64); 2] = [
        ("a sibling that fails after a second", 1),
        ("a sibling that fails after three seconds", 3),
    ];

    let emulator = Emulator::new();
    for (index, (case, delay_seconds)) in rows.into_iter().enumerate() {
        let delay = Duration::from_secs(delay_seconds);
        let child_definition = json!({
            "StartAt": "Hold",
            "States": {
                "Hold": { "Type": "Wait", "Seconds": CHILD_WAIT_SECONDS, "End": true }
            }
        })
        .to_string();
        let child_arn =
            create_machine(&emulator, &format!("inflight-child-{index}"), &child_definition).await;
        let parent_definition = json!({
            "StartAt": "Fan",
            "States": {
                "Fan": {
                    "Type": "Parallel",
                    "Branches": [
                        {
                            "StartAt": "CallChild",
                            "States": {
                                "CallChild": {
                                    "Type": "Task",
                                    "Resource": "arn:aws:states:::states:startExecution.sync",
                                    "Parameters": {
                                        "stateMachineArn": child_arn,
                                        "input": "{\"from\":\"the branch\"}",
                                    },
                                    "End": true,
                                }
                            }
                        },
                        {
                            "StartAt": "Delay",
                            "States": {
                                "Delay": {
                                    "Type": "Wait",
                                    "Seconds": delay_seconds,
                                    "Next": "Boom",
                                },
                                "Boom": {
                                    "Type": "Fail",
                                    "Error": SIBLING_ERROR,
                                    "Cause": SIBLING_CAUSE,
                                }
                            }
                        },
                    ],
                    "End": true,
                }
            }
        })
        .to_string();
        let parent_arn =
            create_machine(&emulator, &format!("inflight-parent-{index}"), &parent_definition)
                .await;
        let started = emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": parent_arn, "name": "run" }),
            )
            .await;
        let parent = string(&started, "executionArn");

        // The job really started, and the child it started is in flight: the FIRST
        // look at it, right after the submission the parent recorded, finds it
        // RUNNING — the sibling's failure has not happened yet.
        let child = submitted_child(&emulator, case, &parent).await;
        let submitted_at = Instant::now();
        let first = described(&emulator, &child).await;
        assert_eq!(
            first.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the submitted child to be RUNNING, got {first:?}"
        );
        assert_eq!(
            first.get("stateMachineArn").and_then(Value::as_str),
            Some(child_arn.as_str()),
            "case {case:?}: expected the child to run {child_arn}, got {first:?}"
        );
        assert!(
            first.get("stopDate").is_none_or(Value::is_null),
            "case {case:?}: a running child has not stopped, got {first:?}"
        );

        // And it stays RUNNING until the sibling's own delay has passed.
        let began = Instant::now();
        let ran_for = loop {
            let status = status_of(&emulator, &child).await;
            if status != "RUNNING" {
                let ran_for = submitted_at.elapsed();
                assert_eq!(
                    status, "ABORTED",
                    "case {case:?}: expected the sibling's failure to abort the child, got \
                     {status:?}"
                );
                break ran_for;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "case {case:?}: expected the child to be aborted within {PATIENCE:?}"
            );
            ::tokio::time::sleep(LOOK_AGAIN).await;
        };
        assert!(
            ran_for.saturating_add(SLACK) >= delay,
            "case {case:?}: expected the child to be in flight for the {delay:?} the sibling \
             waits, got {ran_for:?}"
        );
        assert!(
            ran_for < Duration::from_secs(CHILD_WAIT_SECONDS),
            "case {case:?}: expected the child to be stopped before its own \
             {CHILD_WAIT_SECONDS}s wait, got {ran_for:?}"
        );

        // The parent is the one that failed, with the branch's own error and cause,
        // and it recorded no success for the task whose job was abandoned.
        let settled = described(&emulator, &parent).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the sibling's failure to end the parent, got {settled:?}"
        );
        assert_eq!(
            settled.get("error").and_then(Value::as_str),
            Some(SIBLING_ERROR),
            "case {case:?}: expected the sibling's own error, got {settled:?}"
        );
        assert_eq!(
            settled.get("cause").and_then(Value::as_str),
            Some(SIBLING_CAUSE),
            "case {case:?}: expected the sibling's own cause, got {settled:?}"
        );
        let parent_events = history(&emulator, &parent, json!({})).await;
        assert_eq!(
            count(&parent_events, EVENT_FAIL_STATE_ENTERED),
            1,
            "case {case:?}: expected exactly one {EVENT_FAIL_STATE_ENTERED}, got {parent_events:?}"
        );
        assert_eq!(
            count(&parent_events, EVENT_EXECUTION_FAILED),
            1,
            "case {case:?}: expected exactly one {EVENT_EXECUTION_FAILED}, got {parent_events:?}"
        );
        assert_eq!(
            count(&parent_events, EVENT_TASK_SUCCEEDED),
            0,
            "case {case:?}: the abandoned task succeeded at nothing, got {parent_events:?}"
        );

        // The child's own history ends where it was stopped: its wait never finished.
        let child_events = history(&emulator, &child, json!({})).await;
        let last = child_events
            .last()
            .and_then(|event| event.get("type"))
            .and_then(Value::as_str);
        assert_eq!(
            last,
            Some(EVENT_EXECUTION_ABORTED),
            "case {case:?}: expected the child's history to end on {EVENT_EXECUTION_ABORTED}, \
             got {child_events:?}"
        );
        assert_eq!(
            count(&child_events, "WaitStateExited"),
            0,
            "case {case:?}: the child's own wait never finished, got {child_events:?}"
        );
    }
}

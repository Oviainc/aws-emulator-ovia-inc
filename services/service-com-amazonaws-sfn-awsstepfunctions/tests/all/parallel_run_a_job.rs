// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the run-a-job (`.sync`) integration pattern asked for by a
//! `Task` state INSIDE a `Parallel` branch, driven over this service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`.
//!
//! The rule under test: a `Task` state whose `Resource` asks for the run-a-job pattern
//! starts the integration's job and the task is PARKED at it until the job reaches a
//! terminal state, WHEREVER in the definition that state is — at the top level or
//! inside a `Parallel` branch. So the branch's task records `TaskSubmitted` carrying
//! the job the call started, the execution stays `RUNNING` while the job runs, and the
//! branch — and the `Parallel` around it — carries on with what the job ended as.
//!
//! Two consequences of that rule are asserted beside it:
//!
//! * a branch whose job is still running does not finish the `Parallel`, so a SIBLING
//!   branch that fails fails the whole state, and the job that was in flight is
//!   STOPPED — it is work of another service that nothing will wait for any more;
//! * a `Map` ITERATION still refuses such a task, because a Map Run counts its items
//!   as they finish and an iteration parked at a job has finished nothing: the task is
//!   not run, and no job is started for it.
//!
//! The integration driven here is the nested execution
//! (`arn:aws:states:::states:startExecution.sync`), whose three calls — `StartExecution`,
//! `DescribeExecution`, `StopExecution` — this service serves itself, so every
//! assertion is about work that really ran: the child execution is read back through
//! this API and its own status is what resolves the branch's task.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds and the history reader is `execution_history`'s;
//! `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{EVENT_TASK_SUBMITTED, EVENT_TASK_FAILED};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{create_machine, string, Emulator};

/// How long a test waits for an execution to leave `RUNNING` before giving up.
///
/// A parked task is resolved by polling the job, so the parent's own outcome arrives a
/// few poll intervals and one settle delay after the child's does: this is well past
/// that and far short of the ceiling on a parked job.
const PATIENCE: Duration = Duration::from_secs(20);

/// A child definition that finishes at once with a static result.
const CHILD_SUCCEEDS: &str =
    r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","Result":{"ok":true},"End":true}}}"#;

/// A child definition that keeps running: its one `Wait` state is owed an hour, so the
/// child is `RUNNING` for as long as a test needs it to be.
const CHILD_RUNS_ON: &str = r#"{"StartAt":"Hold","States":{"Hold":{"Type":"Wait",
    "Seconds":3600,"Next":"Done"},"Done":{"Type":"Pass","End":true}}}"#;

/// The `Task` state that starts `child_arn` through the run-a-job pattern.
///
/// The `Parameters` document IS the request of the call the integration makes
/// (`StartExecution`: `stateMachineArn`, `input`), so its members are the ones that
/// operation's own model declares.
fn sync_task(child_arn: &str) -> Value {
    json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::states:startExecution.sync",
        "Parameters": {
            "stateMachineArn": child_arn,
            "input": "{\"from\":\"the branch\"}",
        },
        "End": true,
    })
}

/// `DescribeExecution`, as the API answers it.
async fn described(emulator: &Emulator, execution_arn: &str) -> Value {
    emulator
        .ok(
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await
}

/// The execution `execution_arn` names once it has left `RUNNING`, or a panic naming
/// the case when it never does.
async fn settled(emulator: &Emulator, case: &str, execution_arn: &str) -> Value {
    let began = Instant::now();
    loop {
        let described = described(emulator, execution_arn).await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected {execution_arn} to reach a terminal status within \
             {PATIENCE:?}, got {described:?}"
        );
        ::tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The child execution a `TaskSubmitted` event of this execution names, waiting for
/// that event to appear: the job identifier the invoked service answered, read off the
/// event rather than guessed at.
async fn submitted_job(emulator: &Emulator, case: &str, execution_arn: &str) -> String {
    let began = Instant::now();
    loop {
        let submitted = history(emulator, execution_arn, json!({}))
            .await
            .into_iter()
            .find(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_SUBMITTED));
        if let Some(submitted) = submitted {
            let details = submitted.get("taskSubmittedEventDetails").unwrap_or_else(|| {
                panic!("case {case:?}: expected taskSubmittedEventDetails in {submitted:?}")
            });
            let output = string(details, "output");
            let reported: Value = serde_json::from_str(&output).unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON output, got {error} in {output:?}")
            });
            // The submission carries "the response from a resource when a task has
            // started", which for this integration is `StartExecution`'s own output,
            // reported in the integration's own convention.
            return string(&reported, "ExecutionArn");
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected a {EVENT_TASK_SUBMITTED} event within {PATIENCE:?}"
        );
        ::tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// One row of the parked-branch table: the case and the branches the `Parallel`
/// declares beside the one that parks.
struct BranchRow {
    case: &'static str,
    /// The states of the SIBLING branch, which is what decides whether the `Parallel`
    /// can finish once the parked branch does.
    sibling: Value,
}

/// A `.sync` task inside a `Parallel` branch starts its job, parks the execution at
/// it, and the execution carries on with what that job ended as.
///
/// Rows vary along what the SIBLING branch does — the dimension the rule generalises
/// over, since the `Parallel`'s own outcome is a function of every branch. A sibling
/// that FAILS while the job is in flight is the next test, because what it asserts of
/// the job is different.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_a_job_task_in_a_parallel_branch_parks_at_its_job() {
    let rows = [
        BranchRow {
            case: "a sibling that succeeds at once",
            sibling: json!({
                "StartAt": "Quick",
                "States": { "Quick": { "Type": "Pass", "Result": { "sibling": "done" }, "End": true } }
            }),
        },
        BranchRow {
            case: "a sibling of several states",
            sibling: json!({
                "StartAt": "First",
                "States": {
                    "First": { "Type": "Pass", "Result": { "step": 1 }, "Next": "Second" },
                    "Second": { "Type": "Pass", "Result": { "sibling": "done" }, "End": true }
                }
            }),
        },
        BranchRow {
            case: "a sibling that passes a Wait of its own",
            sibling: json!({
                "StartAt": "Hold",
                "States": {
                    "Hold": { "Type": "Wait", "Seconds": 0, "Next": "Then" },
                    "Then": { "Type": "Pass", "Result": { "sibling": "done" }, "End": true }
                }
            }),
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // The child finishes at once, so what the branch's task is parked at really
        // ends and the parent really carries on from it.
        let child_arn = create_machine(&emulator, &format!("sync-child-{index}"), CHILD_SUCCEEDS)
            .await;
        let definition = json!({
            "StartAt": "Fan",
            "States": {
                "Fan": {
                    "Type": "Parallel",
                    "Branches": [
                        { "StartAt": "CallChild", "States": { "CallChild": sync_task(&child_arn) } },
                        row.sibling.clone(),
                    ],
                    "End": true,
                }
            }
        })
        .to_string();
        let parent_arn =
            create_machine(&emulator, &format!("sync-parent-{index}"), &definition).await;
        let started = emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": parent_arn, "name": format!("run-{index}") }),
            )
            .await;
        let execution_arn = string(&started, "executionArn");

        // The job really started: the child execution the submission names is
        // readable through this API, carrying the document the state built.
        let job = submitted_job(&emulator, case, &execution_arn).await;
        let child = described(&emulator, &job).await;
        assert_eq!(
            child.get("stateMachineArn").and_then(Value::as_str),
            Some(child_arn.as_str()),
            "case {case:?}: expected the job to run {child_arn}, got {child:?}"
        );
        assert_eq!(
            child.get("input").and_then(Value::as_str),
            Some("{\"from\":\"the branch\"}"),
            "case {case:?}: expected the job to carry the document the state built, got {child:?}"
        );

        let settled = settled(&emulator, case, &execution_arn).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the parked branch to resolve, got {settled:?}"
        );
        // The `Parallel` puts out one result per branch, in branch order: the job's
        // own report for the branch that parked, and the sibling's own result.
        let output: Value = serde_json::from_str(&string(&settled, "output"))
            .unwrap_or_else(|error| panic!("case {case:?}: expected a JSON output, got {error}"));
        let branches = output
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an array of branch results, got {output}"));
        assert_eq!(
            branches.len(),
            2,
            "case {case:?}: expected one result per branch, got {output}"
        );
        assert_eq!(
            branches[0].get("Status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the job's own report as the branch's result, got {output}"
        );
        assert_eq!(
            branches[1].get("sibling").and_then(Value::as_str),
            Some("done"),
            "case {case:?}: expected the sibling's own result beside it, got {output}"
        );
    }
}

/// A branch that FAILS while a sibling is parked at a job fails the `Parallel`, and the
/// job that was in flight is stopped: it is another service's work that nothing will
/// wait for any more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sibling_branch_that_fails_stops_the_job_in_flight() {
    let emulator = Emulator::new();
    // The child holds for an hour, so it is still RUNNING when the sibling fails.
    let child_arn = create_machine(&emulator, "aborted-child", CHILD_RUNS_ON).await;
    let definition = json!({
        "StartAt": "Fan",
        "States": {
            "Fan": {
                "Type": "Parallel",
                "Branches": [
                    { "StartAt": "CallChild", "States": { "CallChild": sync_task(&child_arn) } },
                    {
                        "StartAt": "Boom",
                        "States": {
                            "Boom": {
                                "Type": "Fail",
                                "Error": "SiblingBoom",
                                "Cause": "the sibling branch failed",
                            }
                        }
                    },
                ],
                "End": true,
            }
        }
    })
    .to_string();
    let parent_arn = create_machine(&emulator, "aborted-parent", &definition).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": parent_arn, "name": "run" }),
        )
        .await;
    let execution_arn = string(&started, "executionArn");

    let case = "a sibling that fails while the job runs";
    // The job was STARTED before the sibling's failure reached the state: the child
    // execution exists and is describable.
    let job = submitted_job(&emulator, case, &execution_arn).await;

    let settled = settled(&emulator, case, &execution_arn).await;
    assert_eq!(
        settled.get("status").and_then(Value::as_str),
        Some("FAILED"),
        "case {case:?}: expected the Parallel's failure to end the execution, got {settled:?}"
    );
    assert_eq!(
        settled.get("error").and_then(Value::as_str),
        Some("SiblingBoom"),
        "case {case:?}: expected the sibling's own error, got {settled:?}"
    );

    // The job in flight was STOPPED: the child reports the status its own
    // `StopExecution` left it in rather than still running.
    let began = Instant::now();
    loop {
        let child = described(&emulator, &job).await;
        let status = child.get("status").and_then(Value::as_str);
        if status == Some("ABORTED") {
            break;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected the job in flight to be stopped within {PATIENCE:?}, got \
             {child:?}"
        );
        ::tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A `Map` ITERATION still refuses a run-a-job task, and no job is started for it: a
/// Map Run counts its items as they finish and an iteration parked at a job has
/// finished none, so nothing is run rather than run as though the job had ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_a_job_task_in_a_map_iteration_starts_no_job() {
    let emulator = Emulator::new();
    let child_arn = create_machine(&emulator, "unreached-child", CHILD_SUCCEEDS).await;
    let definition = json!({
        "StartAt": "Each",
        "States": {
            "Each": {
                "Type": "Map",
                "ItemProcessor": {
                    "StartAt": "CallChild",
                    "States": { "CallChild": sync_task(&child_arn) }
                },
                "End": true,
            }
        }
    })
    .to_string();
    let parent_arn = create_machine(&emulator, "unreached-parent", &definition).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({
                "stateMachineArn": parent_arn,
                "name": "run",
                "input": "[1]",
            }),
        )
        .await;
    let execution_arn = string(&started, "executionArn");

    let case = "a run-a-job task inside a Map iteration";
    let settled = settled(&emulator, case, &execution_arn).await;
    assert_eq!(
        settled.get("status").and_then(Value::as_str),
        Some("FAILED"),
        "case {case:?}: expected the iteration's refusal to fail the execution, got {settled:?}"
    );
    assert_eq!(
        settled.get("error").and_then(Value::as_str),
        Some("States.Runtime"),
        "case {case:?}: expected the interpreter's own error name, got {settled:?}"
    );
    // Nothing was started for it: the child state machine has no execution at all.
    let listed = emulator
        .ok(
            "ListExecutions",
            &json!({ "stateMachineArn": child_arn }),
        )
        .await;
    let executions = listed
        .get("executions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an executions list in {listed:?}"));
    assert!(
        executions.is_empty(),
        "case {case:?}: expected no child execution to have been started, got {executions:?}"
    );
    // And the task's failure is attributed to the task itself, as every other task
    // failure of this interpreter is.
    let failed = history(&emulator, &execution_arn, json!({}))
        .await
        .into_iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_FAILED));
    assert!(
        failed.is_some(),
        "case {case:?}: expected a {EVENT_TASK_FAILED} event in the history"
    );
}

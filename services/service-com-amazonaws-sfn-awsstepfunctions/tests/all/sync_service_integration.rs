// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the run-a-job (`.sync`) service-integration pattern, driven
//! over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test: a `Task` state whose `Resource` names a run-a-job
//! integration (`arn:aws:states:::<service>:<action>.sync`) is INVOKED — the
//! integration's own call is made with the document the state built — and the task is
//! then PARKED at the job that call started: the history records `TaskSubmitted`
//! carrying the response that names the job, `DescribeExecution` keeps reporting
//! `RUNNING`, and the task is resolved only when the invoked service reports the job
//! terminal, with `TaskSucceeded` carrying that service's own report of it or
//! `TaskFailed` carrying why it ended without doing its work. While the task is
//! parked, `StopExecution` stops the job too — through the stopping call the
//! integration names — and leaves the execution `ABORTED` with the caller's
//! `error`/`cause` and a history ending in `ExecutionAborted`.
//!
//! The integration driven here is the nested execution
//! (`arn:aws:states:::states:startExecution.sync`), whose three calls —
//! `StartExecution`, `DescribeExecution`, `StopExecution` — this service serves
//! itself, so every assertion is about work that really ran: the child execution is
//! read back through this API, and its own status is what resolves the parent's task.
//! The last test covers the whole catalogue, including the Athena query the recorded
//! case this suite's issue was filed about uses.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds and the history reader is `execution_history`'s;
//! `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    integration::{read, RUN_A_JOB},
    EVENT_EXECUTION_ABORTED, EVENT_TASK_FAILED, EVENT_TASK_SUBMITTED, EVENT_TASK_SUBMIT_FAILED,
    EVENT_TASK_SUCCEEDED,
};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{create_machine, string, Emulator, REGION};

/// The ASL error name a task whose own work failed reports
/// (`state::integration::TASK_FAILED_ERROR`).
const TASK_FAILED: &str = "States.TaskFailed";

/// How long a test waits for an execution to leave `RUNNING` before giving up.
///
/// A parked task is resolved by polling the job, so the parent's own outcome arrives
/// a few poll intervals and one settle delay after the child's does: this is well
/// past that and far short of the ceiling on a parked job.
const PATIENCE: Duration = Duration::from_secs(20);

/// A child definition that finishes at once with a static result.
const CHILD_SUCCEEDS: &str =
    r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","Result":{"ok":true},"End":true}}}"#;

/// A child definition that ends in its own failure.
const CHILD_FAILS: &str = r#"{"StartAt":"Boom","States":{"Boom":{"Type":"Fail",
    "Error":"ChildBoom","Cause":"the child state machine failed"}}}"#;

/// A child definition that keeps running: its one `Wait` state is owed an hour, so
/// the child is `RUNNING` for as long as a test needs it to be.
const CHILD_RUNS_ON: &str = r#"{"StartAt":"Hold","States":{"Hold":{"Type":"Wait",
    "Seconds":3600,"Next":"Done"},"Done":{"Type":"Pass","End":true}}}"#;

/// A parent definition whose one `Task` state starts `child_arn` through the
/// run-a-job pattern `suffix` names.
///
/// The `Parameters` document IS the request of the call the integration makes
/// (`StartExecution`: `stateMachineArn`, `input`), so its members are the ones that
/// operation's own model declares.
fn parent_definition(child_arn: &str, suffix: &str) -> String {
    json!({
        "StartAt": "RunChild",
        "States": {
            "RunChild": {
                "Type": "Task",
                "Resource": format!("arn:aws:states:::states:startExecution{suffix}"),
                "Parameters": {
                    "stateMachineArn": child_arn,
                    "input": "{\"from\":\"the parent\"}",
                },
                "End": true,
            }
        }
    })
    .to_string()
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
            "case {case:?}: expected {execution_arn} to reach a terminal status within {PATIENCE:?}, \
             got {described:?}"
        );
        ::tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The one event of `event_type` the history of `execution_arn` carries, or `None`
/// when it carries none.
async fn event(emulator: &Emulator, execution_arn: &str, event_type: &str) -> Option<Value> {
    history(emulator, execution_arn, json!({}))
        .await
        .into_iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
}

/// The child execution the parent's `TaskSubmitted` event names: the job identifier
/// the invoked service answered, read off the event rather than guessed at.
async fn submitted_job(emulator: &Emulator, case: &str, parent_arn: &str) -> String {
    let submitted = event(emulator, parent_arn, EVENT_TASK_SUBMITTED)
        .await
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {EVENT_TASK_SUBMITTED} event in the parent's history")
        });
    let details = submitted
        .get("taskSubmittedEventDetails")
        .unwrap_or_else(|| panic!("case {case:?}: expected taskSubmittedEventDetails in {submitted:?}"));
    let output = string(details, "output");
    let reported: Value = serde_json::from_str(&output)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a JSON output, got {error} in {output:?}"));
    // The submission carries "the response from a resource when a task has started",
    // which for this integration is `StartExecution`'s own output, reported in the
    // integration's own convention.
    string(&reported, "ExecutionArn")
}

/// One row of the resolution table: the case, the child's definition, the pattern
/// suffix the parent's `Resource` carries, and what the parent must end as.
type ResolutionRow = (&'static str, &'static str, &'static str, &'static str);

/// A run-a-job task starts its job, is parked at it, and is resolved by what that job
/// ENDED as — over a table that varies the job's outcome and the revision of the
/// pattern the `Resource` asks for.
///
/// Each row asserts the whole sequence the pattern is made of: the job really started
/// (the child execution the `TaskSubmitted` event names is readable through this API),
/// the parent was `RUNNING` while it ran, and the task's outcome is the job's own —
/// a job that did its work makes the task succeed with the invoked service's report of
/// it, and one that ended otherwise makes it fail with `States.TaskFailed` carrying
/// the reason that service gave.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_a_job_task_is_resolved_by_the_job_it_started() {
    let rows: [ResolutionRow; 3] = [
        (
            "a job that did its work",
            CHILD_SUCCEEDS,
            ".sync",
            "SUCCEEDED",
        ),
        (
            "the same job through the pattern's second revision",
            CHILD_SUCCEEDS,
            ".sync:2",
            "SUCCEEDED",
        ),
        (
            "a job that ended without doing its work",
            CHILD_FAILS,
            ".sync",
            "FAILED",
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, child_definition, suffix, expected)) in rows.into_iter().enumerate() {
        let child_arn = create_machine(&emulator, &format!("child-{index}"), child_definition).await;
        let parent_arn = create_machine(
            &emulator,
            &format!("parent-{index}"),
            &parent_definition(&child_arn, suffix),
        )
        .await;
        let started = emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": parent_arn, "name": format!("run-{index}") }),
            )
            .await;
        let execution_arn = string(&started, "executionArn");

        // The task is PARKED: the call that started the job has answered, the job is
        // running, and the execution has no outcome yet.
        let parked = described(&emulator, &execution_arn).await;
        assert_eq!(
            parked.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected RUNNING while the job runs, got {parked:?}"
        );

        // The job the parent started is a child execution of its own, readable
        // through this API under the ARN the submission reported.
        let job = submitted_job(&emulator, case, &execution_arn).await;
        let child = described(&emulator, &job).await;
        assert_eq!(
            child.get("stateMachineArn").and_then(Value::as_str),
            Some(child_arn.as_str()),
            "case {case:?}: expected the job to run {child_arn}, got {child:?}"
        );
        assert_eq!(
            child.get("input").and_then(Value::as_str),
            Some("{\"from\":\"the parent\"}"),
            "case {case:?}: expected the job to carry the document the state built, got {child:?}"
        );

        let settled = settled(&emulator, case, &execution_arn).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {expected}, got {settled:?}"
        );
        if expected == "FAILED" {
            assert_eq!(
                settled.get("error").and_then(Value::as_str),
                Some(TASK_FAILED),
                "case {case:?}: expected {TASK_FAILED}, got {settled:?}"
            );
            // The cause is the invoked service's own report of the job, as the JSON
            // text of the document a job that DID its work reports as the task's
            // result: for a nested execution that is the child's description, so the
            // error and the reason the child ended on are read out of it.
            let cause: Value = serde_json::from_str(
                settled.get("cause").and_then(Value::as_str).unwrap_or(""),
            )
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON cause, got {error} in {settled:?}")
            });
            assert_eq!(
                (
                    cause.get("Status").and_then(Value::as_str),
                    cause.get("Error").and_then(Value::as_str),
                    cause.get("Cause").and_then(Value::as_str),
                ),
                (
                    Some("FAILED"),
                    Some("ChildBoom"),
                    Some("the child state machine failed"),
                ),
                "case {case:?}: expected the job's own report as the cause, got {cause:?}"
            );
            assert!(
                event(&emulator, &execution_arn, EVENT_TASK_FAILED)
                    .await
                    .is_some(),
                "case {case:?}: expected a {EVENT_TASK_FAILED} event in the parent's history"
            );
            continue;
        }

        // A job that did its work makes the task succeed with the invoked service's
        // own report of that job, which is the document the execution ends with.
        let output: Value = serde_json::from_str(&string(&settled, "output"))
            .unwrap_or_else(|error| panic!("case {case:?}: expected a JSON output, got {error}"));
        assert_eq!(
            output.get("Status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the job's own report as the task's result, got {output:?}"
        );
        assert_eq!(
            output.get("ExecutionArn").and_then(Value::as_str),
            Some(job.as_str()),
            "case {case:?}: expected the report of the job that was started, got {output:?}"
        );
        // The pattern's second revision reports the job's own output as the parsed
        // document; its first reports it as the string that service carries it in.
        let job_output = output
            .get("Output")
            .unwrap_or_else(|| panic!("case {case:?}: expected the job's output in {output:?}"));
        if suffix == ".sync:2" {
            assert_eq!(
                job_output,
                &json!({ "ok": true }),
                "case {case:?}: expected the job's output parsed, got {job_output:?}"
            );
        } else {
            assert_eq!(
                job_output.as_str().map(str::to_string),
                Some(json!({ "ok": true }).to_string()),
                "case {case:?}: expected the job's output as a JSON string, got {job_output:?}"
            );
        }
        assert!(
            event(&emulator, &execution_arn, EVENT_TASK_SUCCEEDED)
                .await
                .is_some(),
            "case {case:?}: expected a {EVENT_TASK_SUCCEEDED} event in the parent's history"
        );
    }
}

/// One row of the stop table: the case, and the `error`/`cause` the stop carries.
type StopRow = (&'static str, Option<&'static str>, Option<&'static str>);

/// Stopping an execution that is PARKED at a job stops that job too, and leaves the
/// execution `ABORTED` with exactly the `error` and `cause` the caller passed.
///
/// The rows vary what the stop carries, because both members are optional ("a caller
/// can stop an execution without using any KMS permissions [...] if the caller
/// provides a null value for both error and cause fields"): what must not vary is
/// that the job ends and nothing reports the task as having succeeded or failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_an_execution_parked_at_a_job_stops_that_job() {
    let rows: [StopRow; 3] = [
        (
            "a stop naming an error and a cause",
            Some("sim2real.Stop"),
            Some("stop while parked"),
        ),
        ("a stop naming only an error", Some("Halt"), None),
        ("a stop naming neither", None, None),
    ];

    let emulator = Emulator::new();
    for (index, (case, error, cause)) in rows.into_iter().enumerate() {
        let child_arn = create_machine(&emulator, &format!("held-{index}"), CHILD_RUNS_ON).await;
        let parent_arn = create_machine(
            &emulator,
            &format!("stopper-{index}"),
            &parent_definition(&child_arn, ".sync"),
        )
        .await;
        let started = emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": parent_arn, "name": format!("stop-{index}") }),
            )
            .await;
        let execution_arn = string(&started, "executionArn");
        let job = submitted_job(&emulator, case, &execution_arn).await;

        // The job is running, and so is the execution parked at it.
        let running = described(&emulator, &job).await;
        assert_eq!(
            running.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the job to be running, got {running:?}"
        );

        let mut request = json!({ "executionArn": execution_arn });
        let members = request.as_object_mut().expect("the request is an object");
        if let Some(error) = error {
            members.insert("error".to_string(), json!(error));
        }
        if let Some(cause) = cause {
            members.insert("cause".to_string(), json!(cause));
        }
        let stopped = emulator.ok("StopExecution", &request).await;
        assert!(
            stopped.get("stopDate").is_some(),
            "case {case:?}: expected a stopDate, got {stopped:?}"
        );

        // (a) the job was stopped through its own service's stopping call;
        let job_after = described(&emulator, &job).await;
        assert_eq!(
            job_after.get("status").and_then(Value::as_str),
            Some("ABORTED"),
            "case {case:?}: expected the job stopped with the execution, got {job_after:?}"
        );
        // (b) the execution reports the values the caller passed, and nothing else;
        let parent_after = described(&emulator, &execution_arn).await;
        assert_eq!(
            parent_after.get("status").and_then(Value::as_str),
            Some("ABORTED"),
            "case {case:?}: expected ABORTED, got {parent_after:?}"
        );
        assert_eq!(
            parent_after.get("error").and_then(Value::as_str),
            error,
            "case {case:?}: expected the error the caller passed, got {parent_after:?}"
        );
        assert_eq!(
            parent_after.get("cause").and_then(Value::as_str),
            cause,
            "case {case:?}: expected the cause the caller passed, got {parent_after:?}"
        );
        // (c) the history ends on the abort, with no outcome claimed for the task.
        let events = history(&emulator, &execution_arn, json!({})).await;
        let types: Vec<&str> = events
            .iter()
            .filter_map(|event| event.get("type").and_then(Value::as_str))
            .collect();
        assert_eq!(
            types.last(),
            Some(&EVENT_EXECUTION_ABORTED),
            "case {case:?}: expected the history to end on {EVENT_EXECUTION_ABORTED}, got {types:?}"
        );
        assert!(
            !types.contains(&EVENT_TASK_SUCCEEDED) && !types.contains(&EVENT_TASK_FAILED),
            "case {case:?}: a stopped task has neither succeeded nor failed, got {types:?}"
        );
        assert!(
            types.contains(&EVENT_TASK_SUBMITTED),
            "case {case:?}: expected the submission of the job in {types:?}"
        );
    }
}

/// A run-a-job task whose job could NOT be started reports what the invoked service
/// answered, and is not parked at a job that does not exist.
///
/// The rows vary what makes the starting call fail — a request the service refuses,
/// and a request naming a resource it does not hold — and each must end the execution
/// `FAILED` under the service's own error code, with the submission recorded as having
/// failed rather than as having happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_that_could_not_be_started_reports_the_services_own_refusal() {
    let absent = format!("arn:aws:states:{REGION}:000000000000:stateMachine:no-such-machine");
    let rows: [(&str, Value, &str); 2] = [
        (
            "a request the service refuses",
            json!({ "input": "{}" }),
            "StepFunctions.ValidationException",
        ),
        (
            "a request naming a state machine the service does not hold",
            json!({ "stateMachineArn": absent }),
            "StepFunctions.StateMachineDoesNotExistException",
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, parameters, expected)) in rows.into_iter().enumerate() {
        let definition = json!({
            "StartAt": "RunChild",
            "States": {
                "RunChild": {
                    "Type": "Task",
                    "Resource": "arn:aws:states:::states:startExecution.sync",
                    "Parameters": parameters,
                    "End": true,
                }
            }
        })
        .to_string();
        let parent_arn = create_machine(&emulator, &format!("unstarted-{index}"), &definition).await;
        let started = emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": parent_arn, "name": format!("unstarted-{index}") }),
            )
            .await;
        let execution_arn = string(&started, "executionArn");

        let settled = settled(&emulator, case, &execution_arn).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {settled:?}"
        );
        assert_eq!(
            settled.get("error").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {expected}, got {settled:?}"
        );
        assert!(
            event(&emulator, &execution_arn, EVENT_TASK_SUBMIT_FAILED)
                .await
                .is_some(),
            "case {case:?}: expected a {EVENT_TASK_SUBMIT_FAILED} event in the history"
        );
        assert!(
            event(&emulator, &execution_arn, EVENT_TASK_SUBMITTED)
                .await
                .is_none(),
            "case {case:?}: nothing was submitted, so no {EVENT_TASK_SUBMITTED} event may appear"
        );
    }
}

/// Every integration the run-a-job catalogue names resolves to the call its ACTION
/// names, under both spellings of the pattern and without it.
///
/// The catalogue is the data the pattern is performed from, so this walks all of it —
/// including `athena:startQueryExecution`, the integration the recorded case this
/// suite's issue was filed about uses — and pins that a `.sync` `Resource` names the
/// same operation its bare form does, and that only the `.sync` forms park at a job.
#[tokio::test]
async fn every_catalogued_run_a_job_integration_names_its_own_call() {
    assert!(
        !RUN_A_JOB.is_empty(),
        "the catalogue of run-a-job integrations must not be empty"
    );
    for entry in RUN_A_JOB {
        let bare = format!("arn:aws:states:::{}", entry.starts);
        let plain = read(&bare)
            .unwrap_or_else(|why| panic!("case {:?}: expected a call, got {why:?}", entry.starts));
        assert!(
            plain.job.is_none(),
            "case {:?}: a Resource naming no pattern parks at no job",
            entry.starts
        );
        for suffix in [".sync", ".sync:2"] {
            let case = format!("{}{suffix}", entry.starts);
            let call = read(&format!("{bare}{suffix}"))
                .unwrap_or_else(|why| panic!("case {case:?}: expected a call, got {why:?}"));
            assert_eq!(
                call.operation, plain.operation,
                "case {case:?}: expected the operation {}, got {}",
                plain.operation, call.operation
            );
            assert_eq!(
                call.api.namespace, plain.api.namespace,
                "case {case:?}: expected the service {}, got {}",
                plain.api.namespace, call.api.namespace
            );
            let job = call
                .job
                .unwrap_or_else(|| panic!("case {case:?}: expected the pattern to park at a job"));
            assert_eq!(
                job.starts, entry.starts,
                "case {case:?}: expected the catalogue entry of {}, got {}",
                entry.starts, job.starts
            );
            assert_eq!(
                call.parsed_output,
                suffix == ".sync:2",
                "case {case:?}: only the second revision parses the job's own output"
            );
            // The calls that report and stop the job are named, and the statuses that
            // end it are stated and disjoint — a job cannot both have done its work
            // and not.
            assert!(
                !job.describe.is_empty() && !job.stop.is_empty() && !job.status_at.is_empty(),
                "case {case:?}: a catalogued job must name its reporting and stopping calls"
            );
            assert!(
                job.succeeded
                    .iter()
                    .all(|status| !job.failed.contains(status)),
                "case {case:?}: a status cannot mean both finished and not"
            );
        }
    }
}

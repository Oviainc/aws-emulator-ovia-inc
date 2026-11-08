// SPDX-License-Identifier: CC0-1.0

//! `SignalWorkflowExecution`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`)
//! through the same `MultiServiceRouter` the emulator mounts in production. The harness
//! is the one `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: a signal is delivered to the ONE run it addresses
//! and only while that run is OPEN — it appends a `WorkflowExecutionSignaled` event
//! carrying the request's `signalName` and `input` and leaves a decision task scheduled
//! for the run's decider, whereas a run that is no longer open is out of this
//! operation's scope and is refused with 400 `UnknownResourceFault` whose message names
//! the requested execution, with NOTHING appended to that run's history.
//!
//! Rows vary what the rule generalises over: the signal name's shape and length, whether
//! `input` is given and what it holds, whether the request names the `runId` or lets the
//! service resolve the open run, how many signals one run receives, and how the run
//! closed before the refused signal (terminated by a caller, or timed out by the clock).

use std::time::Duration;

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, merge, seeded, string_at, Emulator,
};

/// The workflow type every run here is started from.
const TYPE: (&str, &str) = ("demo-wf", "1.0");

/// The task list that type registers as its `defaultTaskList`.
const TASK_LIST: &str = "demo-tasks";

/// One signal, varying the members the rule generalises over.
struct SignalCase {
    name: &'static str,
    signal_name: String,
    input: Option<String>,
    /// Whether the request names the `runId` or lets the service resolve the open run
    /// of the `workflowId`.
    with_run_id: bool,
}

/// The expected `UnknownResourceFault` message when the request ADDRESSED a `runId`:
/// the requested execution and nothing else, the same clause every execution-scoped
/// operation of this service uses.
fn unknown_execution_message(workflow_id: &str, run_id: &str) -> String {
    format!("Unknown execution: WorkflowExecution=[workflowId={workflow_id}, runId={run_id}]")
}

/// The expected `UnknownResourceFault` message when the request named a `workflowId`
/// ALONE: the id it supplied, with no `runId` clause it never asked about.
fn unknown_execution_by_workflow_id_message(workflow_id: &str) -> String {
    format!("Unknown execution, workflowId = {workflow_id}")
}

/// Every event of one run's history, oldest first.
async fn history(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
) -> Vec<Value> {
    let body = emulator
        .ok(
            case,
            "GetWorkflowExecutionHistory",
            &json!({
                "domain": domain,
                "execution": { "workflowId": workflow_id, "runId": run_id },
            }),
        )
        .await;
    body.pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected events in {body:?}"))
}

/// The events of `events` whose `eventType` is `event_type`.
fn of_type(events: &[Value], event_type: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event.get("eventType").and_then(Value::as_str) == Some(event_type))
        .cloned()
        .collect()
}

/// Start one run of the seeded type and return its `runId`.
async fn start_run(emulator: &Emulator, case: &str, domain: &str, workflow_id: &str) -> String {
    let started = emulator
        .ok(
            case,
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "workflowType": { "name": TYPE.0, "version": TYPE.1 },
            }),
        )
        .await;
    string_at(&started, "/runId")
}

#[tokio::test]
async fn a_signal_to_an_open_run_is_recorded_in_that_run_only() {
    let cases = [
        SignalCase {
            name: "a plain name, no input, runId resolved by the service",
            signal_name: "proceed".to_string(),
            input: None,
            with_run_id: false,
        },
        SignalCase {
            name: "a JSON payload and an explicit runId",
            signal_name: "order.updated".to_string(),
            input: Some(json!({ "quantity": 3 }).to_string()),
            with_run_id: true,
        },
        SignalCase {
            name: "a non-ASCII name and an empty input",
            signal_name: "シグナル 名".to_string(),
            input: Some(String::new()),
            with_run_id: true,
        },
        SignalCase {
            name: "the modelled maximum name length and a long input",
            signal_name: "s".repeat(256),
            input: Some("x".repeat(4096)),
            with_run_id: false,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("signal-{index}");
        seeded(
            &emulator,
            case.name,
            &domain,
            TYPE,
            full_defaults(TASK_LIST, "TERMINATE"),
        )
        .await;
        // Two runs of the same type: only the addressed one may be signalled.
        let signalled_run = start_run(&emulator, case.name, &domain, "wf-1").await;
        let bystander_run = start_run(&emulator, case.name, &domain, "wf-2").await;
        let before = history(&emulator, case.name, &domain, "wf-1", &signalled_run).await;

        let mut body = json!({
            "domain": domain,
            "workflowId": "wf-1",
            "signalName": case.signal_name,
        });
        if case.with_run_id {
            merge(&mut body, json!({ "runId": signalled_run }));
        }
        if let Some(input) = &case.input {
            merge(&mut body, json!({ "input": input }));
        }
        emulator
            .ok(case.name, "SignalWorkflowExecution", &body)
            .await;

        let after = history(&emulator, case.name, &domain, "wf-1", &signalled_run).await;
        let signalled = of_type(&after, "WorkflowExecutionSignaled");
        assert_eq!(
            signalled.len(),
            1,
            "case {:?}: the signal is recorded exactly once, got {after:?}",
            case.name
        );
        assert_eq!(
            string_at(
                &signalled[0],
                "/workflowExecutionSignaledEventAttributes/signalName"
            ),
            case.signal_name,
            "case {:?}: the event carries the requested signalName, got {:?}",
            case.name,
            signalled[0]
        );
        assert_eq!(
            signalled[0]
                .pointer("/workflowExecutionSignaledEventAttributes/input")
                .and_then(Value::as_str)
                .map(str::to_string),
            case.input.clone(),
            "case {:?}: the event carries the requested input, got {:?}",
            case.name,
            signalled[0]
        );
        assert_eq!(
            signalled[0].pointer("/workflowExecutionSignaledEventAttributes/externalWorkflowExecution"),
            None,
            "case {:?}: a client's own signal was not sent by another execution, got {:?}",
            case.name,
            signalled[0]
        );
        assert!(
            after.len() > before.len(),
            "case {:?}: the signal grew the history, got {after:?} after {before:?}",
            case.name
        );

        // The run is still open, with a decision task its decider can act on.
        let described = emulator
            .ok(
                case.name,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": "wf-1", "runId": signalled_run },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "OPEN",
            "case {:?}: a signal does not close the run, got {described:?}",
            case.name
        );
        assert_eq!(
            i64_at(&described, "/openCounts/openDecisionTasks"),
            1,
            "case {:?}: the signalled run has a decision task outstanding, got {described:?}",
            case.name
        );

        // A second signal while that decision task is still outstanding is recorded
        // too, and does not pile up another decision task: the task's decider is handed
        // a history that already holds both signals.
        emulator
            .ok(
                case.name,
                "SignalWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": "wf-1",
                    "signalName": "second",
                }),
            )
            .await;
        let twice = history(&emulator, case.name, &domain, "wf-1", &signalled_run).await;
        assert_eq!(
            of_type(&twice, "WorkflowExecutionSignaled").len(),
            2,
            "case {:?}: every signal is recorded, got {twice:?}",
            case.name
        );
        assert_eq!(
            of_type(&twice, "DecisionTaskScheduled").len(),
            1,
            "case {:?}: signals arriving while a decision task is outstanding do not \
             schedule another, got {twice:?}",
            case.name
        );

        // The other run of the same domain and type saw nothing.
        let bystander = history(&emulator, case.name, &domain, "wf-2", &bystander_run).await;
        assert_eq!(
            of_type(&bystander, "WorkflowExecutionSignaled"),
            Vec::<Value>::new(),
            "case {:?}: only the addressed run is signalled, got {bystander:?}",
            case.name
        );
    }
}

/// How a run came to be closed before the signal that is refused.
struct ClosedCase {
    name: &'static str,
    /// The `executionStartToCloseTimeout` the run is started with.
    timeout: &'static str,
    /// Whether a caller terminates it, or the clock is left to close it.
    terminate: bool,
    /// How long to wait for the clock to close it.
    wait: Duration,
}

#[tokio::test]
async fn a_signal_to_a_run_that_is_not_open_is_refused_and_appends_nothing() {
    let cases = [
        ClosedCase {
            name: "closed by TerminateWorkflowExecution",
            timeout: "300",
            terminate: true,
            wait: Duration::ZERO,
        },
        ClosedCase {
            name: "closed by its own start-to-close budget",
            timeout: "1",
            terminate: false,
            wait: Duration::from_millis(1_400),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("signal-closed-{index}");
        seeded(
            &emulator,
            case.name,
            &domain,
            TYPE,
            full_defaults(TASK_LIST, "TERMINATE"),
        )
        .await;
        let started = emulator
            .ok(
                case.name,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": "wf-1",
                    "workflowType": { "name": TYPE.0, "version": TYPE.1 },
                    "executionStartToCloseTimeout": case.timeout,
                }),
            )
            .await;
        let run_id = string_at(&started, "/runId");

        if case.terminate {
            emulator
                .ok(
                    case.name,
                    "TerminateWorkflowExecution",
                    &json!({ "domain": domain, "workflowId": "wf-1" }),
                )
                .await;
        }
        tokio::time::sleep(case.wait).await;

        let before = history(&emulator, case.name, &domain, "wf-1", &run_id).await;
        let described = emulator
            .ok(
                case.name,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": "wf-1", "runId": run_id },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "CLOSED",
            "case {:?}: the run under test must be closed before the signal, \
             got {described:?}",
            case.name
        );

        // Addressed by the explicit runId, and by the workflowId alone: a closed run is
        // out of scope either way.
        let requests = [
            (
                "with the runId",
                json!({
                    "domain": domain,
                    "workflowId": "wf-1",
                    "runId": run_id,
                    "signalName": "too-late",
                }),
                unknown_execution_message("wf-1", &run_id),
            ),
            (
                "without the runId",
                json!({
                    "domain": domain,
                    "workflowId": "wf-1",
                    "signalName": "too-late",
                }),
                unknown_execution_by_workflow_id_message("wf-1"),
            ),
        ];
        for (addressing, body, expected_message) in &requests {
            let label = format!("{} / {addressing}", case.name);
            let (status, refused) = emulator.call("SignalWorkflowExecution", body).await;
            let code = error_code(&refused);
            assert_eq!(
                (status, code.as_str()),
                (400, "UnknownResourceFault"),
                "case {label:?}: signalling a closed run is 400 UnknownResourceFault, \
                 got {status} {code} with {refused:?}"
            );
            let message = refused
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message, expected_message,
                "case {label:?}: the fault names the requested execution and nothing \
                 else, got {message:?}"
            );
        }

        let after = history(&emulator, case.name, &domain, "wf-1", &run_id).await;
        assert_eq!(
            after.len(),
            before.len(),
            "case {:?}: a refused signal appends nothing to the closed run, \
             got {after:?} after {before:?}",
            case.name
        );
        assert_eq!(
            of_type(&after, "WorkflowExecutionSignaled"),
            Vec::<Value>::new(),
            "case {:?}: no WorkflowExecutionSignaled event may reach a closed run, \
             got {after:?}",
            case.name
        );
    }
}

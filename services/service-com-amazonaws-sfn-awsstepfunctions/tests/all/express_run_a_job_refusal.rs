// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the run-a-job (`.sync`) integration pattern inside an EXPRESS
//! workflow, driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test: the run-a-job pattern is served for the STANDARD workflow type
//! only. A definition whose `Task` state asks for it is ACCEPTED at create time
//! whatever the type — it is valid Amazon States Language — and an EXPRESS execution of
//! it then fails AT that state with `States.Runtime`, carrying the service's own
//! refusal of the pattern and the name of the state that asked for it, having started
//! no job: the integration's own call is never made. The same definition run as a
//! STANDARD workflow starts its job and parks at it, which is the other side of the
//! same rule.
//!
//! The rows differ along what the rule generalises over: the revision of the pattern
//! the `Resource` asks for (`.sync` and `.sync:2`), the INTEGRATION it asks it of (a
//! nested execution, whose calls this service serves itself, and an Athena query,
//! whose service is not even mounted here — so a row that reached the call could not
//! answer at all), the name of the offending state, and how the execution was started
//! (`StartSyncExecution`, and `StartExecution` on the same EXPRESS machine).
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds, `create_express` is `sync_execution`'s and the
//! history reader is `execution_history`'s; `tests/all.rs` is this binary's crate
//! root, so each is reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    EXPRESS_RUN_A_JOB_UNSUPPORTED, RUNTIME_ERROR,
};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, string, Emulator, PAST_THE_EXECUTION_DELAY,
};
use crate::sync_execution::create_express;

/// One row of the refusal table: the case, the state name, and the `Resource` that
/// state's run-a-job `Task` names.
type RefusalRow = (&'static str, &'static str, &'static str);

/// A definition whose one `Task` state, named `state`, asks for the run-a-job pattern
/// through `resource`.
fn parent_definition(state: &str, resource: &str, child_arn: &str) -> String {
    json!({
        "StartAt": state,
        "States": {
            state: {
                "Type": "Task",
                "Resource": resource,
                "Parameters": { "StateMachineArn": child_arn, "Input": {} },
                "End": true,
            }
        }
    })
    .to_string()
}

/// A child definition that finishes at once, for the STANDARD control to really run.
const CHILD: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// How many executions `ListExecutions` reports for `machine_arn`.
async fn started_count(emulator: &Emulator, machine_arn: &str) -> usize {
    let listed = emulator
        .ok(
            "ListExecutions",
            &json!({ "stateMachineArn": machine_arn, "maxResults": 10 }),
        )
        .await;
    listed
        .get("executions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// An EXPRESS execution of a definition that asks for the run-a-job pattern fails at
/// the state that asked, naming it, and starts nothing.
#[tokio::test]
async fn an_express_execution_refuses_a_run_a_job_task_and_starts_no_job() {
    let rows: [RefusalRow; 4] = [
        (
            "a nested execution through the pattern's first revision",
            "CallChild",
            "arn:aws:states:::states:startExecution.sync",
        ),
        (
            "the same integration through its second revision",
            "CallChild",
            "arn:aws:states:::states:startExecution.sync:2",
        ),
        (
            "a state of another name, which the refusal must name instead",
            "RunTheChildWorkflow",
            "arn:aws:states:::states:startExecution.sync",
        ),
        (
            // Athena is not mounted beside this service here, so a row that got as
            // far as the integration's own call could not have answered at all: the
            // refusal is the workflow type's, decided before anything is called.
            "another integration's run-a-job pattern entirely",
            "RunQuery",
            "arn:aws:states:::athena:startQueryExecution.sync",
        ),
    ];

    let emulator = Emulator::new();
    let child_arn = create_machine(&emulator, "express-sync-child", CHILD).await;
    for (index, (case, state, resource)) in rows.into_iter().enumerate() {
        let definition = parent_definition(state, resource, &child_arn);
        let express = create_express(&emulator, &format!("express-sync-{index}"), &definition).await;

        // The SYNCHRONOUS start answers the outcome itself.
        let answered = emulator
            .ok(
                "StartSyncExecution",
                &json!({ "stateMachineArn": express, "name": format!("sync-{index}") }),
            )
            .await;
        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {answered:?}"
        );
        assert_eq!(
            answered.get("error").and_then(Value::as_str),
            Some(RUNTIME_ERROR),
            "case {case:?}: expected {RUNTIME_ERROR}, got {answered:?}"
        );
        let cause = string(&answered, "cause");
        assert!(
            cause.contains(EXPRESS_RUN_A_JOB_UNSUPPORTED),
            "case {case:?}: expected the cause to state {EXPRESS_RUN_A_JOB_UNSUPPORTED:?}, got \
             {cause:?}"
        );
        assert!(
            cause.contains(&format!("'{state}'")),
            "case {case:?}: expected the cause to name the state '{state}', got {cause:?}"
        );

        // And the ASYNCHRONOUS start of the same EXPRESS machine reaches the same
        // outcome: the refusal belongs to the workflow type, not to the call that
        // started it. An EXPRESS execution is not reported by `DescribeExecution`, so
        // what it ended as is read off the store the start wrote.
        let started = emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": express, "name": format!("async-{index}") }),
            )
            .await;
        let execution_arn = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let (status, error, async_cause) = {
            let store = emulator.store();
            let mut store = store.lock();
            store.settle(std::time::SystemTime::now());
            let execution = store
                .executions
                .get(&execution_arn)
                .unwrap_or_else(|| panic!("case {case:?}: expected the execution to be stored"));
            (
                execution.status.clone(),
                execution.error.clone(),
                execution.cause.clone().unwrap_or_default(),
            )
        };
        assert_eq!(
            (status.as_str(), error.as_deref()),
            ("FAILED", Some(RUNTIME_ERROR)),
            "case {case:?}: expected an asynchronous EXPRESS execution to fail the same way, got \
             {status} / {error:?}"
        );
        assert!(
            async_cause.contains(EXPRESS_RUN_A_JOB_UNSUPPORTED) && async_cause.contains(&format!("'{state}'")),
            "case {case:?}: expected the same refusal from the asynchronous start, got \
             {async_cause:?}"
        );
    }

    // Nothing was started for the child: the refusal is decided before the
    // integration's own call, so no execution of it exists.
    assert_eq!(
        started_count(&emulator, &child_arn).await,
        0,
        "case \"the child a refused task would have started\": expected no execution of it"
    );
}

/// The SAME definition run as a STANDARD workflow starts its job and parks at it: the
/// refusal above is the workflow type's and not the definition's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standard_execution_of_the_same_definition_starts_its_job() {
    let emulator = Emulator::new();
    let child_arn = create_machine(&emulator, "standard-sync-child", CHILD).await;
    let definition = parent_definition(
        "CallChild",
        "arn:aws:states:::states:startExecution.sync",
        &child_arn,
    );
    let parent = create_machine(&emulator, "standard-sync-parent", &definition).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": parent, "name": "parked" }),
        )
        .await;
    let execution_arn = string(&started, "executionArn");
    let case = "a STANDARD execution of the same definition";

    let described = emulator
        .ok(
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "case {case:?}: expected the parent to be parked at its job, got {described:?}"
    );
    assert_eq!(
        started_count(&emulator, &child_arn).await,
        1,
        "case {case:?}: expected the job to have been started as a child execution"
    );
    let submitted = history(&emulator, &execution_arn, json!({}))
        .await
        .into_iter()
        .any(|event| event.get("type").and_then(Value::as_str) == Some("TaskSubmitted"));
    assert!(
        submitted,
        "case {case:?}: expected the submission of the job in the parent's history"
    );
}

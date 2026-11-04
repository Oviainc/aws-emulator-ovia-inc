// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the history of an execution that is still RUNNING, driven
//! over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test: an execution's history is every event whose own instant has
//! passed, not only the events of an execution that has finished. A walk computes the
//! whole run at once and stamps each event at the instant it happened, so a state
//! entered BEFORE the run's `Wait` states have elapsed is reported from that instant
//! on — while the execution is still `RUNNING` and its outcome is still owed. The
//! events stay in the order the walk numbered them, so what a caller reads is always a
//! prefix of the whole history.
//!
//! The last test is the same rule over a state machine whose
//! `encryptionConfiguration` is a CUSTOMER_MANAGED_KMS_KEY: an encrypted execution
//! progresses exactly as an unencrypted one, so its states are entered and its history
//! reports them.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds, the history reader is `execution_history`'s and
//! the two-service chain is `execution_data_encryption`'s; `tests/all.rs` is this
//! binary's crate root, so each is reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::execution_data_encryption::{granting_policy, Chain};
use crate::execution_history::history;
use crate::state_machine_lifecycle::{create_machine, string, Emulator, REGION, ROLE_ARN};

/// A definition whose one `Wait` state is owed 900 seconds: the execution is `RUNNING`
/// for the whole of a test, with its first state already entered.
const ONE_LONG_WAIT: &str =
    r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":900,"End":true}}}"#;

/// The `type` of each event, in the order they were answered.
fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected a type in {event:?}"))
                .to_string()
        })
        .collect()
}

/// One row: a definition, and the events its execution's history must carry while the
/// execution is still running.
struct Row {
    case: &'static str,
    definition: &'static str,
    input: &'static str,
    events: &'static [&'static str],
}

/// The events an execution has already written are reported while it is still
/// `RUNNING`, over a table that varies WHERE the run's own `Wait` states sit.
///
/// Rows vary along how much of the run is already due — nothing but the start, one
/// state, a state and a whole transition, a nested state's branches — which is the
/// dimension the rule generalises over: each row's history is the events due at the
/// instant it is read, and no row reports an event the clock has not reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_execution_reports_the_events_it_has_already_written() {
    let rows = [
        Row {
            case: "a single long Wait: the state is entered and not yet left",
            definition: ONE_LONG_WAIT,
            input: "{}",
            events: &["ExecutionStarted", "WaitStateEntered"],
        },
        Row {
            case: "a Pass before a long Wait: the Pass is whole",
            definition: r#"{"StartAt":"P","States":{"P":{"Type":"Pass","Result":{"ok":true},"Next":"W"},"W":{"Type":"Wait","Seconds":900,"End":true}}}"#,
            input: "{}",
            events: &[
                "ExecutionStarted",
                "PassStateEntered",
                "PassStateExited",
                "WaitStateEntered",
            ],
        },
        Row {
            case: "a Wait of zero before a long one: the first is entered and left",
            definition: r#"{"StartAt":"Q","States":{"Q":{"Type":"Wait","Seconds":0,"Next":"W"},"W":{"Type":"Wait","Seconds":900,"End":true}}}"#,
            input: "{}",
            events: &[
                "ExecutionStarted",
                "WaitStateEntered",
                "WaitStateExited",
                "WaitStateEntered",
            ],
        },
        Row {
            case: "a Parallel that finishes before a long Wait",
            definition: r#"{"StartAt":"F","States":{"F":{"Type":"Parallel","Branches":[{"StartAt":"A","States":{"A":{"Type":"Pass","End":true}}}],"Next":"W"},"W":{"Type":"Wait","Seconds":900,"End":true}}}"#,
            input: "{}",
            events: &[
                "ExecutionStarted",
                "ParallelStateEntered",
                "ParallelStateStarted",
                "PassStateEntered",
                "PassStateExited",
                "ParallelStateSucceeded",
                "ParallelStateExited",
                "WaitStateEntered",
            ],
        },
        Row {
            case: "a Map iterating two items before a long Wait",
            definition: r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.values","ItemProcessor":{"StartAt":"I","States":{"I":{"Type":"Pass","End":true}}},"Next":"W"},"W":{"Type":"Wait","Seconds":900,"End":true}}}"#,
            input: r#"{"values":[1,2]}"#,
            events: &[
                "ExecutionStarted",
                "MapStateEntered",
                "MapStateStarted",
                "MapIterationStarted",
                "PassStateEntered",
                "PassStateExited",
                "MapIterationSucceeded",
                "MapIterationStarted",
                "PassStateEntered",
                "PassStateExited",
                "MapIterationSucceeded",
                "MapStateSucceeded",
                "MapStateExited",
                "WaitStateEntered",
            ],
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine = create_machine(&emulator, &format!("running-{index}"), row.definition).await;
        let started = emulator
            .ok(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "only",
                    "input": row.input,
                }),
            )
            .await;
        let execution = string(&started, "executionArn");

        // Read at once, while the run's own `Wait` is still owed: the execution has
        // no outcome yet, and the events before that `Wait` have all happened.
        let described = emulator
            .ok("DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the execution to still be running, got {described:?}"
        );
        let events = history(&emulator, &execution, json!({})).await;
        assert_eq!(
            types(&events),
            row.events,
            "case {case:?}: expected the events already written, got {events:?}"
        );
        // Numbered from 1 in the order the walk wrote them, each naming the one
        // before it — so what this read answers is a PREFIX of the whole history
        // rather than a renumbering of it.
        for (position, event) in events.iter().enumerate() {
            let expected = i64::try_from(position + 1).expect("the position fits");
            assert_eq!(
                event.get("id").and_then(Value::as_i64),
                Some(expected),
                "case {case:?}: expected event {expected} at position {position}, got {event:?}"
            );
            let expected_previous = if expected <= 2 { 0 } else { expected - 1 };
            assert_eq!(
                event.get("previousEventId").and_then(Value::as_i64),
                Some(expected_previous),
                "case {case:?}: expected event {expected} to follow {expected_previous}, got \
                 {event:?}"
            );
        }
    }
}

/// An execution of a state machine encrypted with a CUSTOMER MANAGED KMS KEY
/// progresses exactly as an unencrypted one: the same definition enters the same
/// state, and the history reports it.
///
/// The two state machines are created over ONE engine with KMS mounted beside Step
/// Functions, so the key the encrypted one names is the key KMS's own handlers hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_encrypted_execution_enters_its_states() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;

    let rows: [(&str, Value); 2] = [
        ("an unencrypted state machine", Value::Null),
        (
            "a state machine on a customer managed KMS key",
            json!({
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": key,
                "kmsDataKeyReusePeriodSeconds": 300,
            }),
        ),
    ];

    for (index, (case, encryption)) in rows.iter().enumerate() {
        let mut request = json!({
            "name": format!("encrypted-history-{index}"),
            "definition": ONE_LONG_WAIT,
            "roleArn": ROLE_ARN,
        });
        if !encryption.is_null() {
            request["encryptionConfiguration"] = encryption.clone();
        }
        let created = chain.sfn("CreateStateMachine", &request).await;
        let machine = string(&created, "stateMachineArn");
        assert!(
            machine.contains(REGION),
            "case {case:?}: expected the state machine in this endpoint's region, got {machine}"
        );
        let started = chain
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "only" }),
            )
            .await;
        let execution = string(&started, "executionArn");

        let answered = chain
            .sfn(
                "GetExecutionHistory",
                &json!({ "executionArn": execution }),
            )
            .await;
        let events = answered
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected an events list in {answered:?}"))
            .clone();
        assert_eq!(
            types(&events),
            vec!["ExecutionStarted", "WaitStateEntered"],
            "case {case:?}: expected the Wait state to have been entered, got {events:?}"
        );
    }
}

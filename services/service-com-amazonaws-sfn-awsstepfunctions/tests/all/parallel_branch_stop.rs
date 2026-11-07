// SPDX-License-Identifier: CC0-1.0

//! A `Parallel` state's branches are STOPPED by the first branch that fails, so no
//! sibling asks another service for work after that instant.
//!
//! The rule under test: "If any branch fails ... the entire Parallel state is
//! considered to have failed and all the branches are stopped" — the state failed at
//! the instant that branch did, so a surviving branch makes no transition past it and
//! the integration call of a state it had not completed is never made. A client relying
//! on that sees no partially applied side effect.
//!
//! The table varies what the rule generalises over: how far into its own branch the
//! send sits (at the failure's own instant, and at spans of its own after it) and how
//! many siblings there are. A control row — the same fan-out with nothing failing —
//! pins that the send this suite asserts the absence of is one the emulator really
//! makes, so an empty queue is the stop and not a send that never worked. The last test
//! takes the same rule to the other concurrent state: a `Map` that tolerates no failure.
//!
//! One LIMIT of the walk is visible in the rows: the interpreter runs the branches of a
//! `Parallel` one after another in the order the definition lists them, so the stop
//! reaches the branches listed AFTER the one that failed. A sibling listed before it
//! has already been walked when the failure is known, and the events it recorded past
//! that instant are cut back rather than its call unmade; the rows therefore list the
//! failing branch first, and a time-ordered walk of the branches is what would close
//! the gap.
//!
//! Step Functions and Amazon SQS are mounted on ONE engine, so the queue read here is
//! the queue a task's `sqs:sendMessage` would have reached.
//!
//! The assembled router's own suite pins the same rule for a `Parallel` across the
//! services it mounts (`core/router/tests/all/sfn_parallel_branch_cancellation.rs`);
//! this one is the owning crate's, and it is where the rule's other concurrent state —
//! a `Map` whose iteration fails beside a sending one — is asserted.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::integration_failure_error_names::{given_queue, Emulator};
use crate::state_machine_lifecycle::{PAST_THE_EXECUTION_DELAY, ROLE_ARN};

/// How long one poll of the queue waits before reading again.
const POLL: Duration = Duration::from_millis(25);

/// How long a row waits for a message it expects to arrive.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How long a row watches a queue that must stay EMPTY.
///
/// Long enough that the control row's send has arrived by it, so an empty queue is a
/// stopped branch rather than one still in flight.
const SILENCE_WINDOW: Duration = Duration::from_secs(3);

/// Every message body the queue holds within `window`, oldest first, read until `want`
/// of them have arrived or the window closes.
async fn received(emulator: &Emulator, url: &str, want: usize, window: Duration) -> Vec<String> {
    let deadline = Instant::now() + window;
    let mut bodies: Vec<String> = Vec::new();
    while bodies.len() < want && Instant::now() < deadline {
        let answer = emulator
            .sqs(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 0 }),
            )
            .await;
        for message in answer["Messages"].as_array().cloned().unwrap_or_default() {
            bodies.push(message["Body"].as_str().unwrap_or_default().to_string());
        }
        ::tokio::time::sleep(POLL).await;
    }
    bodies
}

/// The branch that sends `body` to `url` after waiting `after` seconds — or at once,
/// for `after` of zero, which is the instant the failing branch's own failure falls on.
fn sending_branch(url: &str, body: &str, after: u64) -> Value {
    let send = json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::sqs:sendMessage",
        "Parameters": { "QueueUrl": url, "MessageBody": body },
        "End": true,
    });
    if after == 0 {
        return json!({ "StartAt": "Send", "States": { "Send": send } });
    }
    json!({
        "StartAt": "Hold",
        "States": {
            "Hold": { "Type": "Wait", "Seconds": after, "Next": "Send" },
            "Send": send,
        },
    })
}

/// A branch that fails at once.
fn failing_branch() -> Value {
    json!({
        "StartAt": "Bang",
        "States": { "Bang": { "Type": "Fail", "Error": "Boom", "Cause": "on purpose" } },
    })
}

/// Create a state machine whose only state is a `Parallel` over `branches`, start one
/// execution of it, and answer what `DescribeExecution` reports once it has settled.
async fn fan_out(emulator: &Emulator, name: &str, branches: Vec<Value>) -> Value {
    let definition = json!({
        "StartAt": "Fan",
        "States": { "Fan": { "Type": "Parallel", "Branches": branches, "End": true } },
    })
    .to_string();
    run_machine(emulator, name, &definition, "{}").await
}

/// Create a state machine with `definition`, start one execution of it on `input`, and
/// answer what `DescribeExecution` reports once it has settled.
async fn run_machine(emulator: &Emulator, name: &str, definition: &str, input: &str) -> Value {
    let created = emulator
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": definition,
                "roleArn": ROLE_ARN,
                "type": "STANDARD",
            }),
        )
        .await;
    let machine = created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no stateMachineArn in {created}"));
    let started = emulator
        .sfn(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "once", "input": input }),
        )
        .await;
    let execution = started["executionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no executionArn in {started}"))
        .to_string();
    // The branches hold for spans of their own, so the state machine's outcome comes due
    // on the walk's own clock: read until the execution has left RUNNING.
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        ::tokio::time::sleep(PAST_THE_EXECUTION_DELAY).await;
        let described = emulator
            .sfn("DescribeExecution", &json!({ "executionArn": &execution }))
            .await;
        if described["status"].as_str() != Some("RUNNING") || Instant::now() >= deadline {
            return described;
        }
    }
}

/// The send a `Parallel` whose sibling branch FAILED had not completed is not made at
/// all, however far into its branch the send sits and however many siblings there are.
#[tokio::test]
async fn a_failed_branch_stops_a_siblings_integration_call() {
    struct Row {
        case: &'static str,
        machine: &'static str,
        /// The branches of the row's `Parallel`, built around the queue and the body.
        branches: fn(&str, &str) -> Vec<Value>,
    }

    let rows = [
        Row {
            // The branches all BEGIN together, so a send due at the very instant the
            // sibling failed is work the failure caught in flight: it does not complete.
            case: "a sibling that would send at once",
            machine: "stops-an-immediate-send",
            branches: |url, body| vec![failing_branch(), sending_branch(url, body, 0)],
        },
        Row {
            case: "a sibling that would send after a hold",
            machine: "stops-a-held-send",
            branches: |url, body| vec![failing_branch(), sending_branch(url, body, 5)],
        },
        Row {
            case: "two siblings that would send at spans of their own",
            machine: "stops-both-sends",
            branches: |url, body| {
                vec![
                    failing_branch(),
                    sending_branch(url, body, 2),
                    sending_branch(url, body, 30),
                ]
            },
        },
    ];

    for Row {
        case,
        machine,
        branches,
    } in rows
    {
        let emulator = Emulator::new();
        let url = given_queue(&emulator, "sibling-sends", json!({})).await;
        let body = "should-not-arrive-if-a-sibling-failed";
        let described = fan_out(&emulator, machine, branches(&url, body)).await;
        assert_eq!(
            described["status"].as_str(),
            Some("FAILED"),
            "case {case:?}: expected the branch's failure to fail the execution, got {described}"
        );
        assert_eq!(
            described["error"].as_str(),
            Some("Boom"),
            "case {case:?}: expected the failing branch's own error, got {described}"
        );
        let delivered = received(&emulator, &url, 1, SILENCE_WINDOW).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: expected no message, got {delivered:?}"
        );
    }
}

/// The control: the same fan-out with nothing failing really does send, so the rows
/// above assert a STOPPED branch and not a call the emulator never makes.
#[tokio::test]
async fn a_parallel_whose_branches_all_succeed_sends_from_each_of_them() {
    let emulator = Emulator::new();
    let url = given_queue(&emulator, "both-send", json!({})).await;
    let case = "two sending branches, neither failing";
    let described = fan_out(
        &emulator,
        "sends-from-both",
        vec![
            sending_branch(&url, "from-the-first", 0),
            sending_branch(&url, "from-the-second", 5),
        ],
    )
    .await;
    assert_eq!(
        described["status"].as_str(),
        Some("SUCCEEDED"),
        "case {case:?}: expected the fan-out to succeed, got {described}"
    );
    let mut delivered = received(&emulator, &url, 2, DELIVERY_DEADLINE).await;
    delivered.sort();
    assert_eq!(
        delivered,
        vec![
            "from-the-first".to_string(),
            "from-the-second".to_string()
        ],
        "case {case:?}: expected both branches' messages, got {delivered:?}"
    );
}

/// The same rule over the other concurrent state: a `Map` that tolerates no failure has
/// failed as soon as one iteration does, so an iteration running BESIDE it makes no
/// transition past that instant either.
///
/// The processor is one state machine over every item, so which item fails is data: the
/// `Choice` reads the item's own index out of the context object, and the iteration of
/// the other item would have sent after a hold of its own.
#[tokio::test]
async fn a_failed_map_iteration_stops_the_iteration_beside_it() {
    let emulator = Emulator::new();
    let url = given_queue(&emulator, "iteration-sends", json!({})).await;
    let case = "an intolerant Map whose first item fails";
    let definition = json!({
        "StartAt": "Each",
        "States": {
            "Each": {
                "Type": "Map",
                "ItemsPath": "$.items",
                // One batch, so both iterations run at once rather than in turn.
                "MaxConcurrency": 0,
                "ItemProcessor": {
                    "ProcessorConfig": { "Mode": "INLINE" },
                    "StartAt": "Pick",
                    "States": {
                        "Pick": {
                            "Type": "Choice",
                            "Choices": [{
                                "Variable": "$$.Map.Item.Index",
                                "NumericEquals": 0,
                                "Next": "Bang",
                            }],
                            "Default": "Hold",
                        },
                        "Bang": { "Type": "Fail", "Error": "Boom", "Cause": "on purpose" },
                        "Hold": { "Type": "Wait", "Seconds": 5, "Next": "Send" },
                        "Send": {
                            "Type": "Task",
                            "Resource": "arn:aws:states:::sqs:sendMessage",
                            "Parameters": {
                                "QueueUrl": &url,
                                "MessageBody": "should-not-arrive-if-a-sibling-failed",
                            },
                            "End": true,
                        },
                    },
                },
                "End": true,
            },
        },
    })
    .to_string();
    let described = run_machine(
        &emulator,
        "stops-its-iterations",
        &definition,
        r#"{"items":[0,1]}"#,
    )
    .await;
    assert_eq!(
        described["status"].as_str(),
        Some("FAILED"),
        "case {case:?}: expected the iteration's failure to fail the execution, got {described}"
    );
    let delivered = received(&emulator, &url, 1, SILENCE_WINDOW).await;
    assert!(
        delivered.is_empty(),
        "case {case:?}: expected no message, got {delivered:?}"
    );
}

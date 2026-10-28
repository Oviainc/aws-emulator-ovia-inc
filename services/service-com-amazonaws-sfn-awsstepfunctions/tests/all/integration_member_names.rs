// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MEMBER NAMES a `Task` state's service integration speaks,
//! driven over both services' own wire protocols through the production
//! `MultiServiceRouter` (the harness `task_integrations.rs` stands up).
//!
//! The rule under test: a `Task` state's integration parameters and its result are
//! written in the integration's own convention — PascalCase — whatever the invoked
//! API spells its own members in, so the document the state built reaches that
//! operation under its model's spelling and the operation's response comes back to
//! the state under PascalCase. A member an integration documents as a document while
//! the operation types it as a string travels as that document's JSON text.
//!
//! Every row drives the rule through an operation and reads the far end back, so a
//! member that did not reach the invoked operation is a refused call rather than an
//! assertion about a document this emulator wrote itself: a state machine ARN that
//! never arrived fails the call with a validation error, and the input a child
//! execution really received is read back through `DescribeExecution`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{string, ROLE_ARN};
use crate::task_integrations::Chain;

/// One row of the spelling table: the case, the `Resource` the task names, the
/// parameters the definition writes (in the integration's PascalCase), and the
/// members the result must carry.
type SpellingRow = (
    &'static str,
    &'static str,
    fn(&str) -> Value,
    &'static [&'static str],
);

/// A `Task` state whose only member beyond the integration is where it goes next.
fn task(resource: &str, parameters: Value) -> Value {
    json!({
        "StartAt": "Work",
        "States": {
            "Work": { "Type": "Task", "Resource": resource, "Parameters": parameters, "End": true }
        }
    })
}

/// The `output` a succeeded execution reports, parsed.
fn output(described: &Value, case: &str) -> Value {
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected SUCCEEDED, got {described:?}"
    );
    described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"))
}

/// The child execution a nested-execution task started, named by the result it
/// reported.
const CHILD: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// A task's integration request reaches the invoked operation whatever CASE the
/// definition wrote its members in, and its result comes back in the integration's
/// own convention.
///
/// The rows differ along what the rule generalises over: the integration FAMILY (the
/// optimized `arn:aws:states:::<namespace>:<action>` form and the AWS SDK
/// `arn:aws:states:::aws-sdk:<sdk name>:<action>` one, which name the same service by
/// two different tokens), the member convention of the invoked MODEL (Step Functions
/// spells its own members in lowerCamel, DynamoDB spells its in PascalCase), and the
/// shape of the result (one member, a list, a document with a caller's own data
/// inside it).
#[tokio::test]
async fn a_tasks_integration_speaks_pascal_case_in_both_directions() {
    let chain = Chain::new();
    chain.create_table("orders").await;
    let created = chain
        .sfn(
            "CreateStateMachine",
            &json!({ "name": "child", "definition": CHILD, "roleArn": ROLE_ARN }),
        )
        .await;
    let child_arn = string(&created, "stateMachineArn");

    let rows: [SpellingRow; 4] = [
        (
            // `StartExecution`'s own members are `stateMachineArn`, `name` and the
            // string `input`; the task writes all three in PascalCase, and `Input`
            // as the document the child receives.
            "a nested execution, through the optimized family",
            "arn:aws:states:::states:startExecution",
            |child| json!({ "StateMachineArn": child, "Input": { "n": 1 } }),
            &["ExecutionArn", "StartDate"],
        ),
        (
            // The same service, named by the token the AWS SDKs give it.
            "the same service named by its SDK name in the AWS SDK family",
            "arn:aws:states:::aws-sdk:sfn:describeStateMachine",
            |child| json!({ "StateMachineArn": child }),
            &["StateMachineArn", "Name", "Definition", "RoleArn", "Type"],
        ),
        (
            // An operation whose result is a LIST of structures.
            "an operation of that service the request names no resource of",
            "arn:aws:states:::aws-sdk:sfn:listStateMachines",
            |_child| json!({ "MaxResults": 10 }),
            &["StateMachines"],
        ),
        (
            // A model that spells its own members in PascalCase already: the
            // convention leaves them alone, and the attribute names INSIDE the item
            // are a caller's data rather than members, so they are not re-spelled.
            "a PascalCase model, whose nested data is left as the caller wrote it",
            "arn:aws:states:::dynamodb:putItem",
            |_child| {
                json!({
                    "TableName": "orders",
                    "Item": { "pk": { "S": "o-1" }, "shipTo": { "S": "here" } }
                })
            },
            &[],
        ),
    ];

    for (index, (case, resource, parameters, expected)) in rows.into_iter().enumerate() {
        let described = chain
            .run(
                &format!("spelling-{index}"),
                &task(resource, parameters(&child_arn)),
                &json!({}),
            )
            .await;
        let result = output(&described, case);
        let members = result
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a document, got {result}"));
        for member in expected {
            assert!(
                members.contains_key(*member),
                "case {case:?}: expected the member {member:?} in the result, got the members \
                 {:?}",
                members.keys().collect::<Vec<_>>()
            );
        }
        for member in members.keys() {
            assert!(
                !member.starts_with(|first: char| first.is_lowercase()),
                "case {case:?}: a result's members are PascalCase, got {member:?} among {:?}",
                members.keys().collect::<Vec<_>>()
            );
        }
    }

    // The child execution the first row started really ran, with the DOCUMENT the
    // task wrote as `Input` as its own input: the optimized integration carries that
    // member as the JSON text `StartExecution` types, so the child receives the
    // document rather than nothing or a quoted string.
    let children = chain
        .sfn(
            "ListExecutions",
            &json!({ "stateMachineArn": child_arn, "maxResults": 10 }),
        )
        .await;
    let started = children
        .get("executions")
        .and_then(Value::as_array)
        .and_then(|executions| executions.first())
        .unwrap_or_else(|| panic!("expected the child execution the task started in {children:?}"));
    let child = chain
        .sfn(
            "DescribeExecution",
            &json!({ "executionArn": string(started, "executionArn") }),
        )
        .await;
    assert_eq!(
        string(&child, "input"),
        json!({ "n": 1 }).to_string(),
        "case \"the child's own input\": expected the document the task wrote, got {child:?}"
    );

    // The attributes the PascalCase row wrote are readable under the names the
    // definition gave them, which is the other side of leaving nested data alone.
    let read = chain
        .run(
            "spelling-read-back",
            &task(
                "arn:aws:states:::dynamodb:getItem",
                json!({ "TableName": "orders", "Key": { "pk": { "S": "o-1" } } }),
            ),
            &json!({}),
        )
        .await;
    let item = output(&read, "the item the task wrote");
    assert_eq!(
        item.get("Item"),
        Some(&json!({ "pk": { "S": "o-1" }, "shipTo": { "S": "here" } })),
        "case {:?}: expected the attribute names the definition wrote, got {item}",
        "the item the task wrote"
    );
}

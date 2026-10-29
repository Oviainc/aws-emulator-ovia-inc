// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SDK response envelope a `Task` state's request-response
//! service integration reports, driven over both services' own wire protocols through
//! the production `MultiServiceRouter` with Step Functions and DynamoDB mounted on ONE
//! engine (the harness `task_integrations.rs` stands up).
//!
//! The rule under test: the result of a request-response call made through an
//! OPTIMIZED integration (`arn:aws:states:::<namespace>:<action>`) is the literal SDK
//! response of that call — the operation's own members under the integration's
//! PascalCase, plus the envelope the SDK puts the response's own HTTP facts in
//! (`SdkHttpMetadata`, carrying the status the call answered, and
//! `SdkResponseMetadata`) — so a definition whose `ResultSelector`, `ResultPath` or
//! downstream `Choice` reads `$.SdkHttpMetadata.HttpStatusCode` resolves it. The
//! result is NOT reshaped: unlike the run-a-job (`.sync`) pattern, a fire-and-forget
//! `states:startExecution` task reports what the start answered and nothing of the
//! child's eventual status or output. An AWS SDK integration
//! (`arn:aws:states:::aws-sdk:<service>:<action>`) reports the operation's own members
//! alone.
//!
//! The rows differ along what the rule generalises over: the integration FAMILY (both
//! of them, over the same operation), the invoked SERVICE (this one and another), the
//! ACTION (a start, a write and a read) and the shape of the operation's own response
//! (two members, none at all, and a document).

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{string, ROLE_ARN};
use crate::task_integrations::Chain;

/// One row of the envelope table: the case, the `Resource`, the parameters the task
/// builds, the members the operation's own response must contribute, and whether the
/// result carries the SDK envelope.
type EnvelopeRow = (
    &'static str,
    &'static str,
    fn(&str) -> Value,
    &'static [&'static str],
    bool,
);

/// The two members the envelope is made of.
const SDK_HTTP_METADATA: &str = "SdkHttpMetadata";
const SDK_RESPONSE_METADATA: &str = "SdkResponseMetadata";

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

/// A child definition the fire-and-forget rows start.
const CHILD: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// A request-response integration's result is the SDK response of its call: the
/// operation's members, and the envelope the optimized family reports them in.
#[tokio::test]
async fn an_optimized_integrations_result_carries_the_sdk_response_envelope() {
    let chain = Chain::new();
    chain.create_table("orders").await;
    let created = chain
        .sfn(
            "CreateStateMachine",
            &json!({ "name": "envelope-child", "definition": CHILD, "roleArn": ROLE_ARN }),
        )
        .await;
    let child_arn = string(&created, "stateMachineArn");

    let rows: [EnvelopeRow; 5] = [
        (
            // The recorded case: a fire-and-forget nested execution, whose result is
            // `StartExecution`'s own response and the envelope around it.
            "a fire-and-forget nested execution",
            "arn:aws:states:::states:startExecution",
            |child| json!({ "StateMachineArn": child, "Input": { "n": 1 } }),
            &["ExecutionArn", "StartDate"],
            true,
        ),
        (
            // An operation whose response carries no member of its own: the envelope
            // is then the whole of the result.
            "a write to another service, whose response carries no member",
            "arn:aws:states:::dynamodb:putItem",
            |_child| json!({ "TableName": "orders", "Item": { "pk": { "S": "e-1" } } }),
            &[],
            true,
        ),
        (
            "a read of another service, whose response carries a document",
            "arn:aws:states:::dynamodb:getItem",
            |_child| json!({ "TableName": "orders", "Key": { "pk": { "S": "e-1" } } }),
            &["Item"],
            true,
        ),
        (
            // The SAME operation through the AWS SDK family, whose result is the
            // operation's own members and nothing else.
            "the AWS SDK family of that same read",
            "arn:aws:states:::aws-sdk:dynamodb:getItem",
            |_child| json!({ "TableName": "orders", "Key": { "pk": { "S": "e-1" } } }),
            &["Item"],
            false,
        ),
        (
            "the AWS SDK family of a start of this service",
            "arn:aws:states:::aws-sdk:sfn:describeStateMachine",
            |child| json!({ "StateMachineArn": child }),
            &["StateMachineArn", "Name"],
            false,
        ),
    ];

    for (index, (case, resource, parameters, expected, enveloped)) in rows.into_iter().enumerate() {
        let described = chain
            .run(
                &format!("envelope-{index}"),
                &task(resource, parameters(&child_arn)),
                &json!({}),
            )
            .await;
        let result = output(&described, case);
        for member in expected {
            assert!(
                result.get(*member).is_some(),
                "case {case:?}: expected the operation's own {member:?} in the result, got \
                 {result}"
            );
        }
        for member in [SDK_HTTP_METADATA, SDK_RESPONSE_METADATA] {
            assert_eq!(
                result.get(member).is_some(),
                enveloped,
                "case {case:?}: expected the envelope member {member:?} to be present \
                 {enveloped}, got {result}"
            );
        }
        if !enveloped {
            continue;
        }
        // The status in the envelope is the one the call really answered.
        assert_eq!(
            result.pointer("/SdkHttpMetadata/HttpStatusCode"),
            Some(&json!(200)),
            "case {case:?}: expected the status the call answered, got {result}"
        );
        // The result is not RESHAPED: a fire-and-forget start reports what the start
        // answered and nothing of the child's eventual outcome, which only the
        // run-a-job pattern reports.
        for reshaped in ["Status", "Output", "StopDate"] {
            assert!(
                result.get(reshaped).is_none(),
                "case {case:?}: a request-response result carries no {reshaped:?}, got {result}"
            );
        }
    }
}

/// An INSTANT in a task's result is the one the call answered, reported in the unit
/// the SDK response renders a date in: whole milliseconds since the epoch.
///
/// The value is read back against the child execution's own `startDate`, which this
/// API reports in the protocol's epoch seconds, so what is asserted is that the two
/// name the same instant in two units — not a number this emulator chose.
#[tokio::test]
async fn an_instant_in_a_result_is_the_answered_one_in_epoch_millis() {
    let chain = Chain::new();
    let created = chain
        .sfn(
            "CreateStateMachine",
            &json!({ "name": "millis-child", "definition": CHILD, "roleArn": ROLE_ARN }),
        )
        .await;
    let child_arn = string(&created, "stateMachineArn");
    let case = "a fire-and-forget nested execution's start date";
    let described = chain
        .run(
            "envelope-millis",
            &task(
                "arn:aws:states:::states:startExecution",
                json!({ "StateMachineArn": child_arn }),
            ),
            &json!({}),
        )
        .await;
    let result = output(&described, case);
    let reported = result
        .get("StartDate")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected an integer StartDate in {result}"));
    // The execution the task really started, read back through this API.
    let child_execution = string(&result, "ExecutionArn");
    let child = chain
        .sfn(
            "DescribeExecution",
            &json!({ "executionArn": child_execution }),
        )
        .await;
    let seconds = child
        .get("startDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected the child's own startDate in {child:?}"));
    let drift = (f64::from(i32::try_from(reported / 1_000).expect("the seconds fit")) - seconds)
        .abs();
    assert!(
        drift <= 1.0,
        "case {case:?}: expected the start date the call answered ({seconds} seconds), got \
         {reported} milliseconds"
    );
}

/// The envelope is reachable by a PATH, which is what a definition reads it with: a
/// `ResultSelector` over `$.SdkHttpMetadata.HttpStatusCode` resolves, and a `Choice`
/// on what it selected routes.
#[tokio::test]
async fn a_definition_reads_the_envelope_through_a_path() {
    let chain = Chain::new();
    chain.create_table("paths").await;
    let definition = json!({
        "StartAt": "Work",
        "States": {
            "Work": {
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Parameters": { "TableName": "paths", "Item": { "pk": { "S": "p-1" } } },
                "ResultSelector": { "code.$": "$.SdkHttpMetadata.HttpStatusCode" },
                "Next": "Check"
            },
            "Check": {
                "Type": "Choice",
                "Choices": [{ "Variable": "$.code", "NumericEquals": 200, "Next": "Ok" }],
                "Default": "Bad"
            },
            "Ok": { "Type": "Pass", "Result": { "routed": "ok" }, "End": true },
            "Bad": { "Type": "Fail", "Error": "NotTwoHundred" }
        }
    });
    let case = "a definition reading the envelope through a path";
    let described = chain.run("envelope-path", &definition, &json!({})).await;
    assert_eq!(
        output(&described, case),
        json!({ "routed": "ok" }),
        "case {case:?}: expected the Choice to route on the status the envelope carried, got \
         {described:?}"
    );
}

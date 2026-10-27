// SPDX-License-Identifier: CC0-1.0

//! What a write of an event bus, and of the rules and grants on it, accepts and
//! refuses — end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * an operation that addresses an EXISTING bus or rule resolves it before it
//!   judges the members of the request body, so a call naming one that does not
//!   exist is answered with the missing resource whatever else the request
//!   carries — the two conditions call for different remedies, and only one of
//!   them is the caller's body;
//! * a submitted `LogConfig` names BOTH of its members: `Level` ("the level of
//!   logging detail to include ... applies to all log destinations for the event
//!   bus") and `IncludeDetail` ("whether EventBridge include detailed event
//!   information in the records it generates"), so a configuration leaving
//!   either out is refused naming the missing member;
//! * a resource-policy statement renders a principal the way the policy language
//!   writes it: the wildcard as the bare string `"*"`, a named account as an
//!   Amazon Web Services principal under `AWS`.
//!
//! Each table spans the class rather than one case: the rows differ along which
//! operation addresses the resource and which member of the body is wrong, along
//! which member of the configuration is missing, and along which of the two
//! forms `PutPermission$Principal` admits (its modelled pattern is
//! `^(\d{12}|\*)$`) is granted.
//!
//! The rows are bodies THIS SERVICE judges. A value the MODEL already constrains
//! (an enum-valued member, a length, a pattern) is refused by the boundary
//! validator before any handler runs, which is where AWS refuses it too, so such
//! a body is not one of the rows below.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The `message` of a failure body.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A bus `name` with a rule `rule` on it, so the rows below have something to
/// address before it goes away.
async fn given_bus_and_rule(router: &MultiServiceRouter, name: &str, rule: &str) {
    let (status, body) = events(router, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: CreateEventBus answered {status} {body}"
    );
    let (status, body) = events(
        router,
        "PutRule",
        &json!({
            "Name": rule,
            "EventBusName": name,
            "EventPattern": json!({ "source": ["orders"] }).to_string(),
        }),
    )
    .await;
    assert_eq!(status, 200, "case {rule}: PutRule answered {status} {body}");
}

/// A request whose body this service refuses AND whose bus is gone reads that the
/// BUS is gone: the resource is resolved first, so the caller is told the one
/// thing no change to its body can fix. The same body against the bus that still
/// exists is refused for the body, which is what makes the order observable.
#[tokio::test]
async fn a_missing_bus_or_rule_is_reported_before_the_body_is_judged() {
    let bus = "orders";
    let rule = "on-order";
    // (case, the operation, a request whose BODY this service refuses)
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a rule with neither an EventPattern nor a ScheduleExpression",
            "PutRule",
            json!({ "Name": rule, "EventBusName": bus }),
        ),
        (
            "a rule whose EventPattern is not a pattern",
            "PutRule",
            json!({ "Name": rule, "EventBusName": bus, "EventPattern": "not-json" }),
        ),
        (
            "a rule whose ScheduleExpression is neither a rate nor a cron expression",
            "PutRule",
            json!({ "Name": rule, "EventBusName": bus, "ScheduleExpression": "every friday" }),
        ),
        (
            "targets carrying one Id twice",
            "PutTargets",
            json!({
                "Rule": rule,
                "EventBusName": bus,
                "Targets": [
                    { "Id": "one", "Arn": "arn:aws:sqs:eu-west-2:000000000000:q" },
                    { "Id": "one", "Arn": "arn:aws:sqs:eu-west-2:000000000000:other" },
                ],
            }),
        ),
        (
            "a target whose Arn is not an ARN",
            "PutTargets",
            json!({
                "Rule": rule,
                "EventBusName": bus,
                "Targets": [{ "Id": "one", "Arn": "not-an-arn" }],
            }),
        ),
        (
            "a grant that is both a whole policy and one statement",
            "PutPermission",
            json!({
                "EventBusName": bus,
                "Policy": json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
                "StatementId": "anyone",
                "Action": "events:PutEvents",
                "Principal": "*",
            }),
        ),
        (
            "a log configuration that names no Level",
            "UpdateEventBus",
            json!({ "Name": bus, "LogConfig": { "IncludeDetail": "FULL" } }),
        ),
    ];

    for (case, operation, request) in &rows {
        // While the bus is there, the request is refused for its BODY.
        let router = emulator();
        given_bus_and_rule(&router, bus, rule).await;
        let (status, body) = events(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal of the body, got {status} {body}"
        );
        let body_code = error_code(&body);
        assert!(
            body_code != "ResourceNotFoundException",
            "case {case:?}: with the bus in place the refusal is about the body, \
             got {body_code} {body}"
        );

        // With the bus gone, the SAME request reads that the bus is gone.
        let (status, deleted) = events(&router, "DeleteRule", &json!({ "Name": rule, "EventBusName": bus })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteRule answered {status} {deleted}"
        );
        let (status, deleted) = events(&router, "DeleteEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEventBus answered {status} {deleted}"
        );
        let (status, body) = events(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal naming the missing bus, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: a request against a bus that is gone reports the missing bus, \
             got {body}"
        );
        assert!(
            message(&body).contains(bus),
            "case {case:?}: the refusal names the bus that is gone, got {body}"
        );
    }
}

/// A `LogConfig` is accepted only when it names both of its members, and the
/// refusal names the one that is missing — on both writers of a bus's
/// configuration.
#[tokio::test]
async fn a_log_config_names_both_of_its_members() {
    // (case, the LogConfig, the member the refusal must name — None when the
    //  configuration is a complete one)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        ("no member at all", json!({}), Some("IncludeDetail")),
        (
            "IncludeDetail without a Level",
            json!({ "IncludeDetail": "FULL" }),
            Some("Level"),
        ),
        (
            "a Level without IncludeDetail",
            json!({ "Level": "INFO" }),
            Some("IncludeDetail"),
        ),
        // Every value pair the two modelled enums admit is a complete
        // configuration. Source: `smithy_query(query="shape",
        // shape="IncludeDetail,Level")` — IncludeDetail is NONE | FULL and
        // Level is OFF | ERROR | INFO | TRACE.
        (
            "both members, at the quietest pair",
            json!({ "IncludeDetail": "NONE", "Level": "OFF" }),
            None,
        ),
        (
            "both members, at the loudest pair",
            json!({ "IncludeDetail": "FULL", "Level": "TRACE" }),
            None,
        ),
        (
            "both members, at a pair neither bound names",
            json!({ "IncludeDetail": "NONE", "Level": "ERROR" }),
            None,
        ),
    ];

    for (case, log_config, missing) in &rows {
        for operation in ["CreateEventBus", "UpdateEventBus"] {
            let router = emulator();
            let bus = "configured";
            if operation == "UpdateEventBus" {
                let (status, body) = events(&router, "CreateEventBus", &json!({ "Name": bus })).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateEventBus answered {status} {body}"
                );
            }
            let request = json!({ "Name": bus, "LogConfig": log_config });
            let (status, body) = events(&router, operation, &request).await;
            match missing {
                Some(member) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?} on {operation}: a configuration missing {member} must be \
                         refused, got {status} {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "ValidationException",
                        "case {case:?} on {operation}: expected ValidationException, got {body}"
                    );
                    assert!(
                        message(&body).contains(member),
                        "case {case:?} on {operation}: the refusal names {member}, got {body}"
                    );
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?} on {operation}: a complete configuration must be accepted, \
                         got {status} {body}"
                    );
                    assert_eq!(
                        body["LogConfig"], *log_config,
                        "case {case:?} on {operation}: the stored configuration is reported back, \
                         got {body}"
                    );
                }
            }
        }
    }
}

/// The policy document a client reads back renders each principal the way the
/// policy language writes it.
#[tokio::test]
async fn a_policy_statement_renders_the_principal_the_policy_language_writes() {
    let router = emulator();
    let bus = "granted";
    let (status, body) = events(&router, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");

    // (case, the Principal granted, the member the statement must render).
    // `PutPermission$Principal`'s modelled pattern is `^(\d{12}|\*)$`, so the
    // rows are the wildcard and account ids — including one the issue did not
    // cite.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("every account", "*", json!("*")),
        (
            "this account",
            engine::emulator_account_id(),
            json!({ "AWS": engine::emulator_account_id() }),
        ),
        (
            "another account",
            "210987654321",
            json!({ "AWS": "210987654321" }),
        ),
    ];

    for (case, principal, expected) in &rows {
        let sid = format!("sid-{}", case.replace(' ', "-"));
        let (status, body) = events(
            &router,
            "PutPermission",
            &json!({
                "EventBusName": bus,
                "Action": "events:PutEvents",
                "Principal": principal,
                "StatementId": sid,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutPermission answered {status} {body}"
        );

        let (status, body) = events(&router, "DescribeEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeEventBus answered {status} {body}"
        );
        let policy: Value = serde_json::from_str(
            body["Policy"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Policy, got {body}")),
        )
        .unwrap_or_else(|error| panic!("case {case:?}: the Policy is not JSON ({error}): {body}"));
        let statement = policy["Statement"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|statement| statement["Sid"] == json!(sid))
            .unwrap_or_else(|| panic!("case {case:?}: expected a statement {sid}, got {policy}"));
        assert_eq!(
            statement["Principal"], *expected,
            "case {case:?}: expected the principal rendered as {expected}, got {statement}"
        );
    }
}

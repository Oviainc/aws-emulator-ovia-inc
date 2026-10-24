// SPDX-License-Identifier: CC0-1.0

//! How this service IDENTIFIES a resource it holds no record of, end to end over
//! the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`)
//! through the real `MultiServiceRouter`.
//!
//! The rule: every refusal for a missing resource is worded by THIS service from
//! the request's own identity for that resource — a rule by its NAME and the
//! EVENT BUS it was looked up on (a rule name is unique only within its bus, so
//! without the bus a caller cannot tell "no such rule anywhere" from "that rule
//! lives on another bus"), an event bus by its NAME — never by echoing back the
//! ARN or any other text the caller sent. One wording per missing-resource kind,
//! whatever operation ran into it.
//!
//! Each table spans the class the rule generalises over rather than one recorded
//! request: the OPERATION that ran the lookup (a read, a target write, a target
//! removal, a target read, all three tagging calls — which name the resource by
//! ARN rather than by member — and the data plane), the bus the resource was
//! sought on (implicit default, explicit default, a bus that exists, one that
//! does not, one whose name carries `/`), and the form of the name itself.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, and so the region segment of
/// every ARN these tests build.
const REGION: &str = "eu-west-2";

/// The bus a request naming none is resolved against: "If you omit this, the
/// default event bus is used" (`DescribeRule`'s `EventBusName`).
const DEFAULT_BUS: &str = "default";

/// The emulator: one router carrying only this service over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    Emulator { router }
}

/// This emulator's account id — the account segment of every ARN it mints.
fn account() -> &'static str {
    engine::emulator_account_id()
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` an error body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// `arn:aws:events:<region>:<account>:rule/<bus-if-not-default>/<name>` — the ARN
/// form this service mints for a rule, built here rather than read from the
/// implementation.
fn rule_arn(bus: &str, name: &str) -> String {
    let resource = if bus == DEFAULT_BUS {
        format!("rule/{name}")
    } else {
        format!("rule/{bus}/{name}")
    };
    format!("arn:aws:events:{REGION}:{}:{resource}", account())
}

/// `arn:aws:events:<region>:<account>:event-bus/<name>`, built here for the same
/// reason.
fn bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account())
}

/// A target ARN pointing at another service's resource.
fn target_arn(resource: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{resource}", account())
}

/// What the rule under test says a missing RULE refusal reads, composed here from
/// the case's own rule name and bus — not imported from the implementation.
fn expected_rule_wording(bus: &str, rule: &str) -> String {
    format!("Rule {rule} does not exist on EventBus {bus}.")
}

/// The same for a missing EVENT BUS.
fn expected_bus_wording(bus: &str) -> String {
    format!("Event bus {bus} does not exist.")
}

/// Every operation that resolves a rule by name refuses a missing one with the
/// rule NAMED and the event bus it was sought on named too, whichever way the
/// caller addressed it (members, or an ARN) — and with no ARN text in the
/// message.
#[tokio::test]
async fn a_missing_rule_is_named_with_its_event_bus_by_every_operation() {
    let emulator = emulator();

    // One bus and one rule really exist, so a row can ask for a stored rule name
    // under another bus: that names a DIFFERENT resource, one nothing created.
    let (status, body) = call(&emulator, "CreateEventBus", &json!({ "Name": "orders-bus" })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
    let (status, body) = call(
        &emulator,
        "PutRule",
        &json!({ "Name": "on-default", "EventPattern": r#"{"source":["com.demo"]}"# }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");

    // (case, the `EventBusName` member the request carries, the bus the lookup
    // runs on, the rule name). The rows differ along the bus the rule is sought
    // on and the form of both names.
    let rows: Vec<(&str, Option<&str>, &str, &str)> = vec![
        (
            "a rule on the implicit default bus",
            None,
            DEFAULT_BUS,
            "absent-rule",
        ),
        (
            "a rule on the default bus, named explicitly",
            Some(DEFAULT_BUS),
            DEFAULT_BUS,
            "absent.rule.dotted",
        ),
        (
            "a rule on a custom bus that exists",
            Some("orders-bus"),
            "orders-bus",
            "absent_rule_underscored",
        ),
        (
            "a rule on a bus that does not exist either",
            Some("ghost-bus"),
            "ghost-bus",
            "absent-rule",
        ),
        (
            "a rule on a bus whose name carries a slash (the partner-bus form)",
            Some("team/orders"),
            "team/orders",
            "absent-rule",
        ),
        (
            "a stored rule's name under another bus",
            Some("orders-bus"),
            "orders-bus",
            "on-default",
        ),
    ];

    for (case, bus_member, bus, rule) in rows {
        let arn = rule_arn(bus, rule);
        let with_bus = |mut body: Value| {
            if let Some(name) = bus_member {
                body["EventBusName"] = json!(name);
            }
            body
        };
        let requests: Vec<(&str, Value)> = vec![
            ("DescribeRule", with_bus(json!({ "Name": rule }))),
            ("ListTargetsByRule", with_bus(json!({ "Rule": rule }))),
            (
                "PutTargets",
                with_bus(json!({
                    "Rule": rule,
                    "Targets": [{ "Id": "t1", "Arn": target_arn("orders-queue") }],
                })),
            ),
            (
                "RemoveTargets",
                with_bus(json!({ "Rule": rule, "Ids": ["t1"] })),
            ),
            (
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": [{ "Key": "Env", "Value": "test" }] }),
            ),
            (
                "UntagResource",
                json!({ "ResourceARN": arn, "TagKeys": ["Env"] }),
            ),
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
        ];

        let expected = expected_rule_wording(bus, rule);
        for (operation, body) in requests {
            let (status, answered) = call(&emulator, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected 400, got {status} {answered:?}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, \
                 got {code:?} {answered:?}"
            );
            let actual = message(&answered);
            assert_eq!(
                actual, expected,
                "case {case:?}/{operation}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains("arn:"),
                "case {case:?}/{operation}: the refusal must name the rule and its bus, \
                 not echo an ARN, got {actual:?}"
            );
        }
    }
}

/// Every operation that resolves an event bus by name refuses a missing one by
/// NAMING that bus, however the caller addressed it — including the per-entry
/// failure the data plane reports.
#[tokio::test]
async fn a_missing_event_bus_is_named_by_every_operation() {
    let emulator = emulator();
    let (status, body) = call(&emulator, "CreateEventBus", &json!({ "Name": "orders-bus" })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");

    // The rows differ along the form of the absent name, including one that
    // differs from a bus that EXISTS only in case — bus names are case sensitive,
    // so this one names nothing.
    let buses = ["ghost-bus", "ghost.bus.dotted", "ghost_bus", "Orders-Bus"];

    for bus in buses {
        let arn = bus_arn(bus);
        let requests: Vec<(&str, Value)> = vec![
            ("DescribeEventBus", json!({ "Name": bus })),
            ("UpdateEventBus", json!({ "Name": bus })),
            ("ListRules", json!({ "EventBusName": bus })),
            (
                "PutRule",
                json!({
                    "Name": "any-rule",
                    "EventPattern": r#"{"source":["com.demo"]}"#,
                    "EventBusName": bus,
                }),
            ),
            (
                "DeleteRule",
                json!({ "Name": "any-rule", "EventBusName": bus }),
            ),
            (
                "PutPermission",
                json!({
                    "EventBusName": bus,
                    "StatementId": "sid",
                    "Action": "events:PutEvents",
                    "Principal": "*",
                }),
            ),
            (
                "RemovePermission",
                json!({ "EventBusName": bus, "StatementId": "sid" }),
            ),
            (
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": [{ "Key": "Env", "Value": "test" }] }),
            ),
            (
                "UntagResource",
                json!({ "ResourceARN": arn, "TagKeys": ["Env"] }),
            ),
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
        ];

        let expected = expected_bus_wording(bus);
        for (operation, body) in requests {
            let (status, answered) = call(&emulator, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {bus:?}/{operation}: expected 400, got {status} {answered:?}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {bus:?}/{operation}: expected ResourceNotFoundException, \
                 got {code:?} {answered:?}"
            );
            let actual = message(&answered);
            assert_eq!(
                actual, expected,
                "case {bus:?}/{operation}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains("arn:"),
                "case {bus:?}/{operation}: the refusal must name the bus, not echo an ARN, \
                 got {actual:?}"
            );
        }

        // `PutEvents` reports the same wording per ENTRY, because the entry names
        // its own bus: the call succeeds and the entry carries the failure.
        let (status, answered) = call(
            &emulator,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": "com.demo.widgets",
                    "DetailType": "Order Created",
                    "Detail": "{}",
                    "EventBusName": bus,
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {bus:?}/PutEvents: a per-entry failure is still a 200, got {status} {answered:?}"
        );
        let entry = &answered["Entries"][0];
        assert_eq!(
            entry["ErrorCode"],
            json!("ResourceNotFoundException"),
            "case {bus:?}/PutEvents: expected a per-entry ResourceNotFoundException, \
             got {answered:?}"
        );
        assert_eq!(
            entry["ErrorMessage"],
            json!(expected),
            "case {bus:?}/PutEvents: expected {expected:?}, got {answered:?}"
        );
    }
}

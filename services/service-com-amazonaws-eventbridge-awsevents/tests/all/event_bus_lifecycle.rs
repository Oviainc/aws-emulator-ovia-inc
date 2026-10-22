// SPDX-License-Identifier: CC0-1.0

//! The EventBridge event-bus slice end to end over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: AWSEvents.<Operation>`), dispatched through the
//! real `MultiServiceRouter` — so every case exercises routing, deserialization,
//! the modelled-constraint validator, the handler, the persisted store and
//! serialization: the same path a client takes.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded
//! shape:
//!
//! * a created bus is reported back member for member from the record
//!   `CreateEventBus` wrote, under an ARN derived from that record's name and the
//!   request's region, and a read repeated changes nothing a later caller sees;
//! * the bus NAME is its identity, so a name already stored — the account's
//!   `default` bus included — is refused and the stored bus is left alone, while
//!   names differing only in case are two different buses;
//! * a request no bus can be built from is refused with the error for WHY, and
//!   nothing is stored;
//! * a bus no record carries is `ResourceNotFoundException`, whether it was asked
//!   for by name or by ARN;
//! * a created bus is the SAME bus the rule operations see, because both read one
//!   store.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    ComAmazonawsEventbridgeAwseventsState, DEFAULT_EVENT_BUS_NAME, MAX_EVENT_BUSES,
};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for. Deliberately NOT the store's
/// fallback region, so an ARN that ignored the request's credential scope would
/// show up as a wrong region segment rather than passing by accident.
const REGION: &str = "eu-west-2";

/// The members `CreateEventBus` both accepts and reports back, so a stored bus
/// can be compared against the request that created it. From
/// `smithy_query(query="operation", operation="CreateEventBus")`: the response
/// carries `EventBusArn` plus exactly these.
const ECHOED_MEMBERS: [&str; 4] = [
    "Description",
    "KmsKeyIdentifier",
    "DeadLetterConfig",
    "LogConfig",
];

/// `IncludeDetail`'s values, from `smithy_query(query="shape",
/// shape="IncludeDetail")`. Spelled out here, not imported, so a wrong set in the
/// implementation cannot make a test pass.
const INCLUDE_DETAIL_VALUES: [&str; 2] = ["FULL", "NONE"];

/// `Level`'s values, from `smithy_query(query="shape", shape="Level")`.
const LEVEL_VALUES: [&str; 4] = ["ERROR", "INFO", "OFF", "TRACE"];

/// `EventBusName`'s modelled `@length(min: 1, max: 256)`, from
/// `smithy_query(query="operation", operation="CreateEventBus")`.
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 256;

/// `EventBusDescription`'s modelled `@length(min: 0, max: 512)`, same source.
const DESCRIPTION_MAX: usize = 512;

/// The emulator: one router carrying only this service over one shared engine,
/// plus that engine, so a test can read the store the handlers wrote.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
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

/// `arn:aws:events:<region>:<account>:event-bus/<name>` — built here rather than
/// read from the implementation.
fn event_bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account())
}

/// The names of every bus the store holds, in its own order.
fn stored_buses(emulator: &Emulator) -> Vec<String> {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .buses
        .keys()
        .cloned()
        .collect()
}

/// A name of `len` characters the modelled `EventBusName` pattern admits.
fn name_of_length(len: usize) -> String {
    "b".repeat(len)
}

/// Every member of `request` that `CreateEventBus` echoes, compared against
/// `reported` — an absent member must stay absent, never become a default.
fn assert_echoes_request(case: &str, source: &str, request: &Value, reported: &Value) {
    for member in ECHOED_MEMBERS {
        let expected = request.get(member).cloned().unwrap_or(Value::Null);
        let actual = reported.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            actual, expected,
            "case {case}: {source} must report {member} {expected}, got {actual} in {reported}"
        );
    }
}

/// A created bus is reported back from the store it was written to: by
/// `CreateEventBus` itself and by `DescribeEventBus`, member for member, under an
/// ARN built from the bus name and the request's region. The rows differ along
/// the name's form and length, which optional members are present, every value
/// each modelled `LogConfig` enum admits, and how many tags arrive.
#[tokio::test]
async fn a_created_bus_is_reported_back_member_for_member() {
    let emulator = emulator();
    let mut cases = vec![
        json!({ "Name": name_of_length(NAME_MIN) }),
        json!({ "Name": name_of_length(NAME_MAX) }),
        json!({ "Name": "dots.and-dashes_and.9" }),
        json!({ "Name": "description-empty", "Description": "" }),
        json!({
            "Name": "description-at-the-bound",
            "Description": "d".repeat(DESCRIPTION_MAX),
        }),
        json!({
            "Name": "customer-managed-key",
            "KmsKeyIdentifier": "alias/events-bus-key",
        }),
        json!({
            "Name": "dead-letter-queue",
            "DeadLetterConfig": { "Arn": format!("arn:aws:sqs:{REGION}:{}:dlq", account()) },
        }),
        json!({ "Name": "no-tags", "Tags": [] }),
        json!({
            "Name": "many-tags",
            "Tags": [
                { "Key": "team", "Value": "platform" },
                { "Key": "env", "Value": "test" },
                { "Key": "owner", "Value": "sim2real" },
            ],
        }),
        json!({
            "Name": "everything-at-once",
            "Description": "a bus carrying every optional member",
            "KmsKeyIdentifier": "arn:aws:kms:us-east-1:000000000000:key/abc",
            "DeadLetterConfig": { "Arn": format!("arn:aws:sqs:{REGION}:{}:spill", account()) },
            "LogConfig": { "IncludeDetail": "FULL", "Level": "TRACE" },
            "Tags": [{ "Key": "sim2real", "Value": "true" }],
        }),
    ];
    // One row per published value of each `LogConfig` enum, so no admitted value
    // is refused and none is quietly replaced by a default.
    for include_detail in INCLUDE_DETAIL_VALUES {
        for level in LEVEL_VALUES {
            cases.push(json!({
                "Name": format!("log-{}-{}", include_detail.to_lowercase(), level.to_lowercase()),
                "LogConfig": { "IncludeDetail": include_detail, "Level": level },
            }));
        }
    }

    for case in &cases {
        let name = case["Name"].as_str().expect("the case names the bus");
        let label = format!("Name={name}");
        let (status, created) = call(&emulator, "CreateEventBus", case).await;
        assert_eq!(
            status, 200,
            "case {label}: CreateEventBus answered {status} {created}"
        );
        let expected_arn = event_bus_arn(name);
        assert_eq!(
            created["EventBusArn"],
            json!(expected_arn),
            "case {label}: expected EventBusArn {expected_arn}, got {created}"
        );
        assert_echoes_request(&label, "CreateEventBus", case, &created);

        let (status, described) =
            call(&emulator, "DescribeEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {label}: DescribeEventBus answered {status} {described}"
        );
        assert_eq!(
            described["Name"],
            json!(name),
            "case {label}: expected Name {name}, got {described}"
        );
        assert_eq!(
            described["Arn"],
            json!(expected_arn),
            "case {label}: expected Arn {expected_arn}, got {described}"
        );
        assert_echoes_request(&label, "DescribeEventBus", case, &described);
        let creation_time = described["CreationTime"].as_f64().unwrap_or_else(|| {
            panic!("case {label}: expected a CreationTime instant, got {described}")
        });
        assert!(
            creation_time > 0.0,
            "case {label}: expected a CreationTime after the epoch, got {creation_time}"
        );
        assert_eq!(
            described["LastModifiedTime"], described["CreationTime"],
            "case {label}: a bus nothing has changed was last modified when it was \
             created, got {described}"
        );
        for absent in ["Policy", "ManagedBy"] {
            assert_eq!(
                described.get(absent).cloned().unwrap_or(Value::Null),
                Value::Null,
                "case {label}: nothing has set {absent} on this bus, got {described}"
            );
        }

        // The ARN form of `Name` addresses the same bus (`EventBusNameOrArn`),
        // and a read repeated reports exactly what it reported the first time.
        let (status, by_arn) =
            call(&emulator, "DescribeEventBus", &json!({ "Name": expected_arn })).await;
        assert_eq!(
            status, 200,
            "case {label}: DescribeEventBus by ARN answered {status} {by_arn}"
        );
        assert_eq!(
            by_arn, described,
            "case {label}: the ARN and the bare name must address one bus, got {by_arn} \
             then {described}"
        );
    }

    // Describing the account's `default` bus needs no `Name` at all: "if you omit
    // this, the default event bus is displayed".
    let (status, default_bus) = call(&emulator, "DescribeEventBus", &json!({})).await;
    assert_eq!(
        status, 200,
        "DescribeEventBus with no Name answered {status} {default_bus}"
    );
    assert_eq!(
        default_bus["Name"],
        json!(DEFAULT_EVENT_BUS_NAME),
        "a request naming no bus describes the account's default bus, got {default_bus}"
    );

    let mut expected: Vec<String> = cases
        .iter()
        .map(|case| case["Name"].as_str().unwrap_or_default().to_string())
        .chain([DEFAULT_EVENT_BUS_NAME.to_string()])
        .collect();
    expected.sort();
    assert_eq!(
        stored_buses(&emulator),
        expected,
        "expected exactly the created buses plus the default bus in the store"
    );
}

/// The name is the bus's identity: a second create under a name already stored is
/// refused and the stored bus is left exactly as it was. The rows differ along
/// WHICH stored name is reused — the pre-existing `default` bus, a bus this test
/// created, and a name whose only difference is case, which is a DIFFERENT bus.
#[tokio::test]
async fn a_name_already_stored_is_refused_and_the_stored_bus_is_unchanged() {
    let emulator = emulator();
    let first = json!({ "Name": "orders", "Description": "the original" });
    let (status, created) = call(&emulator, "CreateEventBus", &first).await;
    assert_eq!(
        status, 200,
        "CreateEventBus answered {status} {created} for the first create"
    );

    for name in [DEFAULT_EVENT_BUS_NAME, "orders"] {
        let before = stored_buses(&emulator);
        let request = json!({ "Name": name, "Description": "the overwrite that must not happen" });
        let (status, body) = call(&emulator, "CreateEventBus", &request).await;
        assert_eq!(
            status, 400,
            "case Name={name}: a name already stored must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceAlreadyExistsException",
            "case Name={name}: expected ResourceAlreadyExistsException, got {body}"
        );
        assert_eq!(
            stored_buses(&emulator),
            before,
            "case Name={name}: a refused create must not add a bus"
        );
        let (status, described) =
            call(&emulator, "DescribeEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case Name={name}: DescribeEventBus answered {status} {described}"
        );
        let expected = if name == "orders" {
            json!("the original")
        } else {
            Value::Null
        };
        assert_eq!(
            described.get("Description").cloned().unwrap_or(Value::Null),
            expected,
            "case Name={name}: the stored bus must be unchanged, got {described}"
        );
    }

    // A name differing only in case names another bus: the modelled
    // `EventBusName` pattern admits both cases and nothing folds them together.
    let (status, body) = call(&emulator, "CreateEventBus", &json!({ "Name": "Orders" })).await;
    assert_eq!(
        status, 200,
        "case Name=Orders: a name differing only in case is a different bus, got {status} {body}"
    );
    assert_eq!(
        body["EventBusArn"],
        json!(event_bus_arn("Orders")),
        "case Name=Orders: expected the ARN of that name, got {body}"
    );
}

/// A bus no record carries is reported as missing, however it was addressed, and
/// a read never brings one into being.
#[tokio::test]
async fn describing_a_bus_that_was_never_created_is_resource_not_found() {
    let emulator = emulator();
    let cases = [
        ("a bare name", json!({ "Name": "never-created" })),
        (
            "the ARN form of an absent bus",
            json!({ "Name": event_bus_arn("never-created") }),
        ),
        (
            "the ARN form of an absent partner bus, whose name carries the / character",
            json!({ "Name": event_bus_arn("aws.partner/example.com/1234/stream") }),
        ),
        (
            "a name that differs from a stored one only in case",
            json!({ "Name": "Default" }),
        ),
    ];

    for (case, request) in &cases {
        let before = stored_buses(&emulator);
        let (status, body) = call(&emulator, "DescribeEventBus", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            stored_buses(&emulator),
            before,
            "case {case}: a read must not create the bus it did not find"
        );
    }
}

/// A request no bus can be built from is refused with the error for WHY — never a
/// 2xx, and never stored. The rows differ along which rule the request breaks:
/// each modelled bound on `Name`, `Description` and `KmsKeyIdentifier`, each
/// `LogConfig` enum, and a partner event source that does not exist.
#[tokio::test]
async fn a_request_no_bus_can_be_built_from_is_refused() {
    let emulator = emulator();
    let cases = [
        (
            "a name shorter than the modelled minimum",
            json!({ "Name": name_of_length(NAME_MIN - 1) }),
            "ValidationException",
        ),
        (
            "a name past the modelled maximum",
            json!({ "Name": name_of_length(NAME_MAX + 1) }),
            "ValidationException",
        ),
        (
            "a name the modelled pattern does not admit",
            json!({ "Name": "spaces are not allowed" }),
            "ValidationException",
        ),
        (
            "a name carrying a character outside the modelled pattern",
            json!({ "Name": "dollar$sign" }),
            "ValidationException",
        ),
        (
            "a description past the modelled maximum",
            json!({
                "Name": "description-too-long",
                "Description": "d".repeat(DESCRIPTION_MAX + 1),
            }),
            "ValidationException",
        ),
        (
            "a KMS key identifier the modelled pattern does not admit",
            json!({ "Name": "bad-key", "KmsKeyIdentifier": "alias/not valid!" }),
            "ValidationException",
        ),
        (
            "a LogConfig.IncludeDetail the enum does not admit",
            json!({
                "Name": "bad-include-detail",
                "LogConfig": { "IncludeDetail": "PARTIAL", "Level": "INFO" },
            }),
            "ValidationException",
        ),
        (
            "a LogConfig.Level the enum does not admit",
            json!({
                "Name": "bad-level",
                "LogConfig": { "IncludeDetail": "FULL", "Level": "VERBOSE" },
            }),
            "ValidationException",
        ),
        (
            "a custom bus name carrying the / character",
            json!({ "Name": "orders/eu" }),
            "ValidationException",
        ),
        (
            "a partner event source that does not exist",
            json!({
                "Name": "aws.partner/example.com/1234/stream",
                "EventSourceName": "aws.partner/example.com/1234/stream",
            }),
            "ResourceNotFoundException",
        ),
        (
            "no Name at all",
            json!({ "Description": "a bus with no name" }),
            "ValidationException",
        ),
    ];

    for (case, request, expected_code) in &cases {
        let before = stored_buses(&emulator);
        let (status, body) = call(&emulator, "CreateEventBus", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected_code,
            "case {case}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            stored_buses(&emulator),
            before,
            "case {case}: a refused create must store nothing"
        );
    }
}

/// The account's bus quota bounds the store: creates succeed up to the limit and
/// the one past it is refused with the modelled `LimitExceededException`, leaving
/// the store at the limit.
#[tokio::test]
async fn the_account_bus_quota_is_enforced() {
    let emulator = emulator();
    // The store comes up holding the account's `default` bus, so that many more
    // creates reach the quota exactly.
    let room = MAX_EVENT_BUSES - stored_buses(&emulator).len();
    for index in 0..room {
        let request = json!({ "Name": format!("bus-{index}") });
        let (status, body) = call(&emulator, "CreateEventBus", &request).await;
        assert_eq!(
            status, 200,
            "case bus-{index}: a create within the quota answered {status} {body}"
        );
    }
    assert_eq!(
        stored_buses(&emulator).len(),
        MAX_EVENT_BUSES,
        "the store must hold exactly the quota now"
    );

    let (status, body) = call(
        &emulator,
        "CreateEventBus",
        &json!({ "Name": "one-bus-too-many" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a create past the quota must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "expected LimitExceededException past the quota, got {body}"
    );
    assert_eq!(
        stored_buses(&emulator).len(),
        MAX_EVENT_BUSES,
        "a refused create must not grow the store"
    );
}

/// A created bus is the same bus every other operation of this service sees:
/// `PutRule` binds a rule to it and mints an ARN naming it, while the same
/// `PutRule` against a bus that was never created is refused as missing.
#[tokio::test]
async fn a_created_bus_is_the_bus_the_rule_operations_see() {
    let emulator = emulator();
    let cases = ["orders", "orders.eu", "orders_eu-1"];

    for bus in cases {
        let (status, created) = call(&emulator, "CreateEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {bus}: CreateEventBus answered {status} {created}"
        );

        let rule = json!({
            "Name": "on-order",
            "EventBusName": bus,
            "EventPattern": r#"{"source":["com.demo.orders"]}"#,
        });
        let (status, body) = call(&emulator, "PutRule", &rule).await;
        assert_eq!(
            status, 200,
            "case {bus}: PutRule on a created bus answered {status} {body}"
        );
        let expected_arn = format!("arn:aws:events:{REGION}:{}:rule/{bus}/on-order", account());
        assert_eq!(
            body["RuleArn"],
            json!(expected_arn),
            "case {bus}: expected RuleArn {expected_arn}, got {body}"
        );

        // The bus the rule was bound to is the one the request named.
        let (status, described) = call(
            &emulator,
            "DescribeRule",
            &json!({ "Name": "on-order", "EventBusName": bus }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {bus}: DescribeRule answered {status} {described}"
        );
        assert_eq!(
            described["EventBusName"],
            json!(bus),
            "case {bus}: expected the rule on that bus, got {described}"
        );
    }

    let (status, body) = call(
        &emulator,
        "PutRule",
        &json!({
            "Name": "on-order",
            "EventBusName": "never-created",
            "EventPattern": r#"{"source":["com.demo.orders"]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a rule on a bus that was never created must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for an absent bus, got {body}"
    );
}

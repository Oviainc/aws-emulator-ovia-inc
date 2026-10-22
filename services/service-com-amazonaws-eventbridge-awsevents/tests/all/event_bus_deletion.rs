// SPDX-License-Identifier: CC0-1.0

//! `DeleteEventBus` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`), dispatched through the real
//! `MultiServiceRouter` — the same path a client takes: routing,
//! deserialization, the modelled-constraint validator, the handler, the
//! persisted store, serialization.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded
//! shape:
//!
//! * a name a delete has removed is a name no bus carries, so every reader of
//!   the store answers `ResourceNotFoundException` for it and the name is free
//!   to be created again — the teardown/recreate cycle;
//! * a delete removes the named bus and nothing else;
//! * `DeleteEventBus` models no not-found error, so a name no bus carries is a
//!   success that removes nothing (idempotent);
//! * the account's `default` bus cannot be deleted, and a bus whose rules have
//!   not been deleted first is refused — both leaving the bus readable.
//!
//! The wire harness below mirrors the one in the sibling `event_bus_lifecycle`
//! suite, which is this crate's convention: each suite is self-contained.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    ComAmazonawsEventbridgeAwseventsState, DEFAULT_EVENT_BUS_NAME,
};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for. Deliberately NOT the store's
/// fallback region, so an ARN that ignored the request's credential scope would
/// show up as a wrong region segment rather than passing by accident.
const REGION: &str = "eu-west-2";

/// `EventBusName`'s modelled `@length(min: 1, max: 256)` and pattern
/// `^[/\.\-_A-Za-z0-9]+$`, from
/// `smithy_query(query="operation", operation="DeleteEventBus")`.
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 256;

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

/// The `CreateEventBus` requests the delete cases below are built from. The rows
/// differ along the name's form and length and along which optional members the
/// bus carries, because the rule under test — a deleted bus is gone — does not
/// depend on either.
fn creatable_buses() -> Vec<Value> {
    vec![
        json!({ "Name": name_of_length(NAME_MIN) }),
        json!({ "Name": name_of_length(NAME_MAX) }),
        json!({ "Name": "dots.and-dashes_and.9" }),
        json!({ "Name": "Mixed-Case" }),
        json!({ "Name": "with-description", "Description": "a bus to delete" }),
        json!({
            "Name": "with-every-optional-member",
            "Description": "",
            "KmsKeyIdentifier": "alias/events-bus-key",
            "DeadLetterConfig": { "Arn": format!("arn:aws:sqs:{REGION}:{}:dlq", account()) },
            "LogConfig": { "IncludeDetail": "FULL", "Level": "TRACE" },
            "Tags": [{ "Key": "team", "Value": "platform" }],
        }),
    ]
}

/// A deleted bus is gone from every reader of the store, and its name is free
/// again: the create/delete/recreate cycle a teardown performs. One row per bus
/// form, so the rule is asserted over the class and not over one recorded name.
#[tokio::test]
async fn a_deleted_bus_is_gone_and_its_name_is_free_again() {
    let emulator = emulator();

    for case in &creatable_buses() {
        let name = case["Name"].as_str().expect("the case names the bus");
        let label = format!("Name={}", &name[..name.len().min(32)]);

        let (status, created) = call(&emulator, "CreateEventBus", case).await;
        assert_eq!(
            status, 200,
            "case {label}: CreateEventBus answered {status} {created}"
        );
        let (status, described) =
            call(&emulator, "DescribeEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {label}: DescribeEventBus before the delete answered {status} {described}"
        );

        let (status, deleted) = call(&emulator, "DeleteEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {label}: DeleteEventBus answered {status} {deleted}"
        );
        assert!(
            !stored_buses(&emulator).contains(&name.to_string()),
            "case {label}: the deleted bus must be gone from the store, got {:?}",
            stored_buses(&emulator)
        );

        // Every way of addressing the bus now reports it missing — the bare name
        // and the ARN form `EventBusNameOrArn` admits.
        for (how, request) in [
            ("by name", json!({ "Name": name })),
            ("by ARN", json!({ "Name": event_bus_arn(name) })),
        ] {
            let (status, body) = call(&emulator, "DescribeEventBus", &request).await;
            assert_eq!(
                status, 400,
                "case {label} {how}: DescribeEventBus after the delete answered {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label} {how}: expected ResourceNotFoundException, got {body}"
            );
        }

        // A rule write is a reader of the same bus record, so it reports the bus
        // missing too.
        let (status, body) = call(
            &emulator,
            "PutRule",
            &json!({
                "Name": "on-order",
                "EventBusName": name,
                "EventPattern": r#"{"source":["com.demo.orders"]}"#,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: PutRule on the deleted bus answered {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: expected ResourceNotFoundException from PutRule, got {body}"
        );

        // The name is free again: the recreate a teardown/setup cycle performs
        // succeeds, and what it hands back is a NEW record, not the stale one.
        let (status, recreated) = call(&emulator, "CreateEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {label}: CreateEventBus after the delete answered {status} {recreated}"
        );
        let (status, described_again) =
            call(&emulator, "DescribeEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {label}: DescribeEventBus after the recreate answered {status} {described_again}"
        );
        assert_eq!(
            described_again.get("Description").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {label}: the recreated bus must not carry the deleted bus's members, got \
             {described_again}"
        );

        // Leave the store as this row found it, so the next row starts clean.
        let (status, deleted) = call(&emulator, "DeleteEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {label}: deleting the recreated bus answered {status} {deleted}"
        );
    }

    assert_eq!(
        stored_buses(&emulator),
        vec![DEFAULT_EVENT_BUS_NAME.to_string()],
        "every created bus was deleted, so only the account's default bus is left"
    );
}

/// A delete removes the bus it names and NOTHING else: one row per bus deleted
/// out of a set of stored buses whose names overlap in prefix and case.
#[tokio::test]
async fn a_delete_removes_only_the_bus_it_names() {
    let emulator = emulator();
    let names = ["orders", "orders-eu", "Orders", "orders.eu.1", "payments"];
    for name in names {
        let (status, body) = call(&emulator, "CreateEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name}: CreateEventBus answered {status} {body}"
        );
    }

    let mut left: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    left.push(DEFAULT_EVENT_BUS_NAME.to_string());
    left.sort();

    for name in names {
        let (status, body) = call(&emulator, "DeleteEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name}: DeleteEventBus answered {status} {body}"
        );
        left.retain(|stored| stored != name);
        assert_eq!(
            stored_buses(&emulator),
            left,
            "case {name}: exactly that bus must be gone"
        );
        for survivor in &left {
            let (status, body) =
                call(&emulator, "DescribeEventBus", &json!({ "Name": survivor })).await;
            assert_eq!(
                status, 200,
                "case {name}: {survivor} must still be describable, got {status} {body}"
            );
        }
    }
}

/// `DeleteEventBus` models no not-found error (its errors are
/// `ConcurrentModificationException` and `InternalException`, from
/// `smithy_query(query="operation", operation="DeleteEventBus")`), so a name no
/// bus carries is a success that removes nothing — however the name misses.
#[tokio::test]
async fn deleting_a_name_no_bus_carries_succeeds_and_removes_nothing() {
    let emulator = emulator();
    let (status, body) = call(&emulator, "CreateEventBus", &json!({ "Name": "orders" })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");

    let cases = [
        ("a name that was never created", "never-created".to_string()),
        (
            "a name differing from a stored one only in case",
            "Orders".to_string(),
        ),
        (
            "a name at the modelled minimum length",
            name_of_length(NAME_MIN),
        ),
        (
            "a name at the modelled maximum length",
            name_of_length(NAME_MAX),
        ),
        (
            "a partner-style name carrying the / character",
            "aws.partner/example.com/1234/stream".to_string(),
        ),
    ];

    for (case, name) in &cases {
        let before = stored_buses(&emulator);
        let (status, body) = call(&emulator, "DeleteEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case}: DeleteEventBus answered {status} {body}"
        );
        assert_eq!(
            stored_buses(&emulator),
            before,
            "case {case}: a delete that found nothing must remove nothing"
        );
    }

    // Repeating the delete of a bus that WAS stored is the same success.
    for attempt in 1..=3 {
        let (status, body) = call(&emulator, "DeleteEventBus", &json!({ "Name": "orders" })).await;
        assert_eq!(
            status, 200,
            "case attempt {attempt}: repeating the delete answered {status} {body}"
        );
        assert!(
            !stored_buses(&emulator).contains(&"orders".to_string()),
            "case attempt {attempt}: the bus must stay gone"
        );
    }
}

/// A delete the service refuses leaves the bus exactly as it was: the account's
/// `default` bus ("you can't delete your account's default event bus") and a bus
/// whose rules have not been deleted first ("all rules associated with this
/// event bus need to be deleted"). The rows differ along WHY the delete is
/// refused and, for the rules case, along how many rules the bus carries.
#[tokio::test]
async fn a_refused_delete_leaves_the_bus_readable() {
    let emulator = emulator();

    // The account's default bus, which no request can delete.
    let before = stored_buses(&emulator);
    let (status, body) = call(
        &emulator,
        "DeleteEventBus",
        &json!({ "Name": DEFAULT_EVENT_BUS_NAME }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the default bus: expected a refusal, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case the default bus: expected ValidationException, got {body}"
    );
    assert_eq!(
        stored_buses(&emulator),
        before,
        "case the default bus: a refused delete must remove nothing"
    );
    let (status, described) = call(
        &emulator,
        "DescribeEventBus",
        &json!({ "Name": DEFAULT_EVENT_BUS_NAME }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the default bus: it must still be describable, got {status} {described}"
    );

    // A custom bus that still carries rules, one row per rule count.
    for rule_count in [1_usize, 3, 5] {
        let bus = format!("with-{rule_count}-rules");
        let (status, body) = call(&emulator, "CreateEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {bus}: CreateEventBus answered {status} {body}"
        );
        let rules: Vec<String> = (0..rule_count).map(|index| format!("rule-{index}")).collect();
        for rule in &rules {
            let (status, body) = call(
                &emulator,
                "PutRule",
                &json!({
                    "Name": rule,
                    "EventBusName": bus,
                    "EventPattern": r#"{"source":["com.demo.orders"]}"#,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {bus}: PutRule {rule} answered {status} {body}"
            );
        }

        let (status, body) = call(&emulator, "DeleteEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 400,
            "case {bus}: a bus whose rules remain must not be deleted, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {bus}: expected ValidationException, got {body}"
        );
        let (status, described) = call(&emulator, "DescribeEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {bus}: the refused bus must still be describable, got {status} {described}"
        );
        for rule in &rules {
            let (status, body) = call(
                &emulator,
                "DescribeRule",
                &json!({ "Name": rule, "EventBusName": bus }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {bus}: {rule} must still be describable, got {status} {body}"
            );
        }

        // With the rules deleted first, the same delete succeeds.
        for rule in &rules {
            let (status, body) = call(
                &emulator,
                "DeleteRule",
                &json!({ "Name": rule, "EventBusName": bus, "Force": false }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {bus}: DeleteRule {rule} answered {status} {body}"
            );
        }
        let (status, body) = call(&emulator, "DeleteEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {bus}: with its rules gone the bus must delete, got {status} {body}"
        );
        let (status, body) = call(&emulator, "DescribeEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 400,
            "case {bus}: the deleted bus must be gone, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {bus}: expected ResourceNotFoundException, got {body}"
        );
    }
}

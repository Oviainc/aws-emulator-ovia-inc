// SPDX-License-Identifier: CC0-1.0

//! The rule-state control plane and the reverse target index, end to end over
//! the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`)
//! through the real `MultiServiceRouter`, with SQS mounted on the same engine so
//! a delivery is observed where a client observes it: on the queue.
//!
//! The RULES asserted here:
//!
//! * `DisableRule` and `EnableRule` WRITE the rule's `RuleState`, which is the
//!   member the matcher consults: after a disable the rule reads back `DISABLED`
//!   and a published event matching its pattern reaches none of its targets,
//!   after an enable it reads back `ENABLED` and the same event reaches them
//!   again. Both are idempotent;
//! * a rule is addressed by (event bus, name) — "if you omit this, the default
//!   event bus is used" (`EnableRule$EventBusName`) — so a call naming a rule
//!   that bus does not hold is `ResourceNotFoundException`, including a call that
//!   names a rule of a CUSTOM bus without naming the bus;
//! * `ListRuleNamesByTarget` reports the rules of one bus that carry a target
//!   with exactly the given `Arn`, in name order, paged by `Limit`/`NextToken`,
//!   and a bus that does not exist is `ResourceNotFoundException` rather than an
//!   empty listing.
//!
//! Each table spans the class rather than one recorded case: the rows differ
//! along the bus the rule lives on, the `State` it was created with, which
//! operation is called, which target ARN is asked about and how the listing is
//! paged.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The source and detail type the rules below match on.
const SOURCE: &str = "com.demo.orders";
const DETAIL_TYPE: &str = "Order Created";
const DETAIL: &str = r#"{"orderId":"B-2"}"#;

/// EventBridge and SQS on one engine: a rule's target is a queue, and a delivery
/// is asserted on that queue.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router }
}

/// One `awsJson` RPC call against the service whose signing name is
/// `signing_name` — the credential scope is what routes it.
async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
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

async fn events(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await
}

async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// A queue called `name`, carrying the policy a rule's target needs: EventBridge
/// "relies on resource-based policies" for a queue (`PutTargets`), so one that has
/// granted the service nothing receives nothing — the rule
/// `tests/all/queue_target_policy_authorization.rs` is the suite for. Answers its url
/// and the ARN a rule names it by.
async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(status, 200, "CreateQueue answered {status} {body}");
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(status, 200, "GetQueueAttributes answered {status} {body}");
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueArn in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "Policy": crate::target_dispatch::granting_events(&arn) },
        }),
    )
    .await;
    assert_eq!(status, 200, "SetQueueAttributes answered {status} {body}");
    (url, arn)
}

/// Every message body waiting on the queue at `url`, as raw text.
async fn drain(emulator: &Emulator, url: &str) -> Vec<String> {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(status, 200, "ReceiveMessage answered {status} {body}");
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Publish the one event every row below routes onto `bus` (the default bus when
/// `None`).
async fn publish(emulator: &Emulator, case: &str, bus: Option<&str>) {
    let mut entry = json!({ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": DETAIL });
    if let Some(bus) = bus {
        entry["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": [entry] })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
}

/// The `State` `DescribeRule` reports for `rule` on `bus`.
async fn rule_state(emulator: &Emulator, case: &str, bus: Option<&str>, rule: &str) -> String {
    let mut request = json!({ "Name": rule });
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "DescribeRule", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRule answered {status} {body}"
    );
    body["State"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no State in {body}"))
        .to_string()
}

/// Call `operation` for `rule` on `bus` and assert it answered 200 with an empty
/// body — `EnableRule` and `DisableRule` are modelled `Unit` output.
async fn state_call(emulator: &Emulator, case: &str, operation: &str, bus: Option<&str>, rule: &str) {
    let mut request = json!({ "Name": rule });
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} answered {status} {body}"
    );
    assert!(
        body.is_null() || body == json!({}),
        "case {case:?}: {operation} is modelled Unit output, got {body}"
    );
}

/// A disable stops the rule matching and an enable resumes it, whichever bus the
/// rule lives on and whatever `State` it was created with.
#[tokio::test]
async fn disable_and_enable_write_the_rule_state_and_govern_delivery() {
    // (case, the bus the rule lives on, the `State` PutRule was given, the state
    //  the rule reads back with before any enable/disable)
    let rows: [(&str, Option<&str>, Option<&str>, &str); 3] = [
        (
            "the default bus, a rule created ENABLED",
            None,
            Some(state::RULE_STATE_ENABLED),
            state::RULE_STATE_ENABLED,
        ),
        (
            "a custom bus, a rule created with no State at all",
            Some("ops.bus"),
            None,
            state::DEFAULT_RULE_STATE,
        ),
        (
            "a custom bus, a rule created DISABLED",
            Some("paused-bus"),
            Some(state::RULE_STATE_DISABLED),
            state::RULE_STATE_DISABLED,
        ),
    ];

    for (case, bus, created_with, expected_initial) in rows {
        let emulator = emulator();
        if let Some(bus) = bus {
            let (status, body) = events(&emulator, "CreateEventBus", &json!({ "Name": bus })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateEventBus answered {status} {body}"
            );
        }
        let (url, queue_arn) = given_queue(&emulator, "orders-queue").await;

        let rule = "orders";
        let mut put_rule = json!({
            "Name": rule,
            "EventPattern": json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string(),
        });
        if let Some(bus) = bus {
            put_rule["EventBusName"] = json!(bus);
        }
        if let Some(created_with) = created_with {
            put_rule["State"] = json!(created_with);
        }
        let (status, body) = events(&emulator, "PutRule", &put_rule).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutRule answered {status} {body}"
        );
        let mut put_targets = json!({
            "Rule": rule,
            "Targets": [{ "Id": "to-queue", "Arn": queue_arn }],
        });
        if let Some(bus) = bus {
            put_targets["EventBusName"] = json!(bus);
        }
        let (status, body) = events(&emulator, "PutTargets", &put_targets).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutTargets answered {status} {body}"
        );

        let state_now = rule_state(&emulator, case, bus, rule).await;
        assert_eq!(
            state_now, expected_initial,
            "case {case:?}: expected the created rule to read back {expected_initial}, \
             got {state_now:?}"
        );

        // DISABLED: the state is written, and the rule matches nothing.
        state_call(&emulator, case, "DisableRule", bus, rule).await;
        let state_now = rule_state(&emulator, case, bus, rule).await;
        assert_eq!(
            state_now,
            state::RULE_STATE_DISABLED,
            "case {case:?}: expected {} after DisableRule, got {state_now:?}",
            state::RULE_STATE_DISABLED
        );
        publish(&emulator, case, bus).await;
        let delivered = drain(&emulator, &url).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: a disabled rule delivers nothing, got {delivered:?}"
        );

        // Idempotent: a second disable leaves it disabled.
        state_call(&emulator, case, "DisableRule", bus, rule).await;
        let state_now = rule_state(&emulator, case, bus, rule).await;
        assert_eq!(
            state_now,
            state::RULE_STATE_DISABLED,
            "case {case:?}: a second DisableRule leaves the rule disabled, got {state_now:?}"
        );

        // ENABLED: the state is written, and the very same event is delivered.
        state_call(&emulator, case, "EnableRule", bus, rule).await;
        let state_now = rule_state(&emulator, case, bus, rule).await;
        assert_eq!(
            state_now,
            state::RULE_STATE_ENABLED,
            "case {case:?}: expected {} after EnableRule, got {state_now:?}",
            state::RULE_STATE_ENABLED
        );
        publish(&emulator, case, bus).await;
        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: an enabled rule delivers the matched event once, got {delivered:?}"
        );
        assert!(
            delivered[0].contains(SOURCE),
            "case {case:?}: expected the delivered envelope to carry {SOURCE:?}, \
             got {:?}",
            delivered[0]
        );

        // Idempotent in the other direction too.
        state_call(&emulator, case, "EnableRule", bus, rule).await;
        let state_now = rule_state(&emulator, case, bus, rule).await;
        assert_eq!(
            state_now,
            state::RULE_STATE_ENABLED,
            "case {case:?}: a second EnableRule leaves the rule enabled, got {state_now:?}"
        );
    }
}

/// A rule is addressed by (bus, name): a name the named bus does not hold is a
/// missing resource, whichever of the two state operations asks for it.
#[tokio::test]
async fn a_state_call_naming_a_rule_the_bus_does_not_hold_is_not_found() {
    let emulator = emulator();
    let bus = "scoped-bus";
    let (status, body) = events(&emulator, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
    let (status, body) = events(
        &emulator,
        "PutRule",
        &json!({
            "Name": "scoped",
            "EventBusName": bus,
            "EventPattern": json!({ "source": [SOURCE] }).to_string(),
        }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");

    // (case, operation, request, the text the refusal names)
    let rows: [(&str, &str, Value, &str); 6] = [
        (
            "EnableRule for a rule the default bus does not hold",
            "EnableRule",
            json!({ "Name": "absent" }),
            state::DEFAULT_EVENT_BUS_NAME,
        ),
        (
            "DisableRule for a rule the default bus does not hold",
            "DisableRule",
            json!({ "Name": "absent" }),
            state::DEFAULT_EVENT_BUS_NAME,
        ),
        (
            "EnableRule for a custom-bus rule, with no EventBusName",
            "EnableRule",
            json!({ "Name": "scoped" }),
            state::DEFAULT_EVENT_BUS_NAME,
        ),
        (
            "DisableRule for a custom-bus rule, with no EventBusName",
            "DisableRule",
            json!({ "Name": "scoped" }),
            state::DEFAULT_EVENT_BUS_NAME,
        ),
        (
            "EnableRule naming a bus that does not exist",
            "EnableRule",
            json!({ "Name": "scoped", "EventBusName": "ghost-bus" }),
            "ghost-bus",
        ),
        (
            "DisableRule naming a bus that does not exist",
            "DisableRule",
            json!({ "Name": "scoped", "EventBusName": "ghost-bus" }),
            "ghost-bus",
        ),
    ];

    for (case, operation, request, named) in rows {
        let (status, body) = events(&emulator, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(named),
            "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
        );
    }

    // Named with its bus, the very same rule is enabled.
    state_call(
        &emulator,
        "the custom-bus rule named with its bus",
        "EnableRule",
        Some(bus),
        "scoped",
    )
    .await;
}

/// `ListRuleNamesByTarget` is the reverse index: the rules of one bus pointing at
/// exactly one target ARN, in name order and paged.
#[tokio::test]
async fn the_target_index_lists_the_rules_pointing_at_one_arn() {
    let emulator = emulator();
    let account = engine::emulator_account_id();
    let watched = format!("arn:aws:sqs:{REGION}:{account}:watched-queue");
    let other = format!("arn:aws:sqs:{REGION}:{account}:other-queue");
    let unused = format!("arn:aws:sqs:{REGION}:{account}:nobody-points-here");
    let bus = "index-bus";

    let (status, body) = events(&emulator, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");

    // (the rule, the bus it lives on, the ARN its one target points at)
    let given: [(&str, Option<&str>, &String); 4] = [
        ("alpha", None, &watched),
        ("beta", None, &watched),
        ("gamma", None, &other),
        ("delta", Some(bus), &watched),
    ];
    for (rule, on_bus, target) in given {
        let mut put_rule = json!({
            "Name": rule,
            "EventPattern": json!({ "source": [SOURCE] }).to_string(),
        });
        let mut put_targets = json!({
            "Rule": rule,
            "Targets": [{ "Id": "only", "Arn": target }],
        });
        if let Some(on_bus) = on_bus {
            put_rule["EventBusName"] = json!(on_bus);
            put_targets["EventBusName"] = json!(on_bus);
        }
        let (status, body) = events(&emulator, "PutRule", &put_rule).await;
        assert_eq!(status, 200, "PutRule {rule} answered {status} {body}");
        let (status, body) = events(&emulator, "PutTargets", &put_targets).await;
        assert_eq!(status, 200, "PutTargets {rule} answered {status} {body}");
    }

    // (case, request, the names the listing reports, whether it pages on)
    let rows: [(&str, Value, Vec<&str>, bool); 5] = [
        (
            "every default-bus rule pointing at the watched queue",
            json!({ "TargetArn": watched }),
            vec!["alpha", "beta"],
            false,
        ),
        (
            "the one rule pointing at the other queue",
            json!({ "TargetArn": other }),
            vec!["gamma"],
            false,
        ),
        (
            "an ARN no target points at",
            json!({ "TargetArn": unused }),
            vec![],
            false,
        ),
        (
            "the index is scoped to the bus named",
            json!({ "TargetArn": watched, "EventBusName": bus }),
            vec!["delta"],
            false,
        ),
        (
            "a page of one, which pages on",
            json!({ "TargetArn": watched, "Limit": 1 }),
            vec!["alpha"],
            true,
        ),
    ];

    for (case, request, expected, pages_on) in rows {
        let (status, body) = events(&emulator, "ListRuleNamesByTarget", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["RuleNames"],
            json!(expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert_eq!(
            body["NextToken"].is_string(),
            pages_on,
            "case {case:?}: expected a NextToken present={pages_on}, got {body}"
        );
        if !pages_on {
            continue;
        }
        // The next page resumes strictly after the last name of this one.
        let token = body["NextToken"].as_str().unwrap_or_default().to_string();
        let (status, next) = events(
            &emulator,
            "ListRuleNamesByTarget",
            &json!({ "TargetArn": watched, "Limit": 1, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the next page answered {status} {next}"
        );
        assert_eq!(
            next["RuleNames"],
            json!(["beta"]),
            "case {case:?}: expected the next page to carry [\"beta\"], got {next}"
        );
        assert!(
            !next["NextToken"].is_string(),
            "case {case:?}: the last page carries no token, got {next}"
        );
    }

    // A rule belongs to a bus, so a bus that does not exist is a missing
    // resource rather than a listing of nothing.
    let (status, body) = events(
        &emulator,
        "ListRuleNamesByTarget",
        &json!({ "TargetArn": watched, "EventBusName": "ghost-bus" }),
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for a bus that does not exist, got {body}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! Refusing an event pattern the pattern LANGUAGE does not admit, end to end
//! over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`,
//! with SQS mounted on the same engine so the rule a refused update left in
//! place is observed where a client observes it: on the queue its target names.
//!
//! The RULES asserted here:
//!
//! * a pattern whose text uses the language's syntax in a way the language does
//!   not admit is REFUSED — `InvalidEventPatternException`, HTTP 400 — and the
//!   message is `Event pattern is not valid. Reason: <reason>` stated ONCE,
//!   followed by the source location the validator reached in the pattern as
//!   submitted (the position itself is `pattern_refusal_location`'s table),
//!   where the reason names what is wrong in the service's own wording: an
//!   unrecognized match type or numeric comparison, a non-numeric numeric
//!   bound, a match expression carrying more than one key, a non-boolean
//!   `exists`, two wildcard characters in a row, a CIDR mask as wide as its
//!   address family, an empty term list and an empty match expression;
//! * the refusal is the SAME whichever operation was handed the pattern, so
//!   `TestEventPattern` answers the message `PutRule` answers;
//! * a pattern the language DOES admit is accepted, so the refusals above reach
//!   no valid pattern;
//! * a refused update is atomic: the stored rule keeps the pattern it had, and
//!   keeps matching exactly what that pattern matches and nothing else.
//!
//! Each table spans the class rather than one recorded case: the rows differ
//! along which construct is malformed, where in the pattern it sits (a term, a
//! nested operator argument, a whole field), and — for the positional reasons —
//! where in the value the offending character falls, including a value whose
//! earlier characters are multi-byte.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-central-1";

/// The source and detail type the live rule's events carry.
const SOURCE: &str = "com.demo.orders";
const DETAIL_TYPE: &str = "Order Placed";

/// The pattern the live rule holds, and keeps holding across every refused
/// update: a `prefix` term on one field of `detail`.
const LIVE_PATTERN: &str = r#"{"detail":{"k":[{"prefix":"ok"}]}}"#;

/// The emulator: EventBridge and SQS on one engine, because the live rule's
/// target is an SQS queue and its deliveries are asserted on that queue.
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

/// The `message` a failure body carries.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The whole message a refused pattern answers with: the opening the service
/// states once, then the reason.
fn refusal(reason: &str) -> String {
    format!("Event pattern is not valid. Reason: {reason}")
}

/// Assert that `body` is the refusal of `pattern` for `reason`: the opening and
/// the reason stated once, and then — for a reason the pattern READER states — the
/// source location naming the pattern exactly as it was submitted and a position
/// inside it (which character, row by row, is `pattern_refusal_location`'s
/// table). A reason the operand's own parser states, like a CIDR block's, is the
/// whole message.
fn assert_refusal(case: &str, body: &Value, pattern: &str, reason: &str, located: bool) {
    let message = error_message(body);
    if !located {
        assert_eq!(
            message,
            refusal(reason),
            "case {case:?}: expected the refusal to state {:?} and nothing after it",
            refusal(reason)
        );
        return;
    }
    // The patterns here are written on one line, so the position is a column of
    // line 1 — and it names a character of the submitted pattern, or the one just
    // past its end where the reader ran out of text.
    let opening = format!(
        "{}\n at [Source: (String)\"{pattern}\"; line: 1, column: ",
        refusal(reason)
    );
    let column = message
        .strip_prefix(&opening)
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|column| column.parse::<usize>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {opening:?}<column>], got {message:?}")
        });
    assert!(
        (1..=pattern.chars().count() + 1).contains(&column),
        "case {case:?}: column {column} must name a character of the submitted pattern \
         {pattern:?}"
    );
}

/// Every malformed-pattern row: the case, the pattern text, the reason the
/// refusal states, and whether the message carries the source location after it
/// (see [`assert_refusal`]).
///
/// The classes are the ones EventBridge names in its own wording; each is
/// exercised on at least one entry beyond the first, and the positional ones on
/// several positions.
fn malformed_rows() -> Vec<(&'static str, String, String, bool)> {
    let rows: Vec<(&'static str, Value, String, bool)> = vec![
        // A comparison the `numeric` operator set does not hold.
        (
            "numeric with an operator the set does not hold",
            json!({ "detail": { "n": [{ "numeric": ["!=", 1] }] } }),
            "Unrecognized numeric range operator: !=".to_string(),
            true,
        ),
        (
            "numeric with another operator the set does not hold",
            json!({ "detail": { "n": [{ "numeric": ["=~", 1] }] } }),
            "Unrecognized numeric range operator: =~".to_string(),
            true,
        ),
        (
            "numeric whose operator is not even text",
            json!({ "detail": { "n": [{ "numeric": [1, 1] }] } }),
            "Unrecognized numeric range operator: 1".to_string(),
            true,
        ),
        // A bound that is not a number, including a list that stops short of one.
        (
            "numeric with a bound that is text",
            json!({ "detail": { "n": [{ "numeric": [">", "1"] }] } }),
            "Value of > must be numeric".to_string(),
            true,
        ),
        (
            "numeric with a bound that is a boolean",
            json!({ "detail": { "n": [{ "numeric": ["<=", true] }] } }),
            "Value of <= must be numeric".to_string(),
            true,
        ),
        (
            "numeric whose last pair carries no bound at all",
            json!({ "detail": { "n": [{ "numeric": [">", 1, ">="] }] } }),
            "Value of >= must be numeric".to_string(),
            true,
        ),
        // A match expression names ONE match type.
        (
            "a match expression carrying two match types",
            json!({ "detail": { "k": [{ "prefix": "a", "suffix": "b" }] } }),
            "Only one key allowed in match expression".to_string(),
            true,
        ),
        (
            "a match expression carrying three match types",
            json!({ "source": [{ "prefix": "a", "suffix": "b", "wildcard": "c*" }] }),
            "Only one key allowed in match expression".to_string(),
            true,
        ),
        // `exists` compares against presence, so its argument is a boolean.
        (
            "exists given the text of a boolean",
            json!({ "detail": { "k": [{ "exists": "false" }] } }),
            "exists match pattern must be either true or false.".to_string(),
            true,
        ),
        (
            "exists given a number",
            json!({ "detail": { "k": [{ "exists": 1 }] } }),
            "exists match pattern must be either true or false.".to_string(),
            true,
        ),
        // Two wildcard characters in a row ask for nothing one does not.
        (
            "wildcard with two characters in a row inside the value",
            json!({ "detail": { "k": [{ "wildcard": "a**b" }] } }),
            "Consecutive wildcard characters at pos 2".to_string(),
            true,
        ),
        (
            "wildcard that is nothing but two characters in a row",
            json!({ "detail": { "k": [{ "wildcard": "**" }] } }),
            "Consecutive wildcard characters at pos 1".to_string(),
            true,
        ),
        (
            "wildcard whose run falls at the end of a longer value",
            json!({ "detail": { "k": [{ "wildcard": "a*b*c**" }] } }),
            "Consecutive wildcard characters at pos 6".to_string(),
            true,
        ),
        (
            // The position is counted in CHARACTERS, so a multi-byte character
            // before the run counts once.
            "wildcard whose run follows a multi-byte character",
            json!({ "detail": { "k": [{ "wildcard": "é**" }] } }),
            "Consecutive wildcard characters at pos 2".to_string(),
            true,
        ),
        (
            // The same rule wherever a wildcard value is given, including as the
            // argument `anything-but` negates.
            "a wildcard nested inside anything-but",
            json!({ "detail": { "k": [{ "anything-but": { "wildcard": "x**" } }] } }),
            "Consecutive wildcard characters at pos 2".to_string(),
            true,
        ),
        // A mask as wide as the address family names one address, not a block.
        // The block's text is read by the ADDRESS parser, so the reason it gives
        // is the whole message.
        (
            "cidr whose mask is as wide as the IPv4 family",
            json!({ "detail": { "ip": [{ "cidr": "10.0.1.7/32" }] } }),
            "IPv4 mask bits must be < 32".to_string(),
            false,
        ),
        (
            "cidr whose mask is wider than the IPv4 family",
            json!({ "detail": { "ip": [{ "cidr": "10.0.0.0/33" }] } }),
            "IPv4 mask bits must be < 32".to_string(),
            false,
        ),
        (
            "cidr whose mask is as wide as the IPv6 family",
            json!({ "detail": { "ip": [{ "cidr": "2001:db8::1/128" }] } }),
            "IPv6 mask bits must be < 128".to_string(),
            false,
        ),
        // A list with no member lists no value to match.
        (
            "a field whose term list is empty",
            json!({ "detail": { "k": [] } }),
            "Empty arrays are not allowed".to_string(),
            true,
        ),
        (
            "numeric given an empty list",
            json!({ "detail": { "n": [{ "numeric": [] }] } }),
            "Empty arrays are not allowed".to_string(),
            true,
        ),
        (
            "$or given an empty list",
            json!({ "$or": [] }),
            "Empty arrays are not allowed".to_string(),
            true,
        ),
        // An object with no member names no match type, and no field.
        (
            "a match expression with no match type in it",
            json!({ "detail": { "k": [{}] } }),
            "Empty objects are not allowed".to_string(),
            true,
        ),
        (
            "a nested pattern object naming no field",
            json!({ "detail": {} }),
            "Empty objects are not allowed".to_string(),
            true,
        ),
        // A match type the language does not define.
        (
            "a match type spelled the way another language spells prefix",
            json!({ "detail": { "k": [{ "startswith": "ok" }] } }),
            "Unrecognized match type startswith".to_string(),
            true,
        ),
        (
            "an envelope field name used where a match type belongs",
            json!({ "source": [{ "detail": "x" }] }),
            "Unrecognized match type detail".to_string(),
            true,
        ),
    ];
    rows.into_iter()
        .map(|(case, pattern, reason, located)| (case, pattern.to_string(), reason, located))
        .collect()
}

/// Every malformed pattern is refused by `PutRule` with the reason for ITS
/// class, and the message states the opening exactly once.
#[tokio::test]
async fn put_rule_refuses_a_malformed_pattern_with_the_reason_for_its_class() {
    let emulator = emulator();
    for (case, pattern, reason, located) in malformed_rows() {
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({ "Name": "probe", "EventPattern": pattern }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidEventPatternException",
            "case {case:?}: expected InvalidEventPatternException, got {body}"
        );
        assert_refusal(case, &body, &pattern, &reason, located);
    }

    // Nothing was stored by any of them.
    let (status, body) = events(&emulator, "ListRules", &json!({})).await;
    assert_eq!(status, 200, "ListRules answered {status} {body}");
    let stored = body["Rules"].as_array().cloned().unwrap_or_default();
    assert!(
        stored.is_empty(),
        "a refused PutRule stores no rule, got {stored:?}"
    );
}

/// The refusal is the pattern language's, not one operation's: the operation
/// that only TESTS a pattern answers the same message for the same pattern.
#[tokio::test]
async fn test_event_pattern_refuses_a_malformed_pattern_with_the_same_message() {
    let emulator = emulator();
    let event = json!({
        "version": "0",
        "id": "0f1a4b7c-8d2e-4f30-9a51-6b7c8d9e0f12",
        "account": engine::emulator_account_id(),
        "source": SOURCE,
        "time": "2026-01-01T12:00:00Z",
        "region": REGION,
        "resources": [],
        "detail-type": DETAIL_TYPE,
        "detail": { "k": "ok-1", "n": 7, "ip": "10.0.1.7" },
    })
    .to_string();
    for (case, pattern, reason, located) in malformed_rows() {
        let (status, body) = events(
            &emulator,
            "TestEventPattern",
            &json!({ "EventPattern": pattern, "Event": event }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_refusal(case, &body, &pattern, &reason, located);
    }
}

/// A pattern the language DOES admit is accepted, so no refusal above reaches a
/// valid pattern: the same constructs, used the way the language defines them.
#[tokio::test]
async fn a_pattern_the_language_admits_is_accepted() {
    let emulator = emulator();
    let rows: Vec<(&str, Value)> = vec![
        (
            "wildcard with single characters spread through the value",
            json!({ "detail": { "k": [{ "wildcard": "a*b*c" }] } }),
        ),
        (
            "wildcard at both ends",
            json!({ "detail": { "k": [{ "wildcard": "*ok*" }] } }),
        ),
        (
            "a wildcard list nested inside anything-but",
            json!({ "detail": { "k": [{ "anything-but": { "wildcard": ["a*", "b*"] } }] } }),
        ),
        (
            "cidr one bit narrower than the IPv4 family",
            json!({ "detail": { "ip": [{ "cidr": "10.0.1.6/31" }] } }),
        ),
        (
            "cidr covering every IPv4 address",
            json!({ "detail": { "ip": [{ "cidr": "0.0.0.0/0" }] } }),
        ),
        (
            "cidr on an IPv6 block",
            json!({ "detail": { "ip": [{ "cidr": "2001:db8::/32" }] } }),
        ),
        (
            "numeric with two pairs bounding a range",
            json!({ "detail": { "n": [{ "numeric": [">", 1, "<=", 5] }] } }),
        ),
        (
            "numeric with one equality pair",
            json!({ "detail": { "n": [{ "numeric": ["=", 7] }] } }),
        ),
        (
            "exists on either side",
            json!({ "detail": { "k": [{ "exists": true }], "missing": [{ "exists": false }] } }),
        ),
        (
            "prefix asking for a case-folded compare",
            json!({ "detail": { "k": [{ "prefix": { "equals-ignore-case": "OK" } }] } }),
        ),
        (
            "$or over two pattern objects",
            json!({ "$or": [{ "source": [SOURCE] }, { "detail": { "n": [7] } }] }),
        ),
    ];
    for (case, pattern) in rows {
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({ "Name": "accepted", "EventPattern": pattern.to_string() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the pattern to be accepted, got {status} {body}"
        );
    }
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

/// Publish one event per `value` of the live rule's field, and answer the
/// `detail.k` values the queue at `url` received.
async fn delivered_values(emulator: &Emulator, url: &str, values: &[&str]) -> Vec<String> {
    let entries: Vec<Value> = values
        .iter()
        .map(|value| {
            json!({
                "EventBusName": "refusal-bus",
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "k": value }).to_string(),
            })
        })
        .collect();
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": entries })).await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    drain(emulator, url)
        .await
        .iter()
        .map(|text| {
            let parsed: Value = serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("a delivered body is not JSON ({error}): {text:?}"));
            parsed["detail"]["k"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A refused update changes nothing: the stored rule keeps the pattern it had,
/// and keeps delivering exactly what that pattern matches — which is what makes
/// a failed deploy safe.
#[tokio::test]
async fn a_refused_update_leaves_the_live_rule_matching_what_it_matched_before() {
    let emulator = emulator();
    let (status, body) = events(
        &emulator,
        "CreateEventBus",
        &json!({ "Name": "refusal-bus" }),
    )
    .await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");

    let (queue, queue_arn) = given_queue(&emulator, "refusal-queue").await;
    let (status, body) = events(
        &emulator,
        "PutRule",
        &json!({
            "Name": "live",
            "EventBusName": "refusal-bus",
            "EventPattern": LIVE_PATTERN,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let (status, body) = events(
        &emulator,
        "PutTargets",
        &json!({
            "Rule": "live",
            "EventBusName": "refusal-bus",
            "Targets": [{ "Id": "t1", "Arn": queue_arn }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");

    // What the rule matches before any update is attempted.
    let before = delivered_values(&emulator, &queue, &["ok-1", "zz-1"]).await;
    assert_eq!(
        before,
        vec!["ok-1".to_string()],
        "case before any update: the live rule delivers only what its pattern matches, got \
         {before:?}"
    );

    for (case, pattern, reason, located) in malformed_rows() {
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({
                "Name": "live",
                "EventBusName": "refusal-bus",
                "EventPattern": pattern,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the update must be refused, got {status} {body}"
        );
        assert_refusal(case, &body, &pattern, &reason, located);
        let (status, body) = events(
            &emulator,
            "DescribeRule",
            &json!({ "Name": "live", "EventBusName": "refusal-bus" }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: DescribeRule answered {status} {body}");
        assert_eq!(
            body["EventPattern"],
            json!(LIVE_PATTERN),
            "case {case:?}: the refused update must leave the stored pattern in place, got {body}"
        );
        assert_eq!(
            body["State"],
            json!("ENABLED"),
            "case {case:?}: the refused update must leave the rule enabled, got {body}"
        );
        // And the rule is still the one that was there: it delivers the value
        // its own pattern matches, and no other.
        let after = delivered_values(&emulator, &queue, &["ok-2", "zz-2"]).await;
        assert_eq!(
            after,
            vec!["ok-2".to_string()],
            "case {case:?}: after the refused update the live rule still delivers only what its \
             pattern matches, got {after:?}"
        );
    }
}

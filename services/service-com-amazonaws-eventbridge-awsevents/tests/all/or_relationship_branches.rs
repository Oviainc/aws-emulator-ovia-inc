// SPDX-License-Identifier: CC0-1.0

//! What `$or` relates, and what a pattern naming it with too little to relate is
//! refused with — end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`,
//! with SQS mounted on the same engine so the rule a refused update was aimed at
//! is read back where a client reads it: on the queue its target names.
//!
//! The RULE asserted here: `$or` is a RELATIONSHIP between pattern objects, so it
//! relates at least two of them. A pattern whose `$or` holds at least two objects
//! is a valid pattern and matches an event that matches ANY one of them. A `$or`
//! holding fewer relates nothing, so it is not a relationship at all: the
//! validator that compiles a pattern reads the list as the ordinary match TERM
//! list a field's value is — which is what makes a lone object naming no match
//! type `Unrecognized match type <the name it carries>` — and the validator
//! `TestEventPattern` answers from words it for the relationship instead
//! (`There must have at least 2 Objects in $or relationship.`). Both are
//! `InvalidEventPatternException`, HTTP 400.
//!
//! And because such a pattern is REFUSED, the refusal is atomic: a `PutRule` that
//! aims it at a rule that already exists leaves that rule's `EventPattern`
//! byte-identical and its `State` `ENABLED`, and the rule keeps routing exactly
//! what it routed before — while the same pattern under a fresh name leaves no
//! rule at all.
//!
//! The rows differ along what the rule generalises over: how many objects the
//! `$or` relates (none, one, two, three), which member the lone object carries
//! (so the refused name is read from the request rather than fixed), which
//! branch of an accepted relationship the published event matches, what a list
//! too short to relate anything is matched as instead, and whether the pattern
//! object carrying it names another field as well.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The bus every rule below lives on.
const BUS: &str = "or-bus";

/// The source and detail type every event below carries.
const SOURCE: &str = "com.demo.orders";
const DETAIL_TYPE: &str = "Order Placed";

/// The pattern the live rule of the atomicity test carries, and keeps.
const LIVE_PATTERN: &str = r#"{"detail":{"k":[{"prefix":"ok"}]}}"#;

/// The emulator: EventBridge and SQS on one engine, because the rule a refused
/// update is aimed at is observed through its SQS target.
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

/// The error code of a refusal, read off the `awsJson` error envelope.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .or_else(|| body["code"].as_str())
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The message of a refusal.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The whole refusal message for `reason`, as every pattern refusal states it.
fn refusal(reason: &str) -> String {
    format!("Event pattern is not valid. Reason: {reason}")
}

/// Assert that `body` refuses `pattern` for `reason`: the opening and the reason,
/// then the source location naming the pattern as submitted and a position inside
/// it (which character, row by row, is `pattern_refusal_location`'s table).
fn assert_refusal(case: &str, body: &Value, pattern: &str, reason: &str) {
    let message = error_message(body);
    // Every pattern here is written on one line, so the position is a column of
    // line 1, naming a character of the submitted pattern or the one just past
    // its end where the reader ran out of text.
    let opening = format!(
        "{}\n at [Source: (String)\"{pattern}\"; line: 1, column: ",
        refusal(reason)
    );
    let column = message
        .strip_prefix(&opening)
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|column| column.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected {opening:?}<column>], got {message:?}"));
    assert!(
        (1..=pattern.chars().count() + 1).contains(&column),
        "case {case:?}: column {column} must name a character of the submitted pattern \
         {pattern:?}"
    );
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

/// A bus called [`BUS`].
async fn given_bus(emulator: &Emulator) {
    let (status, body) = events(emulator, "CreateEventBus", &json!({ "Name": BUS })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
}

/// Publish one event per `detail` onto [`BUS`], and answer the `detail.k` values
/// the queue at `url` received.
async fn delivered_values(emulator: &Emulator, url: &str, details: &[Value]) -> Vec<String> {
    let entries: Vec<Value> = details
        .iter()
        .map(|detail| {
            json!({
                "EventBusName": BUS,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": detail.to_string(),
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

/// A `$or` relating at least two pattern objects is a valid pattern, and the
/// relationship is what it says: an event matching ANY branch reaches the rule,
/// and one matching none of them reaches nothing.
#[tokio::test]
async fn an_or_relating_two_or_more_objects_matches_any_one_of_them() {
    // Each row: the case, the branches the `$or` relates, and the `detail.k`
    // values of the events that must be delivered out of the three published.
    let rows: [(&str, Value, Vec<&str>); 3] = [
        (
            "two branches, one matched by each of two events",
            json!([
                { "detail": { "k": [{ "prefix": "ok" }] } },
                { "detail": { "n": [{ "numeric": [">", 10] }] } },
            ]),
            vec!["ok-1", "big"],
        ),
        (
            "three branches, the third matched by the last event",
            json!([
                { "detail": { "k": [{ "prefix": "ok" }] } },
                { "detail": { "n": [{ "numeric": [">", 10] }] } },
                { "detail": { "k": ["zz"] } },
            ]),
            vec!["ok-1", "big", "zz"],
        ),
        (
            "two branches neither of which the first event matches",
            json!([
                { "detail": { "n": [{ "numeric": [">", 10] }] } },
                { "detail": { "k": ["zz"] } },
            ]),
            vec!["big", "zz"],
        ),
    ];

    // The three events every row publishes: one matched by the prefix branch, one
    // by the numeric branch, one by the literal branch.
    let published = [
        json!({ "k": "ok-1", "n": 1 }),
        json!({ "k": "big", "n": 11 }),
        json!({ "k": "zz", "n": 1 }),
    ];

    for (case, branches, expected) in rows {
        let emulator = emulator();
        given_bus(&emulator).await;
        let (queue, queue_arn) = given_queue(&emulator, "or-queue").await;
        let pattern = json!({ "$or": branches }).to_string();
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({
                "Name": "or-rule",
                "EventBusName": BUS,
                "EventPattern": pattern,
                "State": "ENABLED",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the pattern to be accepted, got {status} {body}"
        );
        let (status, body) = events(
            &emulator,
            "PutTargets",
            &json!({
                "Rule": "or-rule",
                "EventBusName": BUS,
                "Targets": [{ "Id": "t1", "Arn": queue_arn }],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: PutTargets answered {status} {body}");

        let delivered = delivered_values(&emulator, &queue, &published).await;
        assert_eq!(
            delivered,
            expected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected the events matching a branch to be delivered, got \
             {delivered:?}"
        );
    }
}

/// A `$or` too short to relate anything is the ordinary field the parse read it
/// as — and matching reads it the same way, so what a pattern was ADMITTED as is
/// what it is matched as: the terms are compared against the event's own `$or`
/// field, which no event envelope carries.
#[tokio::test]
async fn an_or_too_short_to_relate_is_matched_as_the_field_the_parse_read_it_as() {
    // Each row: the case, the term list the lone-term `$or` carries, and the
    // `detail.k` values of the three published events that must be delivered.
    // A term list the language admits is a valid pattern whatever field carries
    // it, so these are accepted — and then matched against a field no envelope
    // has, which only an `exists: false` term holds for.
    let rows: [(&str, Value, Vec<&str>); 3] = [
        ("one literal term", json!(["zz"]), vec![]),
        ("one string-operator term", json!([{ "prefix": "ok" }]), vec![]),
        (
            "one exists:false term",
            json!([{ "exists": false }]),
            vec!["ok-1", "big", "zz"],
        ),
    ];

    let published = [
        json!({ "k": "ok-1", "n": 1 }),
        json!({ "k": "big", "n": 11 }),
        json!({ "k": "zz", "n": 1 }),
    ];

    for (case, terms, expected) in rows {
        let emulator = emulator();
        given_bus(&emulator).await;
        let (queue, queue_arn) = given_queue(&emulator, "short-or-queue").await;
        let pattern = json!({ "$or": terms }).to_string();
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({
                "Name": "short-or",
                "EventBusName": BUS,
                "EventPattern": pattern,
                "State": "ENABLED",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a term list the language admits is a valid pattern, got {status} \
             {body}"
        );
        let (status, body) = events(
            &emulator,
            "PutTargets",
            &json!({
                "Rule": "short-or",
                "EventBusName": BUS,
                "Targets": [{ "Id": "t1", "Arn": queue_arn }],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: PutTargets answered {status} {body}");

        let delivered = delivered_values(&emulator, &queue, &published).await;
        assert_eq!(
            delivered,
            expected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected the terms to be matched against the event's own $or field, \
             got {delivered:?}"
        );
    }
}

/// A `$or` relating too little to be a relationship is refused, and the two
/// validators word it as each of them words it.
#[tokio::test]
async fn an_or_relating_fewer_than_two_objects_is_refused_by_both_validators() {
    // Each row: the case, the pattern, the reason `PutRule`'s compiler gives, and
    // the reason `TestEventPattern`'s checker gives.
    let rows: [(&str, Value, String, String); 6] = [
        (
            // The lone object is read as a match term, so the member it carries
            // is read as a match type — and the name comes from the request.
            "one object carrying detail",
            json!({ "$or": [{ "detail": { "k": ["zz"] } }] }),
            "Unrecognized match type detail".to_string(),
            "There must have at least 2 Objects in $or relationship.".to_string(),
        ),
        (
            "one object carrying source",
            json!({ "$or": [{ "source": [SOURCE] }] }),
            "Unrecognized match type source".to_string(),
            "There must have at least 2 Objects in $or relationship.".to_string(),
        ),
        (
            "one object carrying detail-type",
            json!({ "$or": [{ "detail-type": [DETAIL_TYPE] }] }),
            "Unrecognized match type detail-type".to_string(),
            "There must have at least 2 Objects in $or relationship.".to_string(),
        ),
        (
            // A list with no member lists nothing at all, which both validators
            // refuse for the empty list it is before any relationship is read.
            "no objects at all",
            json!({ "$or": [] }),
            "Empty arrays are not allowed".to_string(),
            "Empty arrays are not allowed".to_string(),
        ),
        (
            // A short `$or` whose terms the language DOES admit says nothing
            // about the rest of the pattern: every sibling field of the object
            // carrying it is read too, so a malformed one is still refused.
            "a short $or the compiler admits beside an empty term list",
            json!({ "$or": [{ "prefix": "ok" }], "detail": { "k": [] } }),
            "Empty arrays are not allowed".to_string(),
            "There must have at least 2 Objects in $or relationship.".to_string(),
        ),
        (
            "a short $or the compiler admits beside an unknown match type",
            json!({ "$or": ["zz"], "detail": { "k": [{ "startswith": "z" }] } }),
            "Unrecognized match type startswith".to_string(),
            "There must have at least 2 Objects in $or relationship.".to_string(),
        ),
    ];

    let emulator = emulator();
    let event = json!({
        "version": "0",
        "id": "3a7b1c2d-4e5f-4a6b-8c9d-0e1f2a3b4c5d",
        "account": engine::emulator_account_id(),
        "source": SOURCE,
        "time": "2026-01-01T12:00:00Z",
        "region": REGION,
        "resources": [],
        "detail-type": DETAIL_TYPE,
        "detail": { "k": "ok-1", "n": 7 },
    })
    .to_string();

    for (case, pattern, compiled, checked) in rows {
        let pattern = pattern.to_string();
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({ "Name": "probe", "EventPattern": pattern }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected PutRule to refuse the pattern, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidEventPatternException",
            "case {case:?}: expected InvalidEventPatternException, got {body}"
        );
        assert_refusal(case, &body, &pattern, &compiled);

        let (status, body) = events(
            &emulator,
            "TestEventPattern",
            &json!({ "EventPattern": pattern, "Event": event }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected TestEventPattern to refuse the pattern, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidEventPatternException",
            "case {case:?}: expected InvalidEventPatternException, got {body}"
        );
        assert_refusal(case, &body, &pattern, &checked);
    }

    // None of the refusals above stored a rule.
    let (status, body) = events(&emulator, "ListRules", &json!({})).await;
    assert_eq!(status, 200, "ListRules answered {status} {body}");
    let stored = body["Rules"].as_array().cloned().unwrap_or_default();
    assert!(
        stored.is_empty(),
        "a refused PutRule stores no rule, got {stored:?}"
    );
    let (status, body) = events(&emulator, "DescribeRule", &json!({ "Name": "probe" })).await;
    assert_eq!(
        status, 400,
        "a refused create under a fresh name leaves no rule to describe, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a name no rule was stored under answers ResourceNotFoundException, got {body}"
    );
}

/// The refusal is ATOMIC: a refused update leaves the live rule exactly as it
/// was, and routing exactly what it routed.
#[tokio::test]
async fn a_refused_or_update_leaves_the_live_rule_and_its_routing_in_place() {
    let emulator = emulator();
    given_bus(&emulator).await;
    let (queue, queue_arn) = given_queue(&emulator, "live-queue").await;
    let (status, body) = events(
        &emulator,
        "PutRule",
        &json!({
            "Name": "live",
            "EventBusName": BUS,
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
            "EventBusName": BUS,
            "Targets": [{ "Id": "t1", "Arn": queue_arn }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");

    // The positive control: before any update, the rule delivers what its own
    // pattern matches and nothing else.
    let before = delivered_values(
        &emulator,
        &queue,
        &[json!({ "k": "ok-1" }), json!({ "k": "zz" })],
    )
    .await;
    assert_eq!(
        before,
        vec!["ok-1".to_string()],
        "case before any update: the live rule delivers only what its pattern matches, got \
         {before:?}"
    );

    // Each row aims a `$or` that relates too little at the SAME rule name. The
    // rows differ in how little it relates and in what the lone object carries.
    let rows: [(&str, Value); 3] = [
        (
            "one object carrying detail",
            json!({ "$or": [{ "detail": { "k": ["zz"] } }] }),
        ),
        (
            "one object carrying source",
            json!({ "$or": [{ "source": [SOURCE] }] }),
        ),
        ("no objects at all", json!({ "$or": [] })),
    ];

    for (round, (case, pattern)) in rows.into_iter().enumerate() {
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({
                "Name": "live",
                "EventBusName": BUS,
                "EventPattern": pattern.to_string(),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the update must be refused, got {status} {body}"
        );
        let (status, body) = events(
            &emulator,
            "DescribeRule",
            &json!({ "Name": "live", "EventBusName": BUS }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeRule answered {status} {body}"
        );
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
        // And the rule is still the one that was there: the matching event of
        // this round is delivered and the other is not.
        let matched = format!("ok-{round}");
        let after = delivered_values(
            &emulator,
            &queue,
            &[json!({ "k": matched }), json!({ "k": "zz" })],
        )
        .await;
        assert_eq!(
            after,
            vec![matched.clone()],
            "case {case:?}: after the refused update the live rule still delivers only what its \
             pattern matches, got {after:?}"
        );
    }
}

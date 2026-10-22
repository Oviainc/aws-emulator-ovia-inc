// SPDX-License-Identifier: CC0-1.0

//! The EventBridge rule slice end to end over the wire (`awsJson1_1`, `POST /`
//! with `X-Amz-Target: AWSEvents.<Operation>`), dispatched through the real
//! `MultiServiceRouter` — so every case exercises routing, deserialization, the
//! modelled-constraint validator, the handler, the persisted store and
//! serialization: the same path a client takes.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded
//! shape:
//!
//! * a rule is reported back member for member from what was stored, under an
//!   ARN minted once from the rule's bus, name and the request's region;
//! * `PutRule` is create-or-update keyed by name, so a second call mutates the
//!   stored rule in place and hands back the SAME ARN;
//! * a request no rule can be built from is refused with the error for WHY —
//!   never stored, never a 2xx;
//! * targets are upserted by `Id` and paged in their insertion order behind an
//!   opaque token that resumes where the previous page ended;
//! * a published event is routed by EventBridge's event-pattern language — one
//!   row per operator of that language — and what it was routed to is recorded
//!   in state;
//! * `FailedEntryCount` is counted from per-entry outcomes, index-aligned with
//!   the request's entries;
//! * a rule that still carries targets is not deleted unless `Force` is set,
//!   and a deleted rule is gone from every reader.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    ComAmazonawsEventbridgeAwseventsState, DEFAULT_EVENT_BUS_NAME, MAX_ENTRY_SIZE_BYTES,
    MAX_TARGETS_PER_RULE,
};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for. Deliberately NOT the store's
/// fallback region, so an ARN that ignored the request's credential scope would
/// show up as a wrong region segment rather than passing by accident.
const REGION: &str = "eu-west-2";

/// The `RuleState` values, from
/// `smithy_query(query="shape", shape="RuleState")` — `DISABLED`, `ENABLED`,
/// `ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS`. Spelled out here, not
/// imported, so a wrong set in the implementation cannot make a test pass.
const RULE_STATES: [&str; 3] = [
    "DISABLED",
    "ENABLED",
    "ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS",
];

/// `LimitMax100`'s modelled `@range(min: 1, max: 100)`, from
/// `smithy_query(query="operation", operation="ListRules")`.
const LIMIT_MIN: i64 = 1;
const LIMIT_MAX: i64 = 100;

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
    let credential =
        format!("AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, SignedHeaders=host, Signature=0000");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("authorization", format!("AWS4-HMAC-SHA256 Credential={credential}"))
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

/// `arn:aws:events:<region>:<account>:rule/<name>` — the ARN a rule on the
/// account's default bus carries, built here rather than read from the
/// implementation.
fn default_bus_rule_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:rule/{name}", account())
}

/// A target ARN pointing at some other service's resource.
fn target_arn(resource: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{resource}", account())
}

/// The routing log `PutEvents` appended to, as
/// `(EventId, matched rule names, target ARNs)` per accepted event.
fn delivered(emulator: &Emulator) -> Vec<(String, Vec<String>, Vec<String>)> {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .delivered
        .iter()
        .map(|event| {
            (
                event.event_id.clone(),
                event.matched_rules.clone(),
                event.target_arns.clone(),
            )
        })
        .collect()
}

/// The `Id` of every target `ListTargetsByRule` reports for `rule`, following
/// `NextToken` with a page size of `limit` until the listing is exhausted.
async fn target_ids(emulator: &Emulator, rule: &str, limit: Option<i64>) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({ "Rule": rule });
        if let Some(limit) = limit {
            request["Limit"] = json!(limit);
        }
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(emulator, "ListTargetsByRule", &request).await;
        assert_eq!(
            status, 200,
            "limit {limit:?}: ListTargetsByRule answered {status} {body}"
        );
        for target in body["Targets"].as_array().cloned().unwrap_or_default() {
            ids.push(target["Id"].as_str().unwrap_or_default().to_string());
        }
        next = body["NextToken"].as_str().map(ToString::to_string);
        if next.is_none() {
            return ids;
        }
        assert!(
            ids.len() <= 64,
            "limit {limit:?}: ListTargetsByRule kept issuing NextToken after {} targets",
            ids.len()
        );
    }
}

/// The `Name` of every rule `ListRules` reports for `request`, following
/// `NextToken` until the listing is exhausted.
async fn rule_names(emulator: &Emulator, request: &Value) -> Vec<String> {
    let mut names = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut page = request.clone();
        if let Some(token) = &next {
            page["NextToken"] = json!(token);
        }
        let (status, body) = call(emulator, "ListRules", &page).await;
        assert_eq!(
            status, 200,
            "request {request}: ListRules answered {status} {body}"
        );
        for rule in body["Rules"].as_array().cloned().unwrap_or_default() {
            names.push(rule["Name"].as_str().unwrap_or_default().to_string());
        }
        next = body["NextToken"].as_str().map(ToString::to_string);
        if next.is_none() {
            return names;
        }
        assert!(
            names.len() <= 64,
            "request {request}: ListRules kept issuing NextToken after {} rules",
            names.len()
        );
    }
}

/// `PutRule` stores a rule, and every reporting operation reads that stored
/// rule back: the rows differ along which members a rule carries (a pattern, a
/// schedule, both, a state, the optional description and role), and each
/// member must come back exactly as it went in.
#[tokio::test]
async fn a_stored_rule_is_reported_back_member_for_member() {
    let emulator = emulator();
    let cases = vec![
        json!({ "Name": "pattern-only", "EventPattern": r#"{"source":["com.demo.widgets"]}"# }),
        json!({ "Name": "schedule.only", "ScheduleExpression": "rate(5 minutes)" }),
        json!({
            "Name": "cron_schedule",
            "ScheduleExpression": "cron(0 20 * * ? *)",
            "State": "DISABLED",
        }),
        json!({
            "Name": "everything-at-once",
            "EventPattern": r#"{"detail-type":["Order Created"]}"#,
            "ScheduleExpression": "rate(1 hour)",
            "Description": "a rule carrying every optional member",
            "RoleArn": "arn:aws:iam::000000000000:role/demo-invoke",
            "State": "ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS",
        }),
    ];

    for case in &cases {
        let name = case["Name"].as_str().expect("the case names the rule");
        let (status, body) = call(&emulator, "PutRule", case).await;
        assert_eq!(status, 200, "case {case}: PutRule answered {status} {body}");
        let expected_arn = default_bus_rule_arn(name);
        assert_eq!(
            body["RuleArn"],
            json!(expected_arn),
            "case {case}: expected RuleArn {expected_arn}, got {body}"
        );

        let (status, described) = call(&emulator, "DescribeRule", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeRule answered {status} {described}"
        );
        assert_eq!(
            described["Arn"],
            json!(expected_arn),
            "case {case}: expected Arn {expected_arn}, got {described}"
        );
        assert_eq!(
            described["Name"],
            json!(name),
            "case {case}: expected Name {name}, got {described}"
        );
        assert_eq!(
            described["EventBusName"],
            json!(DEFAULT_EVENT_BUS_NAME),
            "case {case}: a rule that named no bus belongs to the default bus, got {described}"
        );
        assert_eq!(
            described["CreatedBy"],
            json!(account()),
            "case {case}: expected CreatedBy {}, got {described}",
            account()
        );
        // Omitted `State` means ENABLED: "Rules are enabled by default, or
        // based on value of the state" (PutRule).
        let expected_state = case.get("State").cloned().unwrap_or(json!("ENABLED"));
        assert_eq!(
            described["State"], expected_state,
            "case {case}: expected State {expected_state}, got {described}"
        );
        for member in ["EventPattern", "ScheduleExpression", "Description", "RoleArn"] {
            let expected = case.get(member).cloned().unwrap_or(Value::Null);
            let actual = described.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case}: expected {member} {expected}, got {actual}"
            );
        }
    }

    // A read changes nothing a later caller sees.
    let listed_once = rule_names(&emulator, &json!({})).await;
    let listed_twice = rule_names(&emulator, &json!({})).await;
    assert_eq!(
        listed_once, listed_twice,
        "listing rules twice must report the same rules, got {listed_once:?} then {listed_twice:?}"
    );
    let mut expected: Vec<String> = cases
        .iter()
        .map(|case| case["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    expected.sort();
    assert_eq!(
        listed_once, expected,
        "expected every stored rule in name order {expected:?}, got {listed_once:?}"
    );
}

/// A second `PutRule` with a name already stored REPLACES that rule and hands
/// back the ARN minted the first time; it never adds a second rule. The rows
/// differ along which members the update carries, including dropping one the
/// first call set.
#[tokio::test]
async fn put_rule_updates_in_place_and_keeps_the_arn() {
    let emulator = emulator();
    let name = "repeat-me";
    let first = json!({
        "Name": name,
        "EventPattern": r#"{"source":["com.demo.first"]}"#,
        "Description": "the first revision",
        "State": "ENABLED",
    });
    let (status, body) = call(&emulator, "PutRule", &first).await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let arn = body["RuleArn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        arn,
        default_bus_rule_arn(name),
        "expected {}, got {arn}",
        default_bus_rule_arn(name)
    );

    let updates = vec![
        json!({ "Name": name, "EventPattern": r#"{"source":["com.demo.second"]}"# }),
        json!({ "Name": name, "ScheduleExpression": "rate(2 minutes)", "State": "DISABLED" }),
        json!({
            "Name": name,
            "EventPattern": r#"{"detail-type":["Order Shipped"]}"#,
            "Description": "the third revision",
        }),
    ];
    for update in &updates {
        let (status, body) = call(&emulator, "PutRule", update).await;
        assert_eq!(status, 200, "case {update}: PutRule answered {status} {body}");
        assert_eq!(
            body["RuleArn"],
            json!(arn),
            "case {update}: an update must keep the rule's ARN {arn}, got {body}"
        );

        let (_, described) = call(&emulator, "DescribeRule", &json!({ "Name": name })).await;
        // "If you omit arguments in PutRule, the old values for those
        // arguments are not kept" — so an omitted member comes back absent.
        for member in ["EventPattern", "ScheduleExpression", "Description"] {
            let expected = update.get(member).cloned().unwrap_or(Value::Null);
            let actual = described.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {update}: expected {member} {expected} after the update, got {actual}"
            );
        }
        let expected_state = update.get("State").cloned().unwrap_or(json!("ENABLED"));
        assert_eq!(
            described["State"], expected_state,
            "case {update}: expected State {expected_state}, got {described}"
        );

        let names = rule_names(&emulator, &json!({})).await;
        assert_eq!(
            names,
            vec![name.to_string()],
            "case {update}: an update must not add a second rule, got {names:?}"
        );
    }
}

/// A `PutRule` no rule can be built from is refused with the error that says
/// why, and nothing is stored. The rows break one rule each: the
/// at-least-one-trigger requirement, the `RuleState` value set, the schedule
/// expression's form, the bus's existence, and — one row per clause of
/// EventBridge's event-pattern grammar — the pattern itself.
#[tokio::test]
async fn put_rule_refuses_a_request_no_rule_can_be_built_from() {
    let emulator = emulator();
    let cases: Vec<(Value, u16, &str)> = vec![
        // "A rule must contain at least an EventPattern or ScheduleExpression".
        (json!({ "Name": "no-trigger" }), 400, "ValidationException"),
        (
            json!({ "Name": "empty-trigger", "EventPattern": "", "ScheduleExpression": "" }),
            400,
            "ValidationException",
        ),
        (
            json!({ "Name": "bad-state", "ScheduleExpression": "rate(5 minutes)", "State": "PAUSED" }),
            400,
            "ValidationException",
        ),
        (
            json!({ "Name": "bad-schedule", "ScheduleExpression": "every 5 minutes" }),
            400,
            "ValidationException",
        ),
        (
            json!({ "Name": "empty-cron", "ScheduleExpression": "cron()" }),
            400,
            "ValidationException",
        ),
        (
            json!({
                "Name": "ghost-bus",
                "EventPattern": r#"{"source":["com.demo.widgets"]}"#,
                "EventBusName": "no-such-bus",
            }),
            400,
            "ResourceNotFoundException",
        ),
        // Not JSON at all.
        (
            json!({ "Name": "pattern-1", "EventPattern": "not json" }),
            400,
            "InvalidEventPatternException",
        ),
        // JSON, but not an object.
        (
            json!({ "Name": "pattern-2", "EventPattern": "[\"source\"]" }),
            400,
            "InvalidEventPatternException",
        ),
        // An object naming no field matches nothing, so it is not a pattern.
        (
            json!({ "Name": "pattern-3", "EventPattern": "{}" }),
            400,
            "InvalidEventPatternException",
        ),
        // A field's value must be a term list or a nested object.
        (
            json!({ "Name": "pattern-4", "EventPattern": r#"{"source":"com.demo.widgets"}"# }),
            400,
            "InvalidEventPatternException",
        ),
        // An empty term list lists no value to match.
        (
            json!({ "Name": "pattern-5", "EventPattern": r#"{"source":[]}"# }),
            400,
            "InvalidEventPatternException",
        ),
        // An operator the language does not define.
        (
            json!({ "Name": "pattern-6", "EventPattern": r#"{"source":[{"starts-with":"com."}]}"# }),
            400,
            "InvalidEventPatternException",
        ),
        // `prefix` takes a string, not a number.
        (
            json!({ "Name": "pattern-7", "EventPattern": r#"{"source":[{"prefix":5}]}"# }),
            400,
            "InvalidEventPatternException",
        ),
        // `exists` takes a boolean.
        (
            json!({ "Name": "pattern-8", "EventPattern": r#"{"detail":{"id":[{"exists":"yes"}]}}"# }),
            400,
            "InvalidEventPatternException",
        ),
        // `numeric` takes operator/value PAIRS.
        (
            json!({ "Name": "pattern-9", "EventPattern": r#"{"detail":{"n":[{"numeric":[">"]}]}}"# }),
            400,
            "InvalidEventPatternException",
        ),
        // A prefix length wider than the address family admits.
        (
            json!({ "Name": "pattern-10", "EventPattern": r#"{"detail":{"ip":[{"cidr":"10.0.0.0/64"}]}}"# }),
            400,
            "InvalidEventPatternException",
        ),
        // `$or` takes a non-empty list of pattern objects.
        (
            json!({ "Name": "pattern-11", "EventPattern": r#"{"$or":[]}"# }),
            400,
            "InvalidEventPatternException",
        ),
    ];

    for (request, status, code) in &cases {
        let (actual_status, body) = call(&emulator, "PutRule", request).await;
        assert_eq!(
            actual_status, *status,
            "case {request}: expected HTTP {status}, got {actual_status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {request}: expected error code {code}, got {body}"
        );
    }

    let names = rule_names(&emulator, &json!({})).await;
    assert!(
        names.is_empty(),
        "a refused PutRule must store nothing, got {names:?}"
    );
}

/// Every `RuleState` the model admits is accepted and reported back, and only a
/// state that matches events routes one: `DISABLED` means "EventBridge does not
/// match any events against the rule", and a rule carrying only a schedule
/// self-triggers rather than matching a published event.
#[tokio::test]
async fn only_an_enabled_pattern_rule_matches_a_published_event() {
    let pattern = r#"{"source":["com.demo.widgets"]}"#;
    let mut cases: Vec<(Value, bool)> = RULE_STATES
        .iter()
        .map(|state| {
            (
                json!({ "Name": "probe", "EventPattern": pattern, "State": state }),
                *state != "DISABLED",
            )
        })
        .collect();
    // No `State` at all: enabled by default, so it matches.
    cases.push((json!({ "Name": "probe", "EventPattern": pattern }), true));
    // A schedule and no pattern: nothing for a published event to match.
    cases.push((
        json!({ "Name": "probe", "ScheduleExpression": "rate(5 minutes)" }),
        false,
    ));

    for (request, should_match) in &cases {
        let emulator = emulator();
        let (status, body) = call(&emulator, "PutRule", request).await;
        assert_eq!(status, 200, "case {request}: PutRule answered {status} {body}");

        let (status, body) = call(
            &emulator,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": "com.demo.widgets",
                    "DetailType": "Order Created",
                    "Detail": r#"{"orderId":"o-1"}"#,
                }],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {request}: PutEvents answered {status} {body}");

        let log = delivered(&emulator);
        assert_eq!(
            log.len(),
            1,
            "case {request}: expected one recorded event, got {log:?}"
        );
        let expected: Vec<String> = if *should_match {
            vec!["probe".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            log[0].1, expected,
            "case {request}: expected matched rules {expected:?}, got {:?}",
            log[0].1
        );
    }
}

/// One row per clause of EventBridge's event-pattern language: a pattern, an
/// event, and whether that language says the one matches the other. A match is
/// asserted through the ROUTING LOG the handler wrote, so a pattern that
/// matched really did reach the rule's target.
#[tokio::test]
async fn a_published_event_is_routed_by_the_event_pattern_language() {
    // (label, pattern, entry, matches)
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "exact value",
            r#"{"source":["com.demo.widgets"]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "exact value, different value",
            r#"{"source":["com.demo.widgets"]}"#,
            json!({ "Source": "com.demo.gadgets", "DetailType": "Order Created", "Detail": "{}" }),
            false,
        ),
        (
            "term list is a disjunction",
            r#"{"source":["com.demo.gadgets","com.demo.widgets"]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "fields are a conjunction",
            r#"{"source":["com.demo.widgets"],"detail-type":["Order Shipped"]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            false,
        ),
        (
            "prefix",
            r#"{"source":[{"prefix":"com.demo."}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "prefix, case folded",
            r#"{"source":[{"prefix":{"equals-ignore-case":"COM.DEMO."}}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "prefix that does not hold",
            r#"{"source":[{"prefix":"com.other."}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            false,
        ),
        (
            "suffix",
            r#"{"detail-type":[{"suffix":"Created"}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "equals-ignore-case",
            r#"{"detail-type":[{"equals-ignore-case":"order created"}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "wildcard",
            r#"{"source":[{"wildcard":"com.*.widgets"}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "wildcard that does not hold",
            r#"{"source":[{"wildcard":"com.*.gadgets"}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            false,
        ),
        (
            "exists on a nested field",
            r#"{"detail":{"orderId":[{"exists":true}]}}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": r#"{"orderId":"o-77"}"#,
            }),
            true,
        ),
        (
            "exists is false for an absent field",
            r#"{"detail":{"refundId":[{"exists":true}]}}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": r#"{"orderId":"o-77"}"#,
            }),
            false,
        ),
        (
            "exists:false matches an absent field",
            r#"{"detail":{"refundId":[{"exists":false}]}}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": r#"{"orderId":"o-77"}"#,
            }),
            true,
        ),
        (
            "nested objects, two levels deep",
            r#"{"detail":{"order":{"status":["SHIPPED"]}}}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Shipped",
                "Detail": r#"{"order":{"status":"SHIPPED"}}"#,
            }),
            true,
        ),
        (
            "anything-but a single value",
            r#"{"source":[{"anything-but":"com.other.app"}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "anything-but a list that holds the value",
            r#"{"source":[{"anything-but":["com.other.app","com.demo.widgets"]}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            false,
        ),
        (
            "numeric range",
            r#"{"detail":{"amount":[{"numeric":[">",10,"<=",100]}]}}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": r#"{"amount":42}"#,
            }),
            true,
        ),
        (
            "numeric range, below the lower bound",
            r#"{"detail":{"amount":[{"numeric":[">",10,"<=",100]}]}}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": r#"{"amount":5}"#,
            }),
            false,
        ),
        (
            "cidr",
            r#"{"detail":{"sourceIp":[{"cidr":"10.0.0.0/8"}]}}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": r#"{"sourceIp":"10.1.2.3"}"#,
            }),
            true,
        ),
        (
            "cidr, outside the block",
            r#"{"detail":{"sourceIp":[{"cidr":"10.0.0.0/8"}]}}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": r#"{"sourceIp":"192.168.0.1"}"#,
            }),
            false,
        ),
        (
            "$or over branches",
            r#"{"$or":[{"source":["com.other.app"]},{"detail-type":["Order Created"]}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            true,
        ),
        (
            "$or with no branch holding",
            r#"{"$or":[{"source":["com.other.app"]},{"detail-type":["Order Shipped"]}]}"#,
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            false,
        ),
        (
            "a term matches a member of an array-valued field",
            r#"{"resources":[{"prefix":"arn:aws:ec2:"}]}"#,
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": "{}",
                "Resources": ["arn:aws:s3:::demo-bucket", "arn:aws:ec2:eu-west-2:000000000000:instance/i-1"],
            }),
            true,
        ),
    ];

    for (label, pattern, entry, should_match) in &cases {
        let emulator = emulator();
        let rule = "routing-probe";
        let arn = target_arn("routing-probe-queue");
        let (status, body) =
            call(&emulator, "PutRule", &json!({ "Name": rule, "EventPattern": pattern })).await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutRule answered {status} {body}"
        );
        let (status, body) = call(
            &emulator,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": [{ "Id": "t1", "Arn": arn }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutTargets answered {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(0),
            "case {label:?}: expected FailedEntryCount 0, got {body}"
        );

        let (status, body) =
            call(&emulator, "PutEvents", &json!({ "Entries": [entry] })).await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutEvents answered {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(0),
            "case {label:?}: expected FailedEntryCount 0, got {body}"
        );

        let log = delivered(&emulator);
        assert_eq!(
            log.len(),
            1,
            "case {label:?}: expected one recorded event, got {log:?}"
        );
        let (event_id, matched, targets) = &log[0];
        assert_eq!(
            body["Entries"][0]["EventId"],
            json!(event_id),
            "case {label:?}: the EventId reported must be the one recorded ({event_id}), got {body}"
        );
        let expected_rules: Vec<String> = if *should_match {
            vec![rule.to_string()]
        } else {
            Vec::new()
        };
        let expected_targets: Vec<String> = if *should_match {
            vec![arn.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            matched, &expected_rules,
            "case {label:?}: expected matched rules {expected_rules:?}, got {matched:?}"
        );
        assert_eq!(
            targets, &expected_targets,
            "case {label:?}: expected target ARNs {expected_targets:?}, got {targets:?}"
        );
    }
}

/// `FailedEntryCount` is COUNTED from per-entry outcomes and the results are
/// index-aligned with the request's entries: "For each record, the index of the
/// response element is the same as the index in the request array". The rows
/// differ along why an entry is or is not acceptable.
#[tokio::test]
async fn put_events_reports_one_outcome_per_entry() {
    let emulator = emulator();
    let (status, body) = call(
        &emulator,
        "PutRule",
        &json!({ "Name": "catch-all", "EventPattern": r#"{"source":[{"exists":true}]}"# }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");

    // (entry, expected ErrorCode — absent means the entry is accepted)
    let cases: Vec<(Value, Option<&str>)> = vec![
        (
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "{}" }),
            None,
        ),
        (
            json!({ "DetailType": "Order Created", "Detail": "{}" }),
            Some("InvalidArgument"),
        ),
        (
            json!({ "Source": "com.demo.widgets", "Detail": "{}" }),
            Some("InvalidArgument"),
        ),
        (
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "   " }),
            None,
        ),
        (
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "not json" }),
            Some("MalformedDetail"),
        ),
        (
            json!({ "Source": "com.demo.widgets", "DetailType": "Order Created", "Detail": "[1,2]" }),
            Some("MalformedDetail"),
        ),
        (
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": "{}",
                "EventBusName": "no-such-bus",
            }),
            Some("ResourceNotFoundException"),
        ),
        (
            json!({
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": r#"{"amount":1}"#,
                "Resources": ["arn:aws:s3:::demo-bucket"],
                "Time": 1_767_225_600_i64,
            }),
            None,
        ),
    ];

    let entries: Vec<Value> = cases.iter().map(|(entry, _)| entry.clone()).collect();
    let expected_failures = cases.iter().filter(|(_, code)| code.is_some()).count();
    let (status, body) = call(&emulator, "PutEvents", &json!({ "Entries": entries })).await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(expected_failures),
        "expected FailedEntryCount {expected_failures}, got {body}"
    );

    let results = body["Entries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        results.len(),
        cases.len(),
        "expected one result per entry ({}), got {body}",
        cases.len()
    );
    let mut event_ids: Vec<String> = Vec::new();
    for (index, (entry, expected_code)) in cases.iter().enumerate() {
        let result = &results[index];
        let actual_code = result.get("ErrorCode").and_then(Value::as_str);
        assert_eq!(
            actual_code, *expected_code,
            "entry {index} {entry}: expected ErrorCode {expected_code:?}, got {result}"
        );
        match expected_code {
            None => {
                let id = result["EventId"].as_str().unwrap_or_default();
                assert!(
                    !id.is_empty(),
                    "entry {index} {entry}: an accepted entry carries an EventId, got {result}"
                );
                event_ids.push(id.to_string());
            }
            Some(_) => {
                assert!(
                    result.get("EventId").is_none_or(Value::is_null),
                    "entry {index} {entry}: a refused entry carries no EventId, got {result}"
                );
                assert!(
                    result
                        .get("ErrorMessage")
                        .and_then(Value::as_str)
                        .is_some_and(|message| !message.is_empty()),
                    "entry {index} {entry}: a refused entry says why, got {result}"
                );
            }
        }
    }

    let mut unique = event_ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        event_ids.len(),
        "every accepted entry gets its own EventId, got {event_ids:?}"
    );

    // Only accepted entries reach the routing log, and each one is recorded
    // under the EventId the response reported.
    let log = delivered(&emulator);
    let recorded: Vec<String> = log.iter().map(|event| event.0.clone()).collect();
    assert_eq!(
        recorded, event_ids,
        "expected the accepted entries' EventIds {event_ids:?} in the routing log, got {recorded:?}"
    );
}

/// The matcher is bounded in the WORK one request can ask of it, not only in
/// its inputs: a `Detail` close to the published 1MB entry limit, driven
/// against patterns chosen to be adversarial for a backtracking matcher,
/// finishes quickly and answers a DEFINITE verdict — either the entry routed,
/// or it was refused for exceeding the emulator's matching budget. The rows
/// differ along the pattern shape that drives the work.
#[tokio::test]
async fn pattern_matching_stays_bounded_for_an_adversarial_event() {
    // Each row is a shape a naive matcher handles badly: many stars with runs
    // that keep almost matching, long literal runs either side of a star, a
    // case-folded affix (which must not fold the whole leaf), and a list the
    // leaf is compared against member by member.
    let patterns = [
        r#"{"detail":{"pad":[{"wildcard":"*a*a*a*a*a*a*a*a*a*b"}]}}"#,
        r#"{"detail":{"pad":[{"wildcard":"aaaaaaaaaa*aaaaaaaaaa*b"}]}}"#,
        r#"{"detail":{"pad":[{"prefix":{"equals-ignore-case":"AAAAAAAA"}}]}}"#,
        r#"{"detail":{"pad":[{"suffix":{"equals-ignore-case":"AAAAAAAA"}}]}}"#,
        r#"{"detail":{"pad":[{"anything-but":["aaaa","bbbb","cccc"]}]}}"#,
    ];

    let emulator = emulator();
    for (index, pattern) in patterns.iter().enumerate() {
        let (status, body) = call(
            &emulator,
            "PutRule",
            &json!({ "Name": format!("adversarial-{index}"), "EventPattern": pattern }),
        )
        .await;
        assert_eq!(
            status, 200,
            "pattern {pattern}: PutRule answered {status} {body}"
        );
    }

    // A detail just under the published entry limit.
    let padding = 900_000;
    let detail = format!(r#"{{"pad":"{}"}}"#, "a".repeat(padding));
    let started = Instant::now();
    let (status, body) = call(
        &emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": "com.demo.widgets",
                "DetailType": "Order Created",
                "Detail": detail,
            }],
        }),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(status, 200, "PutEvents answered {status}");
    assert!(
        elapsed < Duration::from_secs(5),
        "matching a {padding}-byte detail against {} adversarial patterns took {elapsed:?}",
        patterns.len()
    );
    let code = body["Entries"][0]
        .get("ErrorCode")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    assert!(
        code.is_none() || code.as_deref() == Some("InvalidArgument"),
        "expected the entry to route or to be refused with InvalidArgument, got {code:?}"
    );
}

/// "The total entry size must be less than 1MB" (`PutEvents`), measured the way
/// AWS documents under "Calculating PutEvents event entry size". The rows
/// straddle that published bound: just under it, exactly at it, and past it.
#[tokio::test]
async fn put_events_refuses_an_entry_past_the_published_size_limit() {
    let emulator = emulator();
    let source = "com.demo.widgets";
    let detail_type = "Order Created";
    // The entry's size is len(Source) + len(DetailType) + len(Detail); the
    // detail below is `{"pad":"<pad>"}`, so everything except the padding is:
    let overhead = source.len() + detail_type.len() + r#"{"pad":""}"#.len();

    // (label, padding bytes, expected ErrorCode — absent means accepted)
    let cases: Vec<(&str, usize, Option<&str>)> = vec![
        ("well under the limit", 1_024, None),
        (
            "one byte under the limit",
            MAX_ENTRY_SIZE_BYTES - overhead - 1,
            None,
        ),
        (
            "exactly at the limit",
            MAX_ENTRY_SIZE_BYTES - overhead,
            Some("InvalidArgument"),
        ),
        ("past the limit", MAX_ENTRY_SIZE_BYTES, Some("InvalidArgument")),
    ];

    for (label, padding, expected_code) in &cases {
        let detail = format!(r#"{{"pad":"{}"}}"#, "a".repeat(*padding));
        let (status, body) = call(
            &emulator,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": source,
                    "DetailType": detail_type,
                    "Detail": detail,
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutEvents answered {status}, body member Entries {}",
            body["Entries"].is_array()
        );
        let expected_failures = usize::from(expected_code.is_some());
        assert_eq!(
            body["FailedEntryCount"],
            json!(expected_failures),
            "case {label:?}: expected FailedEntryCount {expected_failures}, got {}",
            body["FailedEntryCount"]
        );
        let actual_code = body["Entries"][0]
            .get("ErrorCode")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        assert_eq!(
            actual_code.as_deref(),
            *expected_code,
            "case {label:?}: expected ErrorCode {expected_code:?}, got {actual_code:?}"
        );
    }

    // Only the entries under the limit reached the routing log.
    let accepted = cases.iter().filter(|(_, _, code)| code.is_none()).count();
    let log = delivered(&emulator);
    assert_eq!(
        log.len(),
        accepted,
        "expected {accepted} recorded events, got {}",
        log.len()
    );
}

/// `PutTargets` upserts by `Id` and `ListTargetsByRule` pages the stored list
/// in insertion order: the rows differ along the page size, and the walk must
/// report the same list however it is cut up.
#[tokio::test]
async fn targets_upsert_by_id_and_page_in_insertion_order() {
    let emulator = emulator();
    let rule = "target-probe";
    let (status, body) = call(
        &emulator,
        "PutRule",
        &json!({ "Name": rule, "EventPattern": r#"{"source":["com.demo.widgets"]}"# }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");

    // A rule with no targets yet reports an EMPTY list, not an absent member:
    // the listing's result is a member the call computed either way.
    let (status, body) = call(&emulator, "ListTargetsByRule", &json!({ "Rule": rule })).await;
    assert_eq!(status, 200, "ListTargetsByRule answered {status} {body}");
    assert_eq!(
        body["Targets"],
        json!([]),
        "a rule with no targets reports Targets [], got {body}"
    );

    let (status, body) = call(
        &emulator,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [
                { "Id": "t1", "Arn": target_arn("queue-one") },
                { "Id": "t2", "Arn": target_arn("queue-two"), "Input": r#"{"k":1}"# },
                { "Id": "t3", "Arn": target_arn("queue-three"), "InputPath": "$.detail" },
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "expected FailedEntryCount 0, got {body}"
    );

    // Re-putting an id REPLACES that target in place, and a new id is appended.
    let replaced = target_arn("queue-two-replaced");
    let (status, body) = call(
        &emulator,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [
                { "Id": "t2", "Arn": replaced, "Input": r#"{"k":2}"# },
                { "Id": "t4", "Arn": target_arn("queue-four") },
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "expected FailedEntryCount 0, got {body}"
    );

    let expected = vec![
        "t1".to_string(),
        "t2".to_string(),
        "t3".to_string(),
        "t4".to_string(),
    ];
    // Every page size, including both modelled bounds, reports the same list.
    for limit in [
        None,
        Some(LIMIT_MIN),
        Some(2),
        Some(3),
        Some(4),
        Some(LIMIT_MAX),
    ] {
        let ids = target_ids(&emulator, rule, limit).await;
        assert_eq!(
            ids, expected,
            "limit {limit:?}: expected {expected:?}, got {ids:?}"
        );
    }

    // The replaced target kept its position and carries the new members.
    let (_, body) = call(&emulator, "ListTargetsByRule", &json!({ "Rule": rule })).await;
    let targets = body["Targets"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        targets[1]["Arn"],
        json!(replaced),
        "expected the re-put target to carry {replaced}, got {body}"
    );
    assert_eq!(
        targets[1]["Input"],
        json!(r#"{"k":2}"#),
        "expected the re-put target's Input to be replaced too, got {body}"
    );
    assert_eq!(
        targets[2]["InputPath"],
        json!("$.detail"),
        "expected a stored target to report every member it was given, got {body}"
    );

    // A limit outside the modelled 1..100 range is refused at the boundary.
    for limit in [LIMIT_MIN - 1, LIMIT_MAX + 1] {
        let (status, body) = call(
            &emulator,
            "ListTargetsByRule",
            &json!({ "Rule": rule, "Limit": limit }),
        )
        .await;
        assert_eq!(
            status, 400,
            "limit {limit}: expected HTTP 400 for a Limit outside 1..100, got {status} {body}"
        );
    }

    // A token this service never issued is a bad request, not an empty page.
    let (status, body) = call(
        &emulator,
        "ListTargetsByRule",
        &json!({ "Rule": rule, "NextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for a forged NextToken, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException for a forged NextToken, got {body}"
    );

    // A target whose Arn is not an ARN is a per-entry failure, and is NOT
    // stored; the entries beside it still land.
    let (status, body) = call(
        &emulator,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [
                { "Id": "t5", "Arn": "queue-five" },
                { "Id": "t2", "Arn": target_arn("queue-two-again") },
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(1),
        "expected FailedEntryCount 1 for one malformed target ARN, got {body}"
    );
    assert_eq!(
        body["FailedEntries"][0]["TargetId"],
        json!("t5"),
        "expected the failed entry to name the target that failed, got {body}"
    );
    let ids = target_ids(&emulator, rule, None).await;
    assert_eq!(
        ids, expected,
        "a failed target entry must not be stored, got {ids:?}"
    );

    // The rule's own quota is checked against the rule, not the request.
    let overflow: Vec<Value> = (0..MAX_TARGETS_PER_RULE)
        .map(|index| json!({ "Id": format!("extra-{index}"), "Arn": target_arn("queue-extra") }))
        .collect();
    let (status, body) = call(
        &emulator,
        "PutTargets",
        &json!({ "Rule": rule, "Targets": overflow }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 past {MAX_TARGETS_PER_RULE} targets, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "expected LimitExceededException past {MAX_TARGETS_PER_RULE} targets, got {body}"
    );

    // `RemoveTargets` counts the ids that named nothing.
    let (status, body) = call(
        &emulator,
        "RemoveTargets",
        &json!({ "Rule": rule, "Ids": ["t2", "nope"] }),
    )
    .await;
    assert_eq!(status, 200, "RemoveTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(1),
        "expected FailedEntryCount 1 for one unknown Id, got {body}"
    );
    assert_eq!(
        body["FailedEntries"][0]["TargetId"],
        json!("nope"),
        "expected the failed entry to name the unknown Id, got {body}"
    );
    let ids = target_ids(&emulator, rule, None).await;
    assert_eq!(
        ids,
        vec!["t1".to_string(), "t3".to_string(), "t4".to_string()],
        "expected the removed target to be gone, got {ids:?}"
    );

    // A well-formed token whose target has since been removed is STALE: it is
    // refused, not silently restarted at page one.
    let (status, body) = call(
        &emulator,
        "ListTargetsByRule",
        &json!({ "Rule": rule, "Limit": 1 }),
    )
    .await;
    assert_eq!(status, 200, "ListTargetsByRule answered {status} {body}");
    let stale = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of one of three targets carries a NextToken: {body}"))
        .to_string();
    let (status, body) = call(
        &emulator,
        "RemoveTargets",
        &json!({ "Rule": rule, "Ids": ["t1"] }),
    )
    .await;
    assert_eq!(status, 200, "RemoveTargets answered {status} {body}");
    let (status, body) = call(
        &emulator,
        "ListTargetsByRule",
        &json!({ "Rule": rule, "Limit": 1, "NextToken": stale }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for a stale NextToken, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException for a stale NextToken, got {body}"
    );

    // A rule nothing stored is a missing resource on every target operation.
    for operation in ["PutTargets", "ListTargetsByRule", "RemoveTargets"] {
        let request = json!({
            "Rule": "ghost-rule",
            "Targets": [{ "Id": "t1", "Arn": target_arn("queue-one") }],
            "Ids": ["t1"],
        });
        let (status, body) = call(&emulator, operation, &request).await;
        assert_eq!(
            status, 400,
            "{operation} on an unknown rule: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{operation} on an unknown rule: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// `ListRules` narrows by `NamePrefix` and pages in name order; the rows differ
/// along the prefix and the page size.
#[tokio::test]
async fn list_rules_filters_by_prefix_and_pages_in_name_order() {
    let emulator = emulator();
    let stored = ["demo-alpha", "demo-beta", "demo-gamma", "other-1"];
    for name in stored {
        let (status, body) = call(
            &emulator,
            "PutRule",
            &json!({ "Name": name, "EventPattern": r#"{"source":["com.demo.widgets"]}"# }),
        )
        .await;
        assert_eq!(status, 200, "rule {name}: PutRule answered {status} {body}");
    }

    // (NamePrefix, expected names in order)
    let cases: Vec<(Option<&str>, Vec<&str>)> = vec![
        (None, vec!["demo-alpha", "demo-beta", "demo-gamma", "other-1"]),
        (Some("demo"), vec!["demo-alpha", "demo-beta", "demo-gamma"]),
        (Some("demo-b"), vec!["demo-beta"]),
        (Some("other"), vec!["other-1"]),
        (Some("zzz"), vec![]),
    ];
    for (prefix, expected) in &cases {
        for limit in [None, Some(LIMIT_MIN), Some(2), Some(LIMIT_MAX)] {
            let mut request = json!({});
            if let Some(prefix) = prefix {
                request["NamePrefix"] = json!(prefix);
            }
            if let Some(limit) = limit {
                request["Limit"] = json!(limit);
            }
            let names = rule_names(&emulator, &request).await;
            let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
            assert_eq!(
                names, expected,
                "prefix {prefix:?} limit {limit:?}: expected {expected:?}, got {names:?}"
            );
        }
    }

    let (status, body) = call(
        &emulator,
        "ListRules",
        &json!({ "NextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for a forged NextToken, got {status} {body}"
    );

    let (status, body) = call(
        &emulator,
        "ListRules",
        &json!({ "EventBusName": "no-such-bus" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 listing an unknown bus, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException listing an unknown bus, got {body}"
    );
}

/// A rule that still carries targets is not deleted; one with none is, and a
/// deleted rule is gone from every reader. The rows differ along the target
/// count and along `Force`, which the model says is ignored for a rule that is
/// not a managed rule — so it never lifts the targets refusal.
#[tokio::test]
async fn deleting_a_rule_removes_it_once_its_targets_are_gone() {
    // (rule name, how many targets it carries, Force, expected HTTP status)
    let cases: Vec<(&str, usize, bool, u16)> = vec![
        ("no-targets", 0, false, 200),
        ("no-targets-forced", 0, true, 200),
        ("has-targets", 2, false, 400),
        ("has-targets-forced", 3, true, 400),
    ];

    for (name, targets, force, expected_status) in &cases {
        let emulator = emulator();
        let (status, body) = call(
            &emulator,
            "PutRule",
            &json!({ "Name": name, "EventPattern": r#"{"source":["com.demo.widgets"]}"# }),
        )
        .await;
        assert_eq!(status, 200, "case {name}: PutRule answered {status} {body}");
        if *targets > 0 {
            let list: Vec<Value> = (0..*targets)
                .map(|index| {
                    json!({ "Id": format!("t{index}"), "Arn": target_arn("delete-probe-queue") })
                })
                .collect();
            let (status, body) = call(
                &emulator,
                "PutTargets",
                &json!({ "Rule": name, "Targets": list }),
            )
            .await;
            assert_eq!(status, 200, "case {name}: PutTargets answered {status} {body}");
        }

        let mut request = json!({ "Name": name });
        if *force {
            request["Force"] = json!(true);
        }
        let (status, body) = call(&emulator, "DeleteRule", &request).await;
        assert_eq!(
            status, *expected_status,
            "case {name}: expected HTTP {expected_status}, got {status} {body}"
        );

        let (describe_status, described) =
            call(&emulator, "DescribeRule", &json!({ "Name": name })).await;
        let names = rule_names(&emulator, &json!({})).await;
        if *expected_status == 200 {
            assert_eq!(
                describe_status, 400,
                "case {name}: a deleted rule must not describe, got {describe_status} {described}"
            );
            assert_eq!(
                error_code(&described),
                "ResourceNotFoundException",
                "case {name}: expected ResourceNotFoundException after the delete, got {described}"
            );
            assert!(
                names.is_empty(),
                "case {name}: a deleted rule must not be listed, got {names:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {name}: expected ValidationException refusing the delete, got {body}"
            );
            assert_eq!(
                describe_status, 200,
                "case {name}: a refused delete leaves the rule in place, got {describe_status} {described}"
            );
            assert_eq!(
                names,
                vec![(*name).to_string()],
                "case {name}: a refused delete leaves the rule listed, got {names:?}"
            );
        }
    }

    // "If you call delete rule multiple times for the same rule, all calls
    // will succeed": the delete is idempotent, so a name the bus does not carry
    // is a success. The rows differ along whether the rule ever existed and
    // along `Force`.
    let emulator = emulator();
    let (status, body) = call(
        &emulator,
        "PutRule",
        &json!({ "Name": "delete-twice", "EventPattern": r#"{"source":["com.demo.widgets"]}"# }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let repeats: Vec<Value> = vec![
        json!({ "Name": "delete-twice" }),
        json!({ "Name": "delete-twice" }),
        json!({ "Name": "delete-twice", "Force": true }),
        json!({ "Name": "never-existed" }),
        json!({ "Name": "never-existed", "Force": true }),
    ];
    for request in &repeats {
        let (status, body) = call(&emulator, "DeleteRule", request).await;
        assert_eq!(
            status, 200,
            "case {request}: a repeated or absent-rule delete must succeed, got {status} {body}"
        );
    }
    let names = rule_names(&emulator, &json!({})).await;
    assert!(
        names.is_empty(),
        "the deleted rule must be gone, got {names:?}"
    );

    // "When you call delete rule for a non-existent custom eventbus,
    // ResourceNotFoundException is returned" — the bus, not the rule, is what
    // this operation reports missing.
    let (status, body) = call(
        &emulator,
        "DeleteRule",
        &json!({ "Name": "anything", "EventBusName": "no-such-bus" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 deleting from an unknown bus, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException deleting from an unknown bus, got {body}"
    );
}

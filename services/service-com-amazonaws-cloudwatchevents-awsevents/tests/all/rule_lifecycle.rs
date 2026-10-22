// SPDX-License-Identifier: CC0-1.0

//! The EventBridge (CloudWatch Events) rule lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! awsJson1_1 deserialization, the handler, the persisted state and serialization — the
//! same path an SDK client takes. The rules under test are the ones the service states
//! for its core resources:
//!
//! * a rule's members are whatever the last `PutRule` stored, and every operation
//!   computes its answer from that one record;
//! * a rule's targets are a list, unique by `Target$Id` and ordered by first insertion;
//! * an event is matched against the enabled rules of the bus it was sent to, by the
//!   event-pattern language, and what `PutEvents` answers is recorded.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchevents_awsevents::state::event_pattern;
use service_com_amazonaws_cloudwatchevents_awsevents::state::ComAmazonawsCloudwatcheventsAwseventsState;
use service_com_amazonaws_cloudwatchevents_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. Its 4th field is what routes a request
/// here, and its region is what every ARN this service mints is built for.
///
/// The field is `cloudwatchevents`, this model's sdkId-derived route key, NOT the
/// `events` signing name its model declares: `CloudWatch Events` is the legacy sdkId of
/// the service now called EventBridge, and the two models publish one identical wire
/// identity, so the `events` key is owned by the EventBridge crate
/// (`services/service-com-amazonaws-eventbridge-awsevents`, the superset model) and this
/// service answers under its own key — see the `OWN_SDK_ID_KEY` note in `src/service.rs`.
/// Routing both to one key is what the router refuses as `500 AmbiguousService`.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/cloudwatchevents/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The canonical account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The bus every account owns, used when a request names none.
const DEFAULT_BUS: &str = "default";

/// `RuleState`'s two values, spelled out here rather than imported so a wrong default in
/// `src/state.rs` fails this suite.
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// The largest page `ListRulesRequest$Limit` / `ListTargetsByRuleRequest$Limit` admit
/// (`LimitMax100`, `@range(min: 1, max: 100)`).
const LIMIT_CEILING: i64 = 100;

/// EventBridge's published quota of rules per event bus, which is the bound this
/// emulator puts on the store. Spelled out here rather than imported so a wrong
/// constant in `src/state.rs` fails.
const RULES_PER_BUS: usize = 300;

/// The documented `PutEvents` entry size limit ("Calculating PutEvents event entry
/// size": 256 KB).
const ENTRY_BYTES: usize = 256 * 1024;

/// One emulator: a router carrying only this service, plus the engine it shares, so a
/// test can read the store the handlers wrote.
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

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn string(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

impl Emulator {
    /// One awsJson1_1 call: `POST /` with `X-Amz-Target: AWSEvents.<Operation>`.
    async fn call(&self, operation: &str, body: Value) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "events.eu-west-1.amazonaws.com")
            .header("authorization", SIGNED_FOR_EU_WEST_1)
            .header("x-amz-target", format!("AWSEvents.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered a body that is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    /// The names of the rules the recorded event `event_id` matched, read out of the
    /// store the handler wrote — the only place `PutEvents`'s matching is observable
    /// while `ListRuleNamesByTarget` and the archive operations are still stubs.
    fn matched_rules(&self, bus: &str, event_id: &str) -> Vec<String> {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        let event = store
            .bus(bus)
            .unwrap_or_else(|| panic!("bus {bus} should hold the recorded events"))
            .events
            .iter()
            .find(|event| event.event_id == event_id)
            .unwrap_or_else(|| panic!("event {event_id} should have been recorded on bus {bus}"));
        event.matched_rules.clone()
    }

    /// How many events the store holds on `bus`.
    fn recorded_event_count(&self, bus: &str) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store.bus(bus).map_or(0, |bus| bus.events.len())
    }

    /// Whether the store still holds the event `event_id` on `bus`.
    fn holds_event(&self, bus: &str, event_id: &str) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store.bus(bus).is_some_and(|bus| {
            bus.events
                .iter()
                .any(|event| event.event_id == event_id)
        })
    }
}

/// A rule carrying a pattern that matches everything from one source, for a case that is
/// about something other than the pattern.
fn pattern_for(source: &str) -> String {
    json!({ "source": [source] }).to_string()
}

// ---------------------------------------------------------------------------
// The happy path, and the round trip through the store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_rule_reads_back_exactly_what_put_rule_stored() {
    let emulator = emulator();

    // Every member the model gives `PutRuleRequest`, so the round trip covers all of
    // them rather than just the required one.
    let created = emulator
        .call(
            "PutRule",
            json!({
                "Name": "demo-rule",
                "EventPattern": r#"{"source":["my.app"]}"#,
                "ScheduleExpression": "rate(5 minutes)",
                "State": ENABLED,
                "Description": "demo",
                "RoleArn": "arn:aws:iam::000000000000:role/events",
                "Tags": [{ "Key": "team", "Value": "platform" }],
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "PutRule: expected 200, got {} {:?}",
        created.status, created.body
    );
    let expected_arn = format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/demo-rule");
    assert_eq!(
        created.string("RuleArn"),
        expected_arn,
        "PutRule: expected RuleArn {expected_arn}, got {:?}",
        created.body
    );

    let described = emulator
        .call("DescribeRule", json!({ "Name": "demo-rule" }))
        .await;
    assert_eq!(
        described.status, 200,
        "DescribeRule: expected 200, got {} {:?}",
        described.status, described.body
    );
    for (member, expected) in [
        ("Name", json!("demo-rule")),
        ("Arn", json!(expected_arn)),
        ("EventPattern", json!(r#"{"source":["my.app"]}"#)),
        ("ScheduleExpression", json!("rate(5 minutes)")),
        ("State", json!(ENABLED)),
        ("Description", json!("demo")),
        (
            "RoleArn",
            json!("arn:aws:iam::000000000000:role/events"),
        ),
        ("EventBusName", json!(DEFAULT_BUS)),
        ("CreatedBy", json!(ACCOUNT)),
    ] {
        assert_eq!(
            described.body[member], expected,
            "DescribeRule {member}: expected {expected:?}, got {:?}",
            described.body[member]
        );
    }
    assert!(
        described.body["ManagedBy"].is_null(),
        "DescribeRule ManagedBy: a rule this account created is not managed, got {:?}",
        described.body["ManagedBy"]
    );

    // ListRules is the verification read for the same record.
    let listed = emulator
        .call("ListRules", json!({ "NamePrefix": "demo" }))
        .await;
    let rules = listed.body["Rules"]
        .as_array()
        .unwrap_or_else(|| panic!("ListRules: expected a Rules list, got {:?}", listed.body));
    assert_eq!(
        rules.len(),
        1,
        "ListRules NamePrefix=demo: expected one rule, got {rules:?}"
    );
    assert_eq!(
        rules[0]["Arn"].as_str().unwrap_or_default(),
        expected_arn,
        "ListRules: expected the stored Arn, got {:?}",
        rules[0]
    );
    assert_eq!(
        rules[0]["EventPattern"], described.body["EventPattern"],
        "ListRules and DescribeRule must report one record, got {:?} and {:?}",
        rules[0]["EventPattern"], described.body["EventPattern"]
    );
}

#[tokio::test]
async fn put_rule_replaces_an_existing_rule_rather_than_duplicating_it() {
    let emulator = emulator();

    // The rows differ along what an update changes: a member's value, a member
    // dropped from the request (which "is replaced with null"), and the state.
    // `PutRuleRequest$Tags` is the documented exception: tags on an update are ignored.
    let updates = [
        (
            "sets every member",
            json!({
                "Name": "upsert",
                "EventPattern": r#"{"source":["one.app"]}"#,
                "State": DISABLED,
                "Description": "first",
                "RoleArn": "arn:aws:iam::000000000000:role/one",
            }),
            json!({
                "EventPattern": r#"{"source":["one.app"]}"#,
                "State": DISABLED,
                "Description": "first",
                "RoleArn": "arn:aws:iam::000000000000:role/one",
                "ScheduleExpression": null,
            }),
        ),
        (
            "changes the pattern and the description",
            json!({
                "Name": "upsert",
                "EventPattern": r#"{"source":["two.app"]}"#,
                "Description": "second",
                "RoleArn": "arn:aws:iam::000000000000:role/two",
            }),
            json!({
                "EventPattern": r#"{"source":["two.app"]}"#,
                // Omitted `State` is the documented default, not the stored DISABLED.
                "State": ENABLED,
                "Description": "second",
                "RoleArn": "arn:aws:iam::000000000000:role/two",
                "ScheduleExpression": null,
            }),
        ),
        (
            "drops the optional members",
            json!({
                "Name": "upsert",
                "ScheduleExpression": "cron(0 20 * * ? *)",
            }),
            json!({
                // "If you omit arguments in PutRule, the old values for those
                // arguments are not kept. Instead, they are replaced with null."
                "EventPattern": null,
                "State": ENABLED,
                "Description": null,
                "RoleArn": null,
                "ScheduleExpression": "cron(0 20 * * ? *)",
            }),
        ),
    ];

    for (case, request, expected) in updates {
        let answer = emulator.call("PutRule", request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected PutRule 200, got {} {:?}",
            answer.status, answer.body
        );

        let described = emulator
            .call("DescribeRule", json!({ "Name": "upsert" }))
            .await;
        for (member, value) in expected
            .as_object()
            .expect("the expectation table rows are objects")
        {
            assert_eq!(
                &described.body[member], value,
                "case {case:?}: DescribeRule {member}: expected {value:?}, got {:?}",
                described.body[member]
            );
        }

        let listed = emulator.call("ListRules", json!({})).await;
        let rules = listed.body["Rules"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            rules.len(),
            1,
            "case {case:?}: an upsert must leave ONE rule, got {rules:?}"
        );
    }
}

#[tokio::test]
async fn an_event_bus_arn_names_the_same_bus_as_its_name() {
    let emulator = emulator();

    // `EventBusNameOrArn` admits either form, so both must reach one bus.
    let by_arn = emulator
        .call(
            "PutRule",
            json!({
                "Name": "by-arn",
                "EventPattern": pattern_for("my.app"),
                "EventBusName": format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/{DEFAULT_BUS}"),
            }),
        )
        .await;
    assert_eq!(
        by_arn.status, 200,
        "PutRule by bus ARN: expected 200, got {} {:?}",
        by_arn.status, by_arn.body
    );
    assert_eq!(
        by_arn.string("RuleArn"),
        format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/by-arn"),
        "a rule on the default bus carries rule/<name>, got {:?}",
        by_arn.body
    );

    let described = emulator
        .call("DescribeRule", json!({ "Name": "by-arn" }))
        .await;
    assert_eq!(
        described.string("EventBusName"),
        DEFAULT_BUS,
        "the bus ARN must resolve to the default bus, got {:?}",
        described.body
    );

    // No other bus exists while `CreateEventBus` is a stub, so a rule on one is
    // refused rather than quietly created.
    let unknown = emulator
        .call(
            "PutRule",
            json!({
                "Name": "on-custom",
                "EventPattern": pattern_for("my.app"),
                "EventBusName": "custom-bus",
            }),
        )
        .await;
    assert_eq!(
        (unknown.status, unknown.error_code()),
        (400, "ResourceNotFoundException"),
        "PutRule on an unknown bus: expected 400 ResourceNotFoundException, got {} {:?}",
        unknown.status,
        unknown.body
    );
}

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn targets_are_unique_by_id_and_keep_their_insertion_order() {
    let emulator = emulator();
    emulator
        .call(
            "PutRule",
            json!({ "Name": "with-targets", "EventPattern": pattern_for("my.app") }),
        )
        .await;

    let queue = |n: u32| format!("arn:aws:sqs:{REGION}:{ACCOUNT}:q{n}");

    // The rows differ along count, order and whether an id is new or already held.
    let batches = [
        (
            "two new targets",
            vec![("t1", queue(1)), ("t2", queue(2))],
            vec![("t1", queue(1)), ("t2", queue(2))],
        ),
        (
            "an existing id is replaced in place",
            vec![("t1", queue(9))],
            vec![("t1", queue(9)), ("t2", queue(2))],
        ),
        (
            "a new id is appended after the held ones",
            vec![("t3", queue(3))],
            vec![("t1", queue(9)), ("t2", queue(2)), ("t3", queue(3))],
        ),
        (
            "a mixed batch replaces and appends in one call",
            vec![("t4", queue(4)), ("t2", queue(8))],
            vec![
                ("t1", queue(9)),
                ("t2", queue(8)),
                ("t3", queue(3)),
                ("t4", queue(4)),
            ],
        ),
    ];

    for (case, batch, expected) in batches {
        let targets: Vec<Value> = batch
            .iter()
            .map(|(id, arn)| json!({ "Id": id, "Arn": arn }))
            .collect();
        let answer = emulator
            .call(
                "PutTargets",
                json!({ "Rule": "with-targets", "Targets": targets }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected PutTargets 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected FailedEntryCount 0, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["FailedEntries"],
            json!([]),
            "case {case:?}: expected no FailedEntries, got {:?}",
            answer.body
        );

        let listed = emulator
            .call("ListTargetsByRule", json!({ "Rule": "with-targets" }))
            .await;
        let actual: Vec<(String, String)> = listed.body["Targets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|target| {
                (
                    target["Id"].as_str().unwrap_or_default().to_string(),
                    target["Arn"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(id, arn)| ((*id).to_string(), arn.clone()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected targets {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn target_members_beyond_the_id_and_arn_survive_the_round_trip() {
    let emulator = emulator();
    emulator
        .call(
            "PutRule",
            json!({ "Name": "rich-target", "EventPattern": pattern_for("my.app") }),
        )
        .await;

    let target = json!({
        "Id": "rich",
        "Arn": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:queue.fifo"),
        "RoleArn": "arn:aws:iam::000000000000:role/events",
        "Input": r#"{"literal":true}"#,
        "SqsParameters": { "MessageGroupId": "group-1" },
        "RetryPolicy": { "MaximumRetryAttempts": 3, "MaximumEventAgeInSeconds": 120 },
        "DeadLetterConfig": { "Arn": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:dlq") },
    });
    let answer = emulator
        .call(
            "PutTargets",
            json!({ "Rule": "rich-target", "Targets": [target.clone()] }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "PutTargets: expected 200, got {} {:?}",
        answer.status, answer.body
    );

    let listed = emulator
        .call("ListTargetsByRule", json!({ "Rule": "rich-target" }))
        .await;
    assert_eq!(
        listed.body["Targets"],
        json!([target]),
        "ListTargetsByRule must report the stored target verbatim, got {:?}",
        listed.body["Targets"]
    );
}

#[tokio::test]
async fn target_and_rule_pages_are_bounded_by_limit_and_resumed_by_next_token() {
    let emulator = emulator();
    emulator
        .call(
            "PutRule",
            json!({ "Name": "paged", "EventPattern": pattern_for("my.app") }),
        )
        .await;
    let targets: Vec<Value> = (1..=5)
        .map(|n| json!({ "Id": format!("t{n}"), "Arn": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:q{n}") }))
        .collect();
    emulator
        .call(
            "PutTargets",
            json!({ "Rule": "paged", "Targets": targets }),
        )
        .await;

    // Limits at the published bounds (1 and 100) and in between, plus the absent
    // Limit, which serves the whole list.
    for limit in [Some(1i64), Some(2), Some(5), Some(LIMIT_CEILING), None] {
        let mut request = json!({ "Rule": "paged" });
        if let Some(limit) = limit {
            request["Limit"] = json!(limit);
        }
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut page_request = request.clone();
            if let Some(token) = &token {
                page_request["NextToken"] = json!(token);
            }
            let answer = emulator.call("ListTargetsByRule", page_request).await;
            assert_eq!(
                answer.status, 200,
                "case Limit={limit:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let page = answer.body["Targets"].as_array().cloned().unwrap_or_default();
            let bound = limit.unwrap_or(LIMIT_CEILING);
            assert!(
                i64::try_from(page.len()).expect("a page fits in i64") <= bound,
                "case Limit={limit:?}: a page must hold at most {bound} targets, got {}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .map(|target| target["Id"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= 6,
                "case Limit={limit:?}: pagination must terminate, saw {pages} pages"
            );
            match answer.body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen,
            vec!["t1", "t2", "t3", "t4", "t5"],
            "case Limit={limit:?}: paging must yield every target once, in order, got {seen:?}"
        );
    }

    // And the tokens it does not: neither a non-numeric one nor a position past
    // the end is a token this operation issued.
    for token in ["not-a-number", "999999"] {
        let answer = emulator
            .call(
                "ListTargetsByRule",
                json!({ "Rule": "paged", "NextToken": token }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "case NextToken={token:?}: expected 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // The same bound on ListRules, which pages a NAME-ordered list and filters it.
    for name in ["pg-b", "pg-a", "other"] {
        emulator
            .call(
                "PutRule",
                json!({ "Name": name, "EventPattern": pattern_for("my.app") }),
            )
            .await;
    }
    let first = emulator
        .call("ListRules", json!({ "NamePrefix": "pg-", "Limit": 1 }))
        .await;
    let names: Vec<String> = first.body["Rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| rule["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["pg-a"],
        "ListRules NamePrefix=pg- Limit=1: expected the first name in order, got {names:?}"
    );
    let token = first.body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated ListRules must carry a NextToken, got {:?}", first.body))
        .to_string();
    let second = emulator
        .call(
            "ListRules",
            json!({ "NamePrefix": "pg-", "Limit": 1, "NextToken": token }),
        )
        .await;
    let names: Vec<String> = second.body["Rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| rule["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["pg-b"],
        "ListRules resumed at the token: expected the next name, got {names:?}"
    );
    assert!(
        second.body["NextToken"].is_null(),
        "the last page of a filtered list carries no NextToken, got {:?}",
        second.body
    );
}

// ---------------------------------------------------------------------------
// PutEvents and the event-pattern language
// ---------------------------------------------------------------------------

/// One event-pattern case: a pattern, an event, and whether the pattern matches it.
struct PatternCase {
    /// The rule this case stores the pattern on; also the case label.
    rule: &'static str,
    pattern: Value,
    /// The `PutEventsRequestEntry` to send.
    entry: Value,
    matches: bool,
}

#[tokio::test]
async fn an_event_matches_the_patterns_of_the_rules_on_its_bus() {
    // The filters in this table are the ones the EventBridge event-pattern reference
    // documents: equality and value lists, nested objects, `prefix`, `suffix`,
    // `equals-ignore-case`, `wildcard`, `numeric`, `exists`, `anything-but`, `cidr`
    // and `$or`. Each is exercised on an input the issue did not cite, and `numeric`
    // on both of its range bounds.
    let cases = [
        PatternCase {
            rule: "exact-source",
            pattern: json!({ "source": ["my.app"] }),
            entry: json!({ "Source": "my.app", "DetailType": "test", "Detail": "{\"k\":1}" }),
            matches: true,
        },
        PatternCase {
            rule: "exact-source-miss",
            pattern: json!({ "source": ["my.app"] }),
            entry: json!({ "Source": "other.app" }),
            matches: false,
        },
        PatternCase {
            rule: "source-value-list",
            pattern: json!({ "source": ["a.one", "a.two", "a.three"] }),
            entry: json!({ "Source": "a.two" }),
            matches: true,
        },
        PatternCase {
            rule: "two-fields-are-anded",
            pattern: json!({ "source": ["a.one"], "detail-type": ["placed"] }),
            entry: json!({ "Source": "a.one", "DetailType": "cancelled" }),
            matches: false,
        },
        PatternCase {
            rule: "nested-detail",
            pattern: json!({ "detail": { "state": ["RUNNING"] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"state\":\"RUNNING\"}" }),
            matches: true,
        },
        PatternCase {
            rule: "nested-detail-miss",
            pattern: json!({ "detail": { "state": ["RUNNING"] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"state\":\"STOPPED\"}" }),
            matches: false,
        },
        PatternCase {
            rule: "prefix",
            pattern: json!({ "source": [{ "prefix": "aws.ec" }] }),
            entry: json!({ "Source": "aws.ec2" }),
            matches: true,
        },
        PatternCase {
            rule: "prefix-ignoring-case",
            pattern: json!({ "source": [{ "prefix": { "equals-ignore-case": "AWS.EC" } }] }),
            entry: json!({ "Source": "aws.ec2" }),
            matches: true,
        },
        PatternCase {
            rule: "suffix",
            pattern: json!({ "detail-type": [{ "suffix": "Notification" }] }),
            entry: json!({ "Source": "a.one", "DetailType": "Budget Notification" }),
            matches: true,
        },
        PatternCase {
            rule: "equals-ignoring-case",
            pattern: json!({ "detail-type": [{ "equals-ignore-case": "budget notification" }] }),
            entry: json!({ "Source": "a.one", "DetailType": "Budget Notification" }),
            matches: true,
        },
        PatternCase {
            rule: "wildcard",
            pattern: json!({ "source": [{ "wildcard": "aws.*.service" }] }),
            entry: json!({ "Source": "aws.inner.service" }),
            matches: true,
        },
        PatternCase {
            rule: "wildcard-miss",
            pattern: json!({ "source": [{ "wildcard": "aws.*.service" }] }),
            entry: json!({ "Source": "aws.service" }),
            matches: false,
        },
        PatternCase {
            rule: "wildcard-around-an-inner-run",
            pattern: json!({ "source": [{ "wildcard": "*inner*" }] }),
            entry: json!({ "Source": "aws.inner.service" }),
            matches: true,
        },
        PatternCase {
            rule: "wildcard-with-several-stars",
            pattern: json!({ "source": [{ "wildcard": "aws.*ser*ce" }] }),
            entry: json!({ "Source": "aws.inner.service" }),
            matches: true,
        },
        PatternCase {
            rule: "wildcard-whose-inner-run-is-absent",
            pattern: json!({ "source": [{ "wildcard": "aws.*absent*service" }] }),
            entry: json!({ "Source": "aws.inner.service" }),
            matches: false,
        },
        PatternCase {
            rule: "numeric-at-the-lower-bound",
            pattern: json!({ "detail": { "count": [{ "numeric": [">=", 1, "<=", 5] }] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"count\":1}" }),
            matches: true,
        },
        PatternCase {
            rule: "numeric-at-the-upper-bound",
            pattern: json!({ "detail": { "count": [{ "numeric": [">=", 1, "<=", 5] }] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"count\":5}" }),
            matches: true,
        },
        PatternCase {
            rule: "numeric-past-the-upper-bound",
            pattern: json!({ "detail": { "count": [{ "numeric": [">=", 1, "<=", 5] }] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"count\":6}" }),
            matches: false,
        },
        PatternCase {
            rule: "numbers-compare-by-value",
            pattern: json!({ "detail": { "count": [3] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"count\":3.0}" }),
            matches: true,
        },
        PatternCase {
            rule: "exists-true",
            pattern: json!({ "detail": { "owner": [{ "exists": true }] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"owner\":\"ops\"}" }),
            matches: true,
        },
        PatternCase {
            rule: "exists-false-on-an-absent-field",
            pattern: json!({ "detail": { "owner": [{ "exists": false }] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"other\":\"ops\"}" }),
            matches: true,
        },
        PatternCase {
            rule: "exists-false-on-a-present-field",
            pattern: json!({ "detail": { "owner": [{ "exists": false }] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"owner\":\"ops\"}" }),
            matches: false,
        },
        PatternCase {
            rule: "anything-but-a-value-list",
            pattern: json!({ "source": [{ "anything-but": ["a.one", "a.two"] }] }),
            entry: json!({ "Source": "a.three" }),
            matches: true,
        },
        PatternCase {
            rule: "anything-but-a-prefix",
            pattern: json!({ "source": [{ "anything-but": { "prefix": "aws." } }] }),
            entry: json!({ "Source": "aws.ec2" }),
            matches: false,
        },
        PatternCase {
            rule: "cidr-inside-the-block",
            pattern: json!({ "detail": { "ip": [{ "cidr": "10.0.0.0/8" }] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"ip\":\"10.1.2.3\"}" }),
            matches: true,
        },
        PatternCase {
            rule: "cidr-outside-the-block",
            pattern: json!({ "detail": { "ip": [{ "cidr": "10.0.0.0/8" }] } }),
            entry: json!({ "Source": "a.one", "Detail": "{\"ip\":\"11.1.2.3\"}" }),
            matches: false,
        },
        PatternCase {
            rule: "or-takes-either-branch",
            pattern: json!({ "$or": [{ "source": ["x.one"] }, { "detail-type": ["two"] }] }),
            entry: json!({ "Source": "nothing.like.it", "DetailType": "two" }),
            matches: true,
        },
        PatternCase {
            rule: "or-takes-neither-branch",
            pattern: json!({ "$or": [{ "source": ["x.one"] }, { "detail-type": ["two"] }] }),
            entry: json!({ "Source": "nothing.like.it", "DetailType": "three" }),
            matches: false,
        },
        PatternCase {
            rule: "an-event-list-field-matches-any-of-its-values",
            pattern: json!({ "resources": ["arn:aws:s3:::bucket-two"] }),
            entry: json!({
                "Source": "a.one",
                "Resources": ["arn:aws:s3:::bucket-one", "arn:aws:s3:::bucket-two"],
            }),
            matches: true,
        },
        PatternCase {
            rule: "the-envelope-carries-the-account-and-region",
            pattern: json!({ "account": [ACCOUNT], "region": [REGION] }),
            entry: json!({ "Source": "a.one" }),
            matches: true,
        },
    ];

    let emulator = emulator();
    for case in &cases {
        let stored = emulator
            .call(
                "PutRule",
                json!({ "Name": case.rule, "EventPattern": case.pattern.to_string() }),
            )
            .await;
        assert_eq!(
            stored.status, 200,
            "case {:?}: PutRule must accept the pattern {}, got {} {:?}",
            case.rule, case.pattern, stored.status, stored.body
        );
    }

    for case in &cases {
        let answer = emulator
            .call("PutEvents", json!({ "Entries": [case.entry] }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected PutEvents 200, got {} {:?}",
            case.rule, answer.status, answer.body
        );
        assert_eq!(
            answer.body["FailedEntryCount"],
            json!(0),
            "case {:?}: expected FailedEntryCount 0, got {:?}",
            case.rule,
            answer.body
        );
        let event_id = answer.body["Entries"][0]["EventId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: an accepted entry carries an EventId, got {:?}",
                    case.rule, answer.body
                )
            })
            .to_string();

        let matched = emulator.matched_rules(DEFAULT_BUS, &event_id);
        assert_eq!(
            matched.contains(&case.rule.to_string()),
            case.matches,
            "case {:?}: expected the pattern {} to match {:?} = {}, matched rules were {matched:?}",
            case.rule,
            case.pattern,
            case.entry,
            case.matches
        );
    }
}

#[tokio::test]
async fn a_disabled_rule_matches_nothing() {
    let emulator = emulator();
    for (name, state) in [("on", ENABLED), ("off", DISABLED)] {
        emulator
            .call(
                "PutRule",
                json!({
                    "Name": name,
                    "EventPattern": json!({ "source": ["my.app"] }).to_string(),
                    "State": state,
                }),
            )
            .await;
    }

    let answer = emulator
        .call("PutEvents", json!({ "Entries": [{ "Source": "my.app" }] }))
        .await;
    let event_id = answer.body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let matched = emulator.matched_rules(DEFAULT_BUS, &event_id);
    assert_eq!(
        matched,
        vec!["on".to_string()],
        "only the ENABLED rule may match, matched {matched:?}"
    );

    // Enabling it through PutRule brings it back into the match set, so the match is
    // read from the stored state rather than decided once at create.
    emulator
        .call(
            "PutRule",
            json!({
                "Name": "off",
                "EventPattern": json!({ "source": ["my.app"] }).to_string(),
                "State": ENABLED,
            }),
        )
        .await;
    let answer = emulator
        .call("PutEvents", json!({ "Entries": [{ "Source": "my.app" }] }))
        .await;
    let event_id = answer.body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let matched = emulator.matched_rules(DEFAULT_BUS, &event_id);
    assert_eq!(
        matched,
        vec!["off".to_string(), "on".to_string()],
        "a re-enabled rule must match again, matched {matched:?}"
    );
}

#[tokio::test]
async fn put_events_reports_one_unique_id_per_accepted_entry_and_counts_the_rest() {
    let emulator = emulator();

    // `PutEventsRequest$Entries` is `@length(min: 1, max: 10)`: a full batch is the
    // published upper bound.
    let batch: Vec<Value> = (1..=10)
        .map(|n| json!({ "Source": format!("app.{n}"), "DetailType": "t", "Detail": "{}" }))
        .collect();
    let answer = emulator.call("PutEvents", json!({ "Entries": batch })).await;
    assert_eq!(
        answer.body["FailedEntryCount"],
        json!(0),
        "a batch of ten well-formed entries fails none, got {:?}",
        answer.body
    );
    let ids: Vec<String> = answer.body["Entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["EventId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids.len(),
        10,
        "expected ten result entries, got {:?}",
        answer.body
    );
    let unique: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every EventId must be unique, got {ids:?}"
    );
    assert!(
        ids.iter().all(|id| id.len() == 36 && id.matches('-').count() == 4),
        "an EventId is a UUID, got {ids:?}"
    );
    assert_eq!(
        emulator.recorded_event_count(DEFAULT_BUS),
        10,
        "every accepted entry must be recorded, store holds {}",
        emulator.recorded_event_count(DEFAULT_BUS)
    );

    // A mixed batch: the rows differ along WHY an entry is refused, and the count is
    // the real number of refused rows rather than a constant.
    let answer = emulator
        .call(
            "PutEvents",
            json!({
                "Entries": [
                    { "Source": "app.ok", "Detail": "{\"k\":1}" },
                    { "Source": "app.bad", "Detail": "not json at all" },
                    { "Source": "app.bad", "Detail": "[1,2,3]" },
                    { "Source": "app.ok", "EventBusName": "no-such-bus" },
                ],
            }),
        )
        .await;
    assert_eq!(
        answer.body["FailedEntryCount"],
        json!(3),
        "three of four entries are refused, got {:?}",
        answer.body
    );
    let entries = answer.body["Entries"].as_array().cloned().unwrap_or_default();
    let codes: Vec<String> = entries
        .iter()
        .map(|entry| entry["ErrorCode"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        codes,
        vec![
            String::new(),
            "InvalidArgument".to_string(),
            "InvalidArgument".to_string(),
            "ResourceNotFoundException".to_string(),
        ],
        "each refused row names its own reason, got {codes:?}"
    );
    assert!(
        entries[0]["EventId"].is_string(),
        "the accepted row still carries an EventId, got {:?}",
        entries[0]
    );
    assert!(
        entries[1]["EventId"].is_null(),
        "a refused row carries no EventId, got {:?}",
        entries[1]
    );
    assert_eq!(
        emulator.recorded_event_count(DEFAULT_BUS),
        11,
        "only the accepted row is recorded, store holds {}",
        emulator.recorded_event_count(DEFAULT_BUS)
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_rule_refuses_a_rule_it_cannot_honour() {
    let emulator = emulator();

    // Rows differ along WHICH rule of the operation is broken: the pattern/schedule
    // pair, the pattern's JSON, the pattern's own grammar, and the schedule's form.
    let cases: [(&str, Value, u16, &str); 12] = [
        (
            "neither a pattern nor a schedule",
            json!({ "Name": "no-trigger" }),
            400,
            "ValidationException",
        ),
        (
            "a pattern that is not JSON",
            json!({ "Name": "bad-json", "EventPattern": r#"{"source":["#}),
            400,
            "InvalidEventPatternException",
        ),
        (
            "a pattern that is not an object",
            json!({ "Name": "not-an-object", "EventPattern": r#"["source"]"# }),
            400,
            "InvalidEventPatternException",
        ),
        (
            "a field holding a bare scalar",
            json!({ "Name": "bare-scalar", "EventPattern": r#"{"source":"my.app"}"# }),
            400,
            "InvalidEventPatternException",
        ),
        (
            "an unknown matching operator",
            json!({ "Name": "bad-operator", "EventPattern": r#"{"source":[{"startsWith":"my."}]}"# }),
            400,
            "InvalidEventPatternException",
        ),
        (
            "consecutive wildcard characters",
            json!({ "Name": "double-star", "EventPattern": r#"{"source":[{"wildcard":"a**b"}]}"# }),
            400,
            "InvalidEventPatternException",
        ),
        (
            "a numeric comparison with no value",
            json!({ "Name": "lone-operator", "EventPattern": r#"{"detail":{"n":[{"numeric":[">"]}]}}"# }),
            400,
            "InvalidEventPatternException",
        ),
        (
            "a rate expression with a zero value",
            json!({ "Name": "zero-rate", "ScheduleExpression": "rate(0 minutes)" }),
            400,
            "ValidationException",
        ),
        (
            "a rate expression with an unknown unit",
            json!({ "Name": "bad-unit", "ScheduleExpression": "rate(5 fortnights)" }),
            400,
            "ValidationException",
        ),
        (
            "a rate expression with a signed value",
            json!({ "Name": "signed-rate", "ScheduleExpression": "rate(+5 minutes)" }),
            400,
            "ValidationException",
        ),
        (
            "a schedule in neither documented form",
            json!({ "Name": "prose", "ScheduleExpression": "every five minutes" }),
            400,
            "ValidationException",
        ),
        (
            "a cron expression with the wrong field count",
            json!({ "Name": "short-cron", "ScheduleExpression": "cron(0 20 * * ?)" }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, status, code) in cases {
        let answer = emulator.call("PutRule", request).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // And the forms it must accept, including both documented shapes of a schedule.
    for (case, request) in [
        (
            "a rate schedule",
            json!({ "Name": "rate-rule", "ScheduleExpression": "rate(1 minute)" }),
        ),
        (
            "a cron schedule",
            json!({ "Name": "cron-rule", "ScheduleExpression": "cron(0 20 * * ? *)" }),
        ),
        (
            "a pattern and a schedule together",
            json!({
                "Name": "both-rule",
                "ScheduleExpression": "rate(7 days)",
                "EventPattern": r#"{"source":["my.app"]}"#,
            }),
        ),
    ] {
        let answer = emulator.call("PutRule", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    let listed = emulator.call("ListRules", json!({})).await;
    let names: Vec<String> = listed.body["Rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| rule["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["both-rule", "cron-rule", "rate-rule"],
        "a refused PutRule must store nothing, stored rules are {names:?}"
    );
}

#[tokio::test]
async fn an_operation_naming_no_stored_rule_answers_resource_not_found() {
    let emulator = emulator();
    let cases: [(&str, &str, Value); 4] = [
        (
            "DescribeRule on an unknown name",
            "DescribeRule",
            json!({ "Name": "no-such-rule" }),
        ),
        (
            "ListTargetsByRule on an unknown name",
            "ListTargetsByRule",
            json!({ "Rule": "no-such-rule" }),
        ),
        (
            "PutTargets on an unknown name",
            "PutTargets",
            json!({
                "Rule": "no-such-rule",
                "Targets": [{ "Id": "t1", "Arn": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:q1") }],
            }),
        ),
        (
            "DeleteRule on an unknown name",
            "DeleteRule",
            json!({ "Name": "no-such-rule" }),
        ),
    ];

    for (case, operation, request) in cases {
        let answer = emulator.call(operation, request).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected 400 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // And a rule on the default bus is not reachable through another bus's name.
    emulator
        .call(
            "PutRule",
            json!({ "Name": "on-default", "EventPattern": pattern_for("my.app") }),
        )
        .await;
    let answer = emulator
        .call(
            "DescribeRule",
            json!({ "Name": "on-default", "EventBusName": "other-bus" }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "ResourceNotFoundException"),
        "a rule is scoped to its bus: expected 400 ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.body
    );
}

#[tokio::test]
async fn delete_rule_removes_the_record_once_its_targets_are_gone() {
    let emulator = emulator();
    emulator
        .call(
            "PutRule",
            json!({ "Name": "doomed", "EventPattern": pattern_for("my.app") }),
        )
        .await;
    emulator
        .call(
            "PutRule",
            json!({ "Name": "kept", "EventPattern": pattern_for("my.app") }),
        )
        .await;
    emulator
        .call(
            "PutTargets",
            json!({
                "Rule": "kept",
                "Targets": [{ "Id": "t1", "Arn": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:q1") }],
            }),
        )
        .await;

    // "Before you can delete the rule, you must remove all associated targets."
    for force in [false, true] {
        let answer = emulator
            .call("DeleteRule", json!({ "Name": "kept", "Force": force }))
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "case Force={force}: a rule with targets is not deletable, got {} {:?}",
            answer.status,
            answer.body
        );
    }
    let still_there = emulator
        .call("DescribeRule", json!({ "Name": "kept" }))
        .await;
    assert_eq!(
        still_there.status, 200,
        "a refused delete must leave the rule in place, got {} {:?}",
        still_there.status, still_there.body
    );

    let deleted = emulator
        .call("DeleteRule", json!({ "Name": "doomed" }))
        .await;
    assert_eq!(
        deleted.status, 200,
        "deleting a targetless rule: expected 200, got {} {:?}",
        deleted.status, deleted.body
    );

    let described = emulator
        .call("DescribeRule", json!({ "Name": "doomed" }))
        .await;
    assert_eq!(
        (described.status, described.error_code()),
        (400, "ResourceNotFoundException"),
        "a deleted rule is gone: expected 400 ResourceNotFoundException, got {} {:?}",
        described.status,
        described.body
    );

    let listed = emulator.call("ListRules", json!({})).await;
    let names: Vec<String> = listed.body["Rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| rule["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["kept"],
        "ListRules must no longer carry the deleted rule, got {names:?}"
    );

    // A deleted rule is no longer matched against either.
    let answer = emulator
        .call("PutEvents", json!({ "Entries": [{ "Source": "my.app" }] }))
        .await;
    let event_id = answer.body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let matched = emulator.matched_rules(DEFAULT_BUS, &event_id);
    assert_eq!(
        matched,
        vec!["kept".to_string()],
        "a deleted rule must not match: matched {matched:?}"
    );
}

#[tokio::test]
async fn a_reporting_call_leaves_the_store_as_it_found_it() {
    let emulator = emulator();

    // The default bus is materialised by the first call that CHANGES it, so a read
    // before any write must not write it into existence — and neither must a
    // MUTATING call that is refused before it changes anything.
    for (case, operation, request) in [
        ("DescribeRule", "DescribeRule", json!({ "Name": "absent" })),
        ("ListRules", "ListRules", json!({})),
        (
            "ListTargetsByRule",
            "ListTargetsByRule",
            json!({ "Rule": "absent" }),
        ),
        (
            "a refused PutTargets",
            "PutTargets",
            json!({
                "Rule": "absent",
                "Targets": [{ "Id": "t1", "Arn": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:q1") }],
            }),
        ),
        (
            "a refused DeleteRule",
            "DeleteRule",
            json!({ "Name": "absent" }),
        ),
    ] {
        let _ = emulator.call(operation, request).await;
        let state = emulator
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let buses = state.lock().buses.len();
        assert_eq!(
            buses, 0,
            "case {case:?}: a call that changes nothing must not create a bus, store holds {buses}"
        );
    }

    // And an empty default bus still lists as empty rather than as a missing one.
    let listed = emulator.call("ListRules", json!({})).await;
    assert_eq!(
        (listed.status, listed.body["Rules"].clone()),
        (200, json!([])),
        "ListRules on an untouched default bus: expected 200 with an empty list, got {} {:?}",
        listed.status,
        listed.body
    );
}

// ---------------------------------------------------------------------------
// The bounds on what one caller can make the store hold
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matching_a_large_value_against_a_large_wildcard_stays_cheap() {
    // The adversarial shape for a BACKTRACKING glob: a wildcard whose literal runs
    // all match a long prefix of the observed value, which then fails at the end. A
    // quadratic matcher would spend |wildcard| x |observed| comparisons here —
    // billions — so this case is about the WORK, not only the answer.
    let emulator = emulator();
    let run = "a".repeat(2_000);
    let wildcard = format!("*{run}*{run}*b");
    let pattern = json!({ "detail": { "p": [{ "wildcard": wildcard }] } }).to_string();
    let stored = emulator
        .call(
            "PutRule",
            json!({ "Name": "costly-wildcard", "EventPattern": pattern.clone() }),
        )
        .await;
    assert_eq!(
        stored.status, 200,
        "the wildcard is within the pattern bound: expected 200, got {} {:?}",
        stored.status, stored.body
    );

    let value = "a".repeat(200_000);
    let detail = json!({ "p": value.clone() }).to_string();
    let answer = emulator
        .call(
            "PutEvents",
            json!({ "Entries": [{ "Source": "a.one", "Detail": detail }] }),
        )
        .await;
    assert_eq!(
        answer.body["FailedEntryCount"],
        json!(0),
        "the entry is within the size limit, got {:?}",
        answer.body
    );
    let event_id = answer.body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let matched = emulator.matched_rules(DEFAULT_BUS, &event_id);
    assert!(
        matched.is_empty(),
        "the wildcard ends in 'b' and the value does not, so nothing matches: {matched:?}"
    );

    // The same match, driven directly so the COST is assertable rather than timed:
    // a linear matcher charges about |wildcard| + |observed| steps, a backtracking
    // one would charge their product.
    let parsed = event_pattern::parse(&pattern).expect("the pattern is accepted above");
    let event = json!({ "source": "a.one", "detail": { "p": value } });
    let mut budget = event_pattern::MatchBudget::new(event_pattern::MAX_MATCH_STEPS_PER_REQUEST);
    let matched = event_pattern::matches_within(&parsed, &event, &mut budget);
    let linear_cost = (wildcard.len() + 200_000) as u64;
    assert!(
        !matched,
        "the direct match must agree with the served one: expected no match"
    );
    assert!(
        budget.spent() <= linear_cost * 2,
        "matching must cost about {linear_cost} steps (linear in the inputs), spent {}",
        budget.spent()
    );
}

#[tokio::test]
async fn a_bus_holds_a_bounded_number_of_rules() {
    let emulator = emulator();
    for n in 0..RULES_PER_BUS {
        let answer = emulator
            .call(
                "PutRule",
                json!({ "Name": format!("rule-{n:04}"), "EventPattern": pattern_for("my.app") }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "rule {n} is within the quota: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    let over = emulator
        .call(
            "PutRule",
            json!({ "Name": "one-too-many", "EventPattern": pattern_for("my.app") }),
        )
        .await;
    assert_eq!(
        (over.status, over.error_code()),
        (400, "LimitExceededException"),
        "the rule past the quota: expected 400 LimitExceededException, got {} {:?}",
        over.status,
        over.body
    );

    // An UPDATE adds no record, so it is not refused at the quota.
    let update = emulator
        .call(
            "PutRule",
            json!({ "Name": "rule-0000", "EventPattern": pattern_for("other.app") }),
        )
        .await;
    assert_eq!(
        update.status, 200,
        "an update at the quota adds no record: expected 200, got {} {:?}",
        update.status, update.body
    );
    let described = emulator
        .call("DescribeRule", json!({ "Name": "rule-0000" }))
        .await;
    assert_eq!(
        described.string("EventPattern"),
        pattern_for("other.app"),
        "the update must still have been stored, got {:?}",
        described.body
    );
}

#[tokio::test]
async fn a_bus_retains_a_bounded_window_of_events_oldest_first() {
    // The emulator's own retention window for the per-bus event log, spelled out here
    // so a change in `src/state.rs` that silently widens it fails.
    const RECORDED_EVENTS_PER_BUS: usize = 1_000;

    let emulator = emulator();
    let mut first = String::new();
    let mut last = String::new();
    // One entry past the window, so exactly one event has to have been evicted.
    for n in 0..=RECORDED_EVENTS_PER_BUS {
        let answer = emulator
            .call(
                "PutEvents",
                json!({ "Entries": [{ "Source": format!("app.{n}"), "Detail": "{}" }] }),
            )
            .await;
        let id = answer.body["Entries"][0]["EventId"]
            .as_str()
            .unwrap_or_else(|| panic!("entry {n}: expected an EventId, got {:?}", answer.body))
            .to_string();
        if n == 0 {
            first = id.clone();
        }
        last = id;
    }

    let held = emulator.recorded_event_count(DEFAULT_BUS);
    assert_eq!(
        held, RECORDED_EVENTS_PER_BUS,
        "the log must stay at its retention window, store holds {held}"
    );
    assert!(
        !emulator.holds_event(DEFAULT_BUS, &first),
        "eviction is oldest-first, so the first event {first} must be gone"
    );
    assert!(
        emulator.holds_event(DEFAULT_BUS, &last),
        "the newest event {last} must still be held"
    );
}

#[tokio::test]
async fn a_bus_retains_a_bounded_number_of_bytes_oldest_first() {
    // The emulator's own byte window for the per-bus event log, spelled out here
    // so a change in `src/state.rs` that silently widens it fails.
    const RECORDED_BYTES_PER_BUS: usize = 4 * 1024 * 1024;

    let emulator = emulator();
    // Each entry carries a ~200 KB detail, so roughly twenty fit in the window —
    // far below the 1 000-event count bound, which is the point: the byte bound
    // is what binds here.
    let detail = json!({ "p": "x".repeat(200_000) }).to_string();
    let mut first = String::new();
    let mut last = String::new();
    for n in 0..40 {
        let answer = emulator
            .call(
                "PutEvents",
                json!({ "Entries": [{ "Source": "app.big", "Detail": detail }] }),
            )
            .await;
        let id = answer.body["Entries"][0]["EventId"]
            .as_str()
            .unwrap_or_else(|| panic!("entry {n}: expected an EventId, got {:?}", answer.body))
            .to_string();
        if n == 0 {
            first = id.clone();
        }
        last = id;
    }

    let state = emulator
        .engine
        .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
    let store = state.lock();
    let bus = store
        .bus(DEFAULT_BUS)
        .expect("the default bus holds the recorded events");
    assert!(
        bus.retained_bytes <= RECORDED_BYTES_PER_BUS,
        "the log must stay inside its byte window, holds {} bytes",
        bus.retained_bytes
    );
    assert!(
        bus.events.len() < 40,
        "the byte window must have evicted something, holds {} of 40 events",
        bus.events.len()
    );
    assert!(
        !bus.events.iter().any(|event| event.event_id == first),
        "eviction is oldest-first, so the first event {first} must be gone"
    );
    assert!(
        bus.events.iter().any(|event| event.event_id == last),
        "the newest event {last} must still be held"
    );
}

#[tokio::test]
async fn put_rule_refuses_a_pattern_past_the_size_and_nesting_bounds() {
    let emulator = emulator();

    /// A pattern nested `levels` deep: `{"detail": {"detail": … {"x": ["deep"]}}}`.
    fn nested(levels: usize) -> Value {
        let mut pattern = json!({ "x": ["deep"] });
        for _ in 0..levels {
            pattern = json!({ "detail": pattern });
        }
        pattern
    }

    // The rows differ along which bound is crossed and by how much: just inside,
    // and just outside.
    let cases: [(&str, String, u16); 4] = [
        (
            "a pattern just inside the size bound",
            json!({ "source": ["x".repeat(8 * 1024 - 64)] }).to_string(),
            200,
        ),
        (
            "a pattern past the size bound",
            json!({ "source": ["x".repeat(8 * 1024)] }).to_string(),
            400,
        ),
        (
            "a pattern just inside the nesting bound",
            nested(8).to_string(),
            200,
        ),
        (
            "a pattern past the nesting bound",
            nested(64).to_string(),
            400,
        ),
    ];

    for (index, (case, pattern, status)) in cases.into_iter().enumerate() {
        let answer = emulator
            .call(
                "PutRule",
                json!({ "Name": format!("bounded-{index}"), "EventPattern": pattern }),
            )
            .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected {status}, got {} {:?}",
            answer.status, answer.body
        );
        if status != 200 {
            assert_eq!(
                answer.error_code(),
                "InvalidEventPatternException",
                "case {case:?}: expected InvalidEventPatternException, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn put_events_refuses_an_entry_past_the_documented_size() {
    let emulator = emulator();

    /// A `Detail` document of exactly `bytes` bytes: `{"p":"xx…x"}` is 8 bytes of
    /// envelope plus the padding.
    fn detail_of(bytes: usize) -> String {
        format!("{{\"p\":\"{}\"}}", "x".repeat(bytes - 8))
    }

    // Entry size is `Source` + `DetailType` + `Resources` + `Detail` (+ 14 for
    // `Time`), per "Calculating PutEvents event entry size". `Source` is one byte in
    // both rows, so the first lands exactly ON the limit and the second one byte past
    // it.
    let answer = emulator
        .call(
            "PutEvents",
            json!({
                "Entries": [
                    { "Source": "a", "Detail": detail_of(ENTRY_BYTES - 1) },
                    { "Source": "a", "Detail": detail_of(ENTRY_BYTES) },
                ],
            }),
        )
        .await;
    assert_eq!(
        answer.body["FailedEntryCount"],
        json!(1),
        "only the entry past the limit is refused, got {:?}",
        answer.body["FailedEntryCount"]
    );
    let entries = answer.body["Entries"].as_array().cloned().unwrap_or_default();
    assert!(
        entries[0]["EventId"].is_string(),
        "an entry exactly at the limit is accepted, got {:?}",
        entries[0]
    );
    assert_eq!(
        entries[1]["ErrorCode"].as_str().unwrap_or_default(),
        "InvalidArgument",
        "an entry past the limit names its reason, got {:?}",
        entries[1]
    );
    assert_eq!(
        emulator.recorded_event_count(DEFAULT_BUS),
        1,
        "only the accepted entry is recorded, store holds {}",
        emulator.recorded_event_count(DEFAULT_BUS)
    );
}

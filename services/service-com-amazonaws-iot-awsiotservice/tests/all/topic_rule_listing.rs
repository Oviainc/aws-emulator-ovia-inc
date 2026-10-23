// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListTopicRules`: the filtered, paginated reader over
//! the AWS IoT rules store.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `GET /rules` request signed for the `iot` signing name — so what is
//! asserted is what an AWS SDK client would see, not an internal function's return
//! value.
//!
//! The rule under test: `ListTopicRules` reports ONE PAGE of the rules the store holds,
//! in `ruleName` order, each entry built from the record `CreateTopicRule` persisted —
//! `ruleArn`, `ruleName`, `createdAt`, `ruleDisabled`, and `topicPattern` as the topic
//! filter that rule's SQL statement selects from; the request's `ruleDisabled` and
//! `topic` members NARROW that set (a rule belongs to a topic when its topic filter
//! covers that topic under MQTT matching); `maxResults` bounds the page and `nextToken`
//! is returned only when rules remain and resumes the SAME listing exactly once; and the
//! listing writes nothing.
//!
//! Each table spans the class the rule generalises over — the form of the statement the
//! topic filter is read out of, which filter members a request carries, which topics and
//! filters are compared, how many rules match, and every page size from one to past the
//! number of matches — rather than one recorded request.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::state::MAX_SQL_SCAN_CHARACTERS;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::describe_endpoint::percent_encode;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service, and how the service picks the
/// region its ARNs are scoped to. The signature itself is never checked here — the
/// harness turns request authentication off, exactly as an unauthenticated emulator
/// runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The `rules` member as the list it is, or an empty list when it is absent.
    fn rules(&self) -> Vec<Value> {
        self.body
            .get("rules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `ruleName` of every entry, in the order the response carries them.
    fn names(&self) -> Vec<String> {
        self.rules()
            .iter()
            .map(|rule| {
                rule.get("ruleName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The entry naming `rule_name`, when the page carries it.
    fn entry(&self, rule_name: &str) -> Option<Value> {
        self.rules()
            .into_iter()
            .find(|rule| rule.get("ruleName").and_then(Value::as_str) == Some(rule_name))
    }

    fn next_token(&self) -> Option<String> {
        self.body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_rule(&self, rule_name: &str, payload: &Value) -> Reply {
        self.call("POST", &format!("/rules/{rule_name}"), Some(payload))
            .await
    }

    async fn get_rule(&self, rule_name: &str) -> Reply {
        self.call("GET", &format!("/rules/{rule_name}"), None).await
    }

    /// `GET /rules` with the query string `query` (already encoded).
    async fn list_rules(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/rules".to_string()
        } else {
            format!("/rules?{query}")
        };
        self.call("GET", &path, None).await
    }
}

/// The ARN a rule created in this harness's region must carry.
fn expected_arn(rule_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:rule/{rule_name}")
}

/// Epoch seconds now, for bounding a reported `createdAt`.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One rule a fixture records: the name it is created under, the SQL statement it
/// queries with, whether the payload disables it, and the `topicPattern` a listing must
/// then report for it.
struct RuleFixture {
    name: &'static str,
    sql: &'static str,
    /// `TopicRulePayload$ruleDisabled`, absent when `None`.
    disabled: Option<bool>,
    /// The topic filter the statement selects from, or `None` when it names none.
    topic_pattern: Option<&'static str>,
}

impl RuleFixture {
    fn payload(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("sql".to_string(), json!(self.sql));
        payload.insert("actions".to_string(), json!([]));
        if let Some(disabled) = self.disabled {
            payload.insert("ruleDisabled".to_string(), json!(disabled));
        }
        Value::Object(payload)
    }

    /// What `ruleDisabled` must report: the payload's value, or `false` when it carried
    /// none (`state::RegisteredTopicRule`: a rule created without it is enabled).
    fn expected_disabled(&self) -> bool {
        self.disabled.unwrap_or(false)
    }
}

/// The STATEMENT table: the forms a rule's `TopicRulePayload$sql` takes, and the topic
/// filter each one selects from.
///
/// The AWS IoT SQL reference gives the statement as
/// `SELECT <attributes> FROM <topic filter> [WHERE <condition>]` with the topic filter
/// written as a single-quoted string, so the rows differ along everything that leaves
/// that clause meaning the same thing — the clause's casing, the whitespace and newlines
/// around it, a line or block comment before it, a quoted constant in the select clause,
/// a `WHERE` after it, a selected attribute list instead of `*`, a quote escaped inside
/// the filter, how many levels the filter has and which wildcards it uses — plus the
/// statements that name NO quoted filter, whose `topicPattern` is absent rather than
/// invented.
fn statement_fixtures() -> Vec<RuleFixture> {
    vec![
        RuleFixture {
            name: "plain_topic",
            sql: "SELECT * FROM 'sensors/one/telemetry'",
            disabled: None,
            topic_pattern: Some("sensors/one/telemetry"),
        },
        RuleFixture {
            name: "single_level_wildcard",
            sql: "SELECT * FROM 'sensors/+/telemetry'",
            disabled: Some(false),
            topic_pattern: Some("sensors/+/telemetry"),
        },
        RuleFixture {
            name: "multi_level_wildcard_with_where",
            sql: "SELECT temperature FROM 'devices/#' WHERE temperature > 30",
            disabled: Some(true),
            topic_pattern: Some("devices/#"),
        },
        RuleFixture {
            name: "lowercase_keywords",
            sql: "select * from 'lower/case'",
            disabled: None,
            topic_pattern: Some("lower/case"),
        },
        RuleFixture {
            name: "newlines_and_tabs",
            sql: "SELECT *\n  FROM\t'multi/line/topic'\n  WHERE device = 'a'",
            disabled: Some(true),
            topic_pattern: Some("multi/line/topic"),
        },
        RuleFixture {
            name: "comment_before_from",
            sql: "SELECT * -- everything in the message\nFROM 'commented/topic'",
            disabled: None,
            topic_pattern: Some("commented/topic"),
        },
        RuleFixture {
            name: "block_comment_before_from",
            sql: "SELECT * /* everything\n in the message */ FROM 'block/comment/topic'",
            disabled: None,
            topic_pattern: Some("block/comment/topic"),
        },
        RuleFixture {
            name: "literal_before_from",
            // A quoted constant in the select clause is not the topic filter: the
            // filter is the literal the FROM clause names.
            sql: "SELECT 'constant' AS tag FROM 'literal/before/from'",
            disabled: Some(true),
            topic_pattern: Some("literal/before/from"),
        },
        RuleFixture {
            name: "unterminated_literal",
            // A statement whose literal never closes names no topic filter.
            sql: "SELECT * FROM 'never/closed",
            disabled: None,
            topic_pattern: None,
        },
        RuleFixture {
            name: "reserved_topic",
            sql: "SELECT * FROM '$aws/things/+/shadow/update'",
            disabled: None,
            topic_pattern: Some("$aws/things/+/shadow/update"),
        },
        RuleFixture {
            name: "escaped_quote_in_filter",
            // A doubled quote is how a SQL string literal carries one: the filter is
            // `quote's/topic`, which only a lexer reads correctly.
            sql: "SELECT * FROM 'quote''s/topic'",
            disabled: None,
            topic_pattern: Some("quote's/topic"),
        },
        RuleFixture {
            name: "single_level_topic",
            sql: "SELECT VALUE payload FROM 'raw'",
            disabled: None,
            topic_pattern: Some("raw"),
        },
        RuleFixture {
            name: "whole_topic_space",
            sql: "SELECT * FROM '#'",
            disabled: Some(true),
            topic_pattern: Some("#"),
        },
        RuleFixture {
            name: "no_from_clause",
            sql: "SELECT 1",
            disabled: None,
            topic_pattern: None,
        },
        RuleFixture {
            name: "unquoted_from",
            // The IoT SQL reference requires the topic filter in single quotes, so an
            // unquoted `FROM` names no topic filter at all.
            sql: "SELECT * FROM sensors",
            disabled: None,
            topic_pattern: None,
        },
    ]
}

/// An emulator holding every rule of `fixtures`, created through the wire, and the names
/// a full listing must report, in `ruleName` order.
async fn store_of(fixtures: &[RuleFixture]) -> (Emulator, Vec<String>) {
    let emulator = Emulator::new();
    for fixture in fixtures {
        let created = emulator.create_rule(fixture.name, &fixture.payload()).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the fixture CreateTopicRule must succeed, got {created:?}",
            fixture.name
        );
    }
    let mut ordered: Vec<String> = fixtures
        .iter()
        .map(|fixture| fixture.name.to_string())
        .collect();
    ordered.sort();
    (emulator, ordered)
}

#[tokio::test]
async fn an_empty_store_lists_no_rules() {
    let emulator = Emulator::new();

    // `TopicRuleList` carries no length constraint, so "no rules" is an EMPTY list and
    // no `nextToken` — never an absent member and never a refusal.
    let listed = emulator.list_rules("").await;
    assert_eq!(
        listed.status, 200,
        "ListTopicRules over an empty store must succeed, got {listed:?}"
    );
    assert_eq!(
        listed.rules(),
        Vec::<Value>::new(),
        "an empty store must list no rules, got {listed:?}"
    );
    assert_eq!(
        listed.next_token(),
        None,
        "a listing with nothing left must carry no nextToken, got {listed:?}"
    );
}

#[tokio::test]
async fn every_entry_reports_the_record_the_create_persisted() {
    let before = now_epoch_seconds();
    let fixtures = statement_fixtures();
    let (emulator, ordered) = store_of(&fixtures).await;

    let listed = emulator.list_rules("").await;
    assert_eq!(
        listed.status, 200,
        "ListTopicRules must succeed, got {listed:?}"
    );
    assert_eq!(
        listed.names(),
        ordered,
        "the listing must report every rule once, in ruleName order, got {listed:?}"
    );
    assert_eq!(
        listed.next_token(),
        None,
        "a listing that reported every rule must carry no nextToken, got {listed:?}"
    );
    let after = now_epoch_seconds();

    for fixture in &fixtures {
        let label = fixture.name;
        let entry = listed
            .entry(label)
            .unwrap_or_else(|| panic!("case {label:?}: the listing must carry the rule: {listed:?}"));
        assert_eq!(
            entry.get("ruleArn").and_then(Value::as_str),
            Some(expected_arn(label).as_str()),
            "case {label:?}: expected the rule's ARN, got {entry:?}"
        );
        assert_eq!(
            entry.get("topicPattern").and_then(Value::as_str),
            fixture.topic_pattern,
            "case {label:?}: expected topicPattern {:?}, got {entry:?}",
            fixture.topic_pattern
        );
        assert_eq!(
            entry.get("ruleDisabled").and_then(Value::as_bool),
            Some(fixture.expected_disabled()),
            "case {label:?}: expected ruleDisabled {}, got {entry:?}",
            fixture.expected_disabled()
        );

        // `createdAt` is the instant of the create — a real clock reading — and it is
        // the SAME value `GetTopicRule` reports for that rule, because both readers
        // report one stored record.
        let created_at = entry
            .get("createdAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: the entry reported no createdAt: {entry:?}"));
        assert!(
            (before - 1.0..=after + 1.0).contains(&created_at),
            "case {label:?}: createdAt {created_at} is outside [{before}, {after}], got {entry:?}"
        );

        let read = emulator.get_rule(label).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetTopicRule must find the listed rule, got {read:?}"
        );
        assert_eq!(
            read.body.get("ruleArn").and_then(Value::as_str),
            entry.get("ruleArn").and_then(Value::as_str),
            "case {label:?}: the listed ARN and the read ARN must be one value, got {read:?}"
        );
        let rule = read
            .body
            .get("rule")
            .unwrap_or_else(|| panic!("case {label:?}: GetTopicRule reported no rule: {read:?}"));
        assert_eq!(
            rule.get("createdAt").and_then(Value::as_f64),
            Some(created_at),
            "case {label:?}: the listed createdAt and the read createdAt must be one value, got \
             {read:?}"
        );
        assert_eq!(
            rule.get("ruleDisabled").and_then(Value::as_bool),
            Some(fixture.expected_disabled()),
            "case {label:?}: the listed ruleDisabled and the read ruleDisabled must agree, got \
             {read:?}"
        );
        assert_eq!(
            rule.get("sql").and_then(Value::as_str),
            Some(fixture.sql),
            "case {label:?}: the rule must report the persisted SQL, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_statement_scan_is_bounded() {
    let emulator = Emulator::new();

    // `TopicRulePayload$sql` carries no modelled length, so the pass that reads the
    // topic filter reads at most `MAX_SQL_SCAN_CHARACTERS` of the statement. A filter
    // inside that prefix is still reported; one pushed past it is reported as UNKNOWN —
    // an absent `topicPattern`, never a guess, and never by reading a statement of
    // unbounded size.
    let filler = "x".repeat(MAX_SQL_SCAN_CHARACTERS + 1_000);
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "a filter inside the prefix, with the statement's bulk after it",
            format!("SELECT * FROM 'early/topic' WHERE device = '{filler}'"),
            Some("early/topic"),
        ),
        (
            "a filter pushed past the prefix by a comment",
            format!("SELECT * /* {filler} */ FROM 'late/topic'"),
            None,
        ),
    ];

    for (index, (label, sql, expected)) in rows.iter().enumerate() {
        let rule_name = format!("bounded_scan_{index}");
        let created = emulator
            .create_rule(&rule_name, &json!({ "sql": sql, "actions": [] }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateTopicRule must succeed, got {created:?}"
        );

        let listed = emulator.list_rules("").await;
        let entry = listed
            .entry(&rule_name)
            .unwrap_or_else(|| panic!("case {label:?}: the listing must carry the rule: {listed:?}"));
        assert_eq!(
            entry.get("topicPattern").and_then(Value::as_str),
            *expected,
            "case {label:?}: expected topicPattern {expected:?}, got {entry:?}"
        );
    }
}

#[tokio::test]
async fn listing_the_rules_changes_nothing_a_later_caller_sees() {
    let fixtures = statement_fixtures();
    let (emulator, _) = store_of(&fixtures).await;

    // The listing reports the rules; it must not re-time, re-mint or otherwise rewrite
    // them, so repeated listings — whole, filtered and paged — are byte-identical.
    for query in [
        "",
        "maxResults=2",
        "ruleDisabled=true",
        "ruleDisabled=false",
        "topic=raw",
    ] {
        let first = emulator.list_rules(query).await;
        let second = emulator.list_rules(query).await;
        assert_eq!(
            first.status, second.status,
            "case {query:?}: both listings must answer the same status, got {second:?}"
        );
        assert_eq!(
            first.body, second.body,
            "case {query:?}: a listing must change nothing, got {first:?} then {second:?}"
        );
    }

    // And a rule read after all that listing is still the record the create wrote.
    for fixture in &fixtures {
        let read = emulator.get_rule(fixture.name).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: the rule must still be readable, got {read:?}",
            fixture.name
        );
    }
}

#[tokio::test]
async fn the_rule_disabled_filter_selects_the_rules_in_that_state() {
    let fixtures = statement_fixtures();
    let (emulator, ordered) = store_of(&fixtures).await;

    // `ruleDisabled` is "Specifies whether the rule is disabled": present, it selects
    // the rules in that state; absent, every rule.
    let mut disabled: Vec<String> = fixtures
        .iter()
        .filter(|fixture| fixture.expected_disabled())
        .map(|fixture| fixture.name.to_string())
        .collect();
    disabled.sort();
    let mut enabled: Vec<String> = fixtures
        .iter()
        .filter(|fixture| !fixture.expected_disabled())
        .map(|fixture| fixture.name.to_string())
        .collect();
    enabled.sort();
    assert!(
        !disabled.is_empty() && !enabled.is_empty(),
        "the fixture must hold rules in both states, got {disabled:?} and {enabled:?}"
    );

    for (query, expected) in [
        ("ruleDisabled=true", &disabled),
        ("ruleDisabled=false", &enabled),
        ("", &ordered),
    ] {
        let listed = emulator.list_rules(query).await;
        assert_eq!(
            listed.status, 200,
            "case {query:?}: ListTopicRules must succeed, got {listed:?}"
        );
        assert_eq!(
            &listed.names(),
            expected,
            "case {query:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

/// The MATCHING fixture: one rule per topic-filter form, so a `topic` the listing is
/// asked for selects a set computed from the filters alone.
///
/// The rows span the forms an MQTT topic filter takes — a filter with no wildcard, `+`
/// in the middle and at the end, `#` under a parent and at the root, `+` at the root,
/// and a filter under the `$`-prefixed topics AWS IoT reserves.
fn matching_fixtures() -> Vec<RuleFixture> {
    vec![
        RuleFixture {
            name: "m_exact",
            sql: "SELECT * FROM 'a/b/c'",
            disabled: None,
            topic_pattern: Some("a/b/c"),
        },
        RuleFixture {
            name: "m_plus_mid",
            sql: "SELECT * FROM 'a/+/c'",
            disabled: None,
            topic_pattern: Some("a/+/c"),
        },
        RuleFixture {
            name: "m_plus_last",
            sql: "SELECT * FROM 'a/b/+'",
            disabled: None,
            topic_pattern: Some("a/b/+"),
        },
        RuleFixture {
            name: "m_hash",
            sql: "SELECT * FROM 'a/#'",
            disabled: None,
            topic_pattern: Some("a/#"),
        },
        RuleFixture {
            name: "m_hash_root",
            sql: "SELECT * FROM '#'",
            disabled: Some(true),
            topic_pattern: Some("#"),
        },
        RuleFixture {
            name: "m_plus_root",
            sql: "SELECT * FROM '+'",
            disabled: None,
            topic_pattern: Some("+"),
        },
        RuleFixture {
            name: "m_reserved",
            sql: "SELECT * FROM '$aws/events/#'",
            disabled: None,
            topic_pattern: Some("$aws/events/#"),
        },
        RuleFixture {
            name: "m_hash_not_last",
            // `#` anywhere but the last level is not a legal topic filter (MQTT 3.1.1
            // §4.7.1), so it covers no topic at all.
            sql: "SELECT * FROM '#/telemetry'",
            disabled: None,
            topic_pattern: Some("#/telemetry"),
        },
        RuleFixture {
            name: "m_no_pattern",
            sql: "SELECT 1",
            disabled: None,
            topic_pattern: None,
        },
    ]
}

#[tokio::test]
async fn a_topic_selects_the_rules_whose_filter_covers_it() {
    let fixtures = matching_fixtures();
    let (emulator, ordered) = store_of(&fixtures).await;

    // MQTT 3.1.1 §4.7, the matching AWS IoT Core's "Topics" documentation restates: `+`
    // matches exactly one level, `#` matches the remaining levels and the parent level
    // too, a filter with no wildcard covers only the identical topic, and a filter
    // starting with a wildcard does not reach a `$`-prefixed topic. A rule whose
    // statement names no filter (`m_no_pattern`), and one whose filter puts `#` before
    // the last level (`m_hash_not_last`, which is not a topic filter at all), are in NO
    // topic's set.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        (
            "a/b/c",
            vec!["m_exact", "m_plus_mid", "m_plus_last", "m_hash", "m_hash_root"],
        ),
        ("a/x/c", vec!["m_plus_mid", "m_hash", "m_hash_root"]),
        ("a/b/z", vec!["m_plus_last", "m_hash", "m_hash_root"]),
        ("a/b", vec!["m_hash", "m_hash_root"]),
        ("a/b/c/d", vec!["m_hash", "m_hash_root"]),
        // `a/#` covers its own parent level, and `+` covers a one-level topic.
        ("a", vec!["m_hash", "m_hash_root", "m_plus_root"]),
        ("b", vec!["m_hash_root", "m_plus_root"]),
        ("b/c", vec!["m_hash_root"]),
        // A reserved topic is reached only by a filter that names it literally.
        ("$aws/events/x/y", vec!["m_reserved"]),
        ("$aws", vec![]),
        ("nothing/matches/this", vec!["m_hash_root"]),
        // The levels a malformed `#/telemetry` filter would cover if `#` were honoured
        // mid-filter: it covers neither.
        ("telemetry", vec!["m_hash_root", "m_plus_root"]),
        ("x/telemetry", vec!["m_hash_root"]),
    ];

    for (topic, expected) in rows {
        let mut expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        let listed = emulator
            .list_rules(&format!("topic={}", percent_encode(topic)))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {topic:?}: ListTopicRules must succeed, got {listed:?}"
        );
        assert_eq!(
            listed.names(),
            expected,
            "case {topic:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // A listing with no `topic` reports every rule, including the one no topic selects.
    let whole = emulator.list_rules("").await;
    assert_eq!(
        whole.names(),
        ordered,
        "a listing with no topic must report every rule, got {whole:?}"
    );
}

#[tokio::test]
async fn a_topic_and_a_state_narrow_the_listing_together() {
    let fixtures = matching_fixtures();
    let (emulator, _) = store_of(&fixtures).await;

    // The filters compose: each row is the set its topic selects, narrowed to one state.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        (
            "topic=a%2Fb%2Fc&ruleDisabled=false",
            vec!["m_exact", "m_plus_mid", "m_plus_last", "m_hash"],
        ),
        ("topic=a%2Fb%2Fc&ruleDisabled=true", vec!["m_hash_root"]),
        ("topic=%24aws%2Fevents%2Fx&ruleDisabled=true", vec![]),
        ("topic=%24aws%2Fevents%2Fx&ruleDisabled=false", vec!["m_reserved"]),
    ];

    for (query, expected) in rows {
        let mut expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        let listed = emulator.list_rules(query).await;
        assert_eq!(
            listed.status, 200,
            "case {query:?}: ListTopicRules must succeed, got {listed:?}"
        );
        assert_eq!(
            listed.names(),
            expected,
            "case {query:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn every_page_size_walks_the_listing_once_through_its_next_token() {
    let fixtures = statement_fixtures();
    let (emulator, ordered) = store_of(&fixtures).await;
    let total = ordered.len();

    // Exhaustive over the page size rather than sampled: every size from the modelled
    // minimum (`TopicRuleMaxResults` is `range 1..10000`) to one past the number of
    // rules, plus the modelled maximum. Each walk must visit the listing exactly once,
    // in order, whatever the page boundary falls between.
    let mut page_sizes: Vec<usize> = (1..=total + 1).collect();
    page_sizes.push(10_000);

    for page_size in page_sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("maxResults={page_size}");
            if let Some(token) = &token {
                query.push_str(&format!("&nextToken={}", percent_encode(token)));
            }
            let page = emulator.list_rules(&query).await;
            assert_eq!(
                page.status, 200,
                "page size {page_size}, page {pages}: ListTopicRules must succeed, got {page:?}"
            );
            let names = page.names();
            assert!(
                names.len() <= page_size,
                "page size {page_size}, page {pages}: a page must not exceed maxResults, got \
                 {names:?}"
            );
            walked.extend(names);
            assert_eq!(
                page.next_token().is_some(),
                walked.len() < total,
                "page size {page_size}, page {pages}: nextToken must appear exactly while rules \
                 remain, got {page:?}"
            );
            token = page.next_token();
            pages += 1;
            assert!(
                pages <= total + 2,
                "page size {page_size}: the walk must terminate, got {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, ordered,
            "page size {page_size}: the pages must concatenate to the whole listing once, got \
             {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_page_of_a_filtered_listing_resumes_that_same_listing() {
    let fixtures = matching_fixtures();
    let (emulator, _) = store_of(&fixtures).await;

    // The whole filtered listing, then the same listing one rule at a time: the pages
    // of a filtered walk concatenate to it, so a token carries the filters it was
    // issued under.
    let query = "topic=a%2Fb%2Fc";
    let whole = emulator.list_rules(query).await;
    let expected = whole.names();
    assert!(
        expected.len() > 2,
        "the fixture must hold more than two rules for that topic, got {expected:?}"
    );

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut paged = format!("{query}&maxResults=1");
        if let Some(token) = &token {
            paged.push_str(&format!("&nextToken={}", percent_encode(token)));
        }
        let page = emulator.list_rules(&paged).await;
        assert_eq!(
            page.status, 200,
            "a page of the filtered listing must succeed, got {page:?}"
        );
        walked.extend(page.names());
        token = page.next_token();
        assert!(
            walked.len() <= expected.len(),
            "the filtered walk must not exceed the listing, got {walked:?}"
        );
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, expected,
        "the filtered pages must concatenate to the filtered listing, got {walked:?}"
    );
}

#[tokio::test]
async fn a_next_token_of_another_listing_or_of_nothing_is_refused() {
    let fixtures = matching_fixtures();
    let (emulator, _) = store_of(&fixtures).await;

    // A token is issued FOR a listing: the filters it was issued under decide what "the
    // next set of results" means, so replaying it against another listing is refused
    // with the modelled `InvalidRequestException` (400) rather than silently resuming a
    // different walk from the middle. So is a token this listing never issued.
    let issued = emulator.list_rules("maxResults=1").await;
    let unfiltered_token = issued
        .next_token()
        .unwrap_or_else(|| panic!("a truncated listing must carry a nextToken: {issued:?}"));
    let filtered = emulator.list_rules("topic=a%2Fb%2Fc&maxResults=1").await;
    let filtered_token = filtered
        .next_token()
        .unwrap_or_else(|| panic!("a truncated filtered listing must carry a nextToken: {filtered:?}"));

    let rows: Vec<(&str, String)> = vec![
        (
            "an unfiltered token replayed with a topic",
            format!("topic=a%2Fb%2Fc&nextToken={}", percent_encode(&unfiltered_token)),
        ),
        (
            "an unfiltered token replayed with a state filter",
            format!("ruleDisabled=false&nextToken={}", percent_encode(&unfiltered_token)),
        ),
        (
            "a filtered token replayed without the topic",
            format!("nextToken={}", percent_encode(&filtered_token)),
        ),
        (
            "a filtered token replayed under another topic",
            format!("topic=a%2Fb&nextToken={}", percent_encode(&filtered_token)),
        ),
        (
            "text that is not hex",
            "nextToken=not-a-token".to_string(),
        ),
        (
            "hex of arbitrary text",
            format!("nextToken={}", ::hex::encode("zulu")),
        ),
        (
            "hex of a scope with no cursor",
            format!("nextToken={}", ::hex::encode("a\u{1e}n")),
        ),
        (
            "hex of a cursor outside RuleName",
            format!("nextToken={}", ::hex::encode("a\u{1e}n\u{1f}not a rule name")),
        ),
        ("the empty token", "nextToken=".to_string()),
    ];

    for (label, query) in rows {
        let refused = emulator.list_rules(&query).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: must be refused 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {label:?}: expected InvalidRequestException, got {refused:?}"
        );
    }

    // A token replayed under the listing that issued it still works.
    let resumed = emulator
        .list_rules(&format!("nextToken={}", percent_encode(&unfiltered_token)))
        .await;
    assert_eq!(
        resumed.status, 200,
        "the issuing listing must accept its own token, got {resumed:?}"
    );
}

#[tokio::test]
async fn max_results_is_honoured_at_its_bounds_and_refused_outside_them() {
    let fixtures = statement_fixtures();
    let (emulator, ordered) = store_of(&fixtures).await;

    // `TopicRuleMaxResults` is `range 1..10000`
    // (`smithy_query(query="operation",operation="ListTopicRules")`): both bounds are
    // served, and a value outside the range is refused before the listing runs.
    let smallest = emulator.list_rules("maxResults=1").await;
    assert_eq!(
        smallest.names().len(),
        1,
        "maxResults=1 must report exactly one rule, got {smallest:?}"
    );
    let largest = emulator.list_rules("maxResults=10000").await;
    assert_eq!(
        largest.names(),
        ordered,
        "maxResults=10000 must report every rule the store holds, got {largest:?}"
    );
    assert_eq!(
        largest.next_token(),
        None,
        "a page that held every rule must carry no nextToken, got {largest:?}"
    );

    for out_of_range in ["0", "-1", "10001"] {
        let refused = emulator
            .list_rules(&format!("maxResults={out_of_range}"))
            .await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={out_of_range}: must be refused 400, got {refused:?}"
        );
    }
}

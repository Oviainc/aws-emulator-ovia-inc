// SPDX-License-Identifier: CC0-1.0

//! The rule lifecycle of this crate's model, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! awsJson1_1 deserialization, the handler, the persisted state and serialization — the
//! same path an SDK client takes. This model is the LEGACY `CloudWatch Events` name of
//! the service now called EventBridge, so it no longer claims the `events` key its model
//! declares (the superset EventBridge crate owns that) and answers under its own
//! sdkId-derived key instead — see `src/service.rs`'s `OWN_SDK_ID_KEY`, and
//! `superseded_by_eventbridge.rs` for the store-level proof that each key is served by
//! exactly one of the two models. Every request below is therefore signed for THIS
//! model's key, which is what reaches these handlers.
//!
//! The rules under test are the ones the service states for its core resources, and each
//! is a RULE over a TABLE of inputs rather than one recorded shape:
//!
//! * a rule's members are whatever the last `PutRule` stored, and every reporting
//!   operation computes its answer from that one record, under one ARN;
//! * a rule's targets are a list, unique by `Target$Id` and ordered by first insertion,
//!   and every member of a target survives the round trip;
//! * a listing is bounded by `Limit`, resumed by `NextToken`, and refuses a token it
//!   never issued;
//! * an event is matched against the enabled, pattern-carrying rules of the bus it was
//!   sent to, by the event-pattern language — one row per documented content filter;
//! * `PutEvents` answers one outcome per entry, counting the refused ones;
//! * a request no rule can be built from, and one naming no stored rule, are refused for
//!   WHY rather than stored or silently ignored;
//! * the store is bounded: rules per bus, and the per-bus event log's record and byte
//!   windows, evicting oldest-first;
//! * a reporting call leaves the store exactly as it found it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchevents_awsevents::state::ComAmazonawsCloudwatcheventsAwseventsState;

use crate::support::{
    default_bus_rule_arn, emulator, event_bus_arn, target_arn, Emulator, DEFAULT_BUS, REGION,
};

/// `RuleState`'s two values, spelled out here rather than imported so a wrong default in
/// `src/state.rs` fails this suite.
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// EventBridge's published quota of rules per event bus, which is the bound this
/// emulator puts on the store. Spelled out here rather than imported so a wrong constant
/// in `src/state.rs` fails.
const RULES_PER_BUS: usize = 300;

/// This emulator's OWN retention window for a bus's event log: at most this many records
/// and at most this many bytes of their content, whichever binds first.
const RECORDED_EVENTS_PER_BUS: usize = 1_000;
const RECORDED_BYTES_PER_BUS: usize = 4 * 1024 * 1024;

/// The documented `PutEvents` entry size limit ("Calculating PutEvents event entry
/// size": 256 KB).
const ENTRY_BYTES: usize = 256 * 1024;

/// The emulator's own bounds on a stored event pattern: its size in bytes and how deeply
/// it may nest (`src/state/event_pattern.rs`).
const PATTERN_BYTES: usize = 8 * 1024;
const PATTERN_DEPTH: usize = 32;

impl Emulator {
    /// The rules the recorded event `event_id` matched, read out of the store this
    /// service's handlers wrote. PANICS when the log holds no such event, so a negative
    /// assertion built on it cannot pass because nothing was recorded at all.
    fn matched_rules(&self, bus: &str, event_id: &str) -> Vec<String> {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store
            .bus(bus)
            .into_iter()
            .flat_map(|bus| bus.events.iter())
            .find(|event| event.event_id == event_id)
            .map(|event| event.matched_rules.clone())
            .unwrap_or_else(|| {
                panic!("the log of bus {bus} holds no event {event_id}");
            })
    }

    /// How many events the bus's log holds.
    fn recorded_event_count(&self, bus: &str) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store.bus(bus).map_or(0, |bus| bus.events.len())
    }

    /// Whether the bus's log still holds `event_id`.
    fn holds_event(&self, bus: &str, event_id: &str) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store.bus(bus).is_some_and(|bus| {
            bus.events.iter().any(|event| event.event_id == event_id)
        })
    }

    /// The names of the rules the bus holds, and the bytes its log retains.
    fn stored_rule_names(&self, bus: &str) -> Vec<String> {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store
            .bus(bus)
            .map(|bus| bus.rules.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn retained_bytes(&self, bus: &str) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store.bus(bus).map_or(0, |bus| bus.retained_bytes)
    }

    /// How many buses the store holds. The default bus is materialised only by a call
    /// that WRITES, so this is what a read-only call must leave at zero.
    fn stored_bus_count(&self) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store.buses.len()
    }

    /// `PutRule` with a pattern, asserted to have been stored.
    async fn put_pattern_rule(&self, name: &str, pattern: &Value) {
        let answer = self
            .call(
                "PutRule",
                json!({ "Name": name, "EventPattern": pattern.to_string() }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "PutRule {name} with pattern {pattern} answered {} {:?}",
            answer.status, answer.body
        );
    }

    /// `PutEvents` for one entry, answering the `EventId` it minted.
    async fn put_event(&self, entry: Value) -> String {
        let answer = self.call("PutEvents", json!({ "Entries": [entry] })).await;
        assert_eq!(
            answer.status, 200,
            "PutEvents answered {} {:?}",
            answer.status, answer.body
        );
        answer.body["Entries"][0]["EventId"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an EventId, got {:?}", answer.body))
            .to_string()
    }

    /// Every rule `ListRules` reports for `request`, following `NextToken` to the end.
    async fn list_rule_names(&self, request: &Value) -> Vec<String> {
        let mut names = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut page = request.clone();
            if let Some(token) = &next {
                page["NextToken"] = json!(token);
            }
            let answer = self.call("ListRules", page).await;
            assert_eq!(
                answer.status, 200,
                "ListRules {request} answered {} {:?}",
                answer.status, answer.body
            );
            for rule in answer.body["Rules"].as_array().cloned().unwrap_or_default() {
                names.push(rule["Name"].as_str().unwrap_or_default().to_string());
            }
            next = answer.body["NextToken"].as_str().map(ToString::to_string);
            if next.is_none() {
                return names;
            }
            assert!(
                names.len() <= RULES_PER_BUS,
                "ListRules kept issuing NextToken after {} rules",
                names.len()
            );
        }
    }

    /// Every target id `ListTargetsByRule` reports for `rule`, page by page.
    async fn list_target_ids(&self, rule: &str, limit: Option<i64>) -> Vec<String> {
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
            let answer = self.call("ListTargetsByRule", request).await;
            assert_eq!(
                answer.status, 200,
                "ListTargetsByRule {rule} answered {} {:?}",
                answer.status, answer.body
            );
            for target in answer.body["Targets"].as_array().cloned().unwrap_or_default() {
                ids.push(target["Id"].as_str().unwrap_or_default().to_string());
            }
            next = answer.body["NextToken"].as_str().map(ToString::to_string);
            if next.is_none() {
                return ids;
            }
            assert!(
                ids.len() <= 128,
                "ListTargetsByRule kept issuing NextToken after {} targets",
                ids.len()
            );
        }
    }
}

/// An event pattern matching `source` exactly.
fn pattern_for(source: &str) -> Value {
    json!({ "source": [source] })
}

/// `PutRule` stores a rule and every reporting operation reads that stored rule back:
/// the rows differ along which members a rule carries (a pattern, a rate schedule, a
/// cron schedule, a state, the optional description and role), and each member must come
/// back exactly as it went in, under one ARN.
#[tokio::test]
async fn a_rule_reads_back_exactly_what_put_rule_stored() {
    let emulator = emulator();
    let cases = vec![
        json!({ "Name": "pattern-only", "EventPattern": pattern_for("com.demo.widgets").to_string() }),
        json!({ "Name": "schedule.rate", "ScheduleExpression": "rate(5 minutes)" }),
        json!({
            "Name": "schedule_cron",
            "ScheduleExpression": "cron(0 20 * * ? *)",
            "State": DISABLED,
        }),
        json!({
            "Name": "everything-at-once",
            "EventPattern": pattern_for("com.demo.orders").to_string(),
            "ScheduleExpression": "rate(1 hour)",
            "Description": "a rule carrying every optional member",
            "RoleArn": format!("arn:aws:iam::{}:role/demo-invoke", engine::emulator_account_id()),
            "State": ENABLED,
        }),
    ];

    for case in &cases {
        let name = case["Name"].as_str().expect("the case names the rule");
        let answer = emulator.call("PutRule", case.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: PutRule answered {} {:?}",
            answer.status, answer.body
        );
        let expected_arn = default_bus_rule_arn(name);
        assert_eq!(
            answer.string("RuleArn"),
            expected_arn,
            "case {case}: expected RuleArn {expected_arn}, got {:?}",
            answer.body
        );

        let described = emulator.call("DescribeRule", json!({ "Name": name })).await;
        assert_eq!(
            described.status, 200,
            "case {case}: DescribeRule answered {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            (described.string("Arn"), described.string("Name")),
            (expected_arn, name.to_string()),
            "case {case}: expected the stored ARN and name, got {:?}",
            described.body
        );
        assert_eq!(
            described.string("EventBusName"),
            DEFAULT_BUS,
            "case {case}: a rule that named no bus belongs to the default bus, got {:?}",
            described.body
        );
        assert_eq!(
            described.string("CreatedBy"),
            engine::emulator_account_id(),
            "case {case}: expected CreatedBy to be this account, got {:?}",
            described.body
        );
        // An omitted `State` means ENABLED: "Rules are enabled by default, or based on
        // value of the state."
        let expected_state = case.get("State").cloned().unwrap_or(json!(ENABLED));
        assert_eq!(
            described.body["State"], expected_state,
            "case {case}: expected State {expected_state}, got {:?}",
            described.body
        );
        for member in ["EventPattern", "ScheduleExpression", "Description", "RoleArn"] {
            let expected = case.get(member).cloned().unwrap_or(Value::Null);
            let actual = described.body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case}: expected {member} {expected}, got {actual}"
            );
        }
        // Only an Amazon Web Services service creating a rule on the account's behalf
        // sets `ManagedBy`, and this emulator creates no managed rules.
        assert_eq!(
            described.body.get("ManagedBy").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {case}: no rule this emulator stores is a managed rule, got {:?}",
            described.body
        );
        // A rule is scoped to its bus: the same name on a bus the account does not hold
        // names no rule.
        let elsewhere = emulator
            .call(
                "DescribeRule",
                json!({ "Name": name, "EventBusName": "never-created" }),
            )
            .await;
        assert_eq!(
            (elsewhere.status, elsewhere.error_code()),
            (400, "ResourceNotFoundException"),
            "case {case}: a rule is scoped to its bus, got {} {:?}",
            elsewhere.status,
            elsewhere.body
        );
    }

    // Every stored rule is listed, in name order, and listing twice reports the same
    // rules: a read changes nothing a later caller sees.
    let listed_once = emulator.list_rule_names(&json!({})).await;
    let listed_twice = emulator.list_rule_names(&json!({})).await;
    let mut expected: Vec<String> = cases
        .iter()
        .map(|case| case["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    expected.sort();
    assert_eq!(
        (listed_once.clone(), listed_twice),
        (expected.clone(), expected),
        "expected every stored rule in name order, got {listed_once:?}"
    );
}

/// A second `PutRule` under a name already stored REPLACES that rule and hands back the
/// ARN minted the first time; it never adds a second record, and a member the update
/// omits is cleared rather than kept.
#[tokio::test]
async fn put_rule_replaces_an_existing_rule_rather_than_duplicating_it() {
    let emulator = emulator();
    let name = "repeat-me";
    let first = emulator
        .call(
            "PutRule",
            json!({
                "Name": name,
                "EventPattern": pattern_for("com.demo.first").to_string(),
                "Description": "the first revision",
                "State": ENABLED,
            }),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the first PutRule answered {} {:?}",
        first.status, first.body
    );

    // The rows differ along what the update carries, including dropping a member the
    // first call set.
    for update in [
        json!({ "Name": name, "EventPattern": pattern_for("com.demo.second").to_string() }),
        json!({ "Name": name, "ScheduleExpression": "rate(2 minutes)", "State": DISABLED }),
    ] {
        let answer = emulator.call("PutRule", update.clone()).await;
        assert_eq!(
            (answer.status, answer.string("RuleArn")),
            (200, first.string("RuleArn")),
            "case {update}: an update keeps the ARN minted at creation, got {:?}",
            answer.body
        );
        let described = emulator.call("DescribeRule", json!({ "Name": name })).await;
        for member in ["EventPattern", "ScheduleExpression", "Description", "State"] {
            let expected = update
                .get(member)
                .cloned()
                .unwrap_or(if member == "State" { json!(ENABLED) } else { Value::Null });
            let actual = described.body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {update}: an update replaces the record, so {member} must be \
                 {expected}, got {actual}"
            );
        }
        assert_eq!(
            emulator.stored_rule_names(DEFAULT_BUS),
            vec![name.to_string()],
            "case {update}: an update must not add a second record"
        );
    }
}

/// An event bus is one resource however a request addresses it: `EventBusNameOrArn`
/// admits a bare name or an event-bus ARN, so a rule written under one form is read back
/// under the other.
#[tokio::test]
async fn an_event_bus_arn_names_the_same_bus_as_its_name() {
    let emulator = emulator();
    let forms: [(&str, String); 3] = [
        ("omitted", String::new()),
        ("by name", DEFAULT_BUS.to_string()),
        ("by ARN", event_bus_arn(DEFAULT_BUS)),
    ];

    for (write_case, write_form) in &forms {
        let rule = format!("rule-written-{}", write_case.replace(' ', "-"));
        let mut request = json!({
            "Name": rule,
            "EventPattern": pattern_for("com.demo.buses").to_string(),
        });
        if !write_form.is_empty() {
            request["EventBusName"] = json!(write_form);
        }
        let answer = emulator.call("PutRule", request).await;
        assert_eq!(
            (answer.status, answer.string("RuleArn")),
            (200, default_bus_rule_arn(&rule)),
            "case {write_case:?}: every form names the default bus, got {:?}",
            answer.body
        );

        for (read_case, read_form) in &forms {
            let mut request = json!({ "Name": rule });
            if !read_form.is_empty() {
                request["EventBusName"] = json!(read_form);
            }
            let described = emulator.call("DescribeRule", request).await;
            assert_eq!(
                (described.status, described.string("EventBusName")),
                (200, DEFAULT_BUS.to_string()),
                "case written {write_case:?}, read {read_case:?}: both forms address one \
                 bus, got {:?}",
                described.body
            );
        }
    }
}

/// A rule's targets are a list keyed by `Id`: putting an id the rule already holds
/// updates that target in place and keeps its position, and every member of a target
/// survives the round trip.
#[tokio::test]
async fn targets_are_unique_by_id_and_keep_their_insertion_order() {
    let emulator = emulator();
    emulator
        .put_pattern_rule("with-targets", &pattern_for("com.demo.targets"))
        .await;

    let answer = emulator
        .call(
            "PutTargets",
            json!({
                "Rule": "with-targets",
                "Targets": [
                    { "Id": "first", "Arn": target_arn("queue-first") },
                    { "Id": "second", "Arn": target_arn("queue-second") },
                    {
                        "Id": "third",
                        "Arn": target_arn("queue-third"),
                        "Input": r#"{"literal":true}"#,
                        "RoleArn": format!("arn:aws:iam::{}:role/invoke", engine::emulator_account_id()),
                        "SqsParameters": { "MessageGroupId": "orders" },
                        "RetryPolicy": {
                            "MaximumRetryAttempts": 2,
                            "MaximumEventAgeInSeconds": 600,
                        },
                        "DeadLetterConfig": { "Arn": target_arn("queue-dlq") },
                    },
                ],
            }),
        )
        .await;
    assert_eq!(
        (
            answer.status,
            answer.body["FailedEntryCount"].clone(),
            answer.body["FailedEntries"].clone(),
        ),
        (200, json!(0), json!([])),
        "PutTargets answered {:?}",
        answer.body
    );
    assert_eq!(
        emulator.list_target_ids("with-targets", None).await,
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string()
        ],
        "targets are reported in insertion order"
    );

    // Re-putting an id updates it in place while a NEW id in the same batch is appended:
    // one request may replace and add at once.
    let answer = emulator
        .call(
            "PutTargets",
            json!({
                "Rule": "with-targets",
                "Targets": [
                    { "Id": "second", "Arn": target_arn("queue-second-v2"), "InputPath": "$.detail" },
                    { "Id": "fourth", "Arn": target_arn("queue-fourth") },
                ],
            }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.body["FailedEntryCount"].clone()),
        (200, json!(0)),
        "the mixed replace-and-append batch answered {:?}",
        answer.body
    );
    assert_eq!(
        emulator.list_target_ids("with-targets", None).await,
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
            "fourth".to_string()
        ],
        "an upsert keeps the target's position and the new id is appended"
    );

    let listed = emulator
        .call("ListTargetsByRule", json!({ "Rule": "with-targets" }))
        .await;
    let targets = listed.body["Targets"].as_array().cloned().unwrap_or_default();
    // Every target is reported VERBATIM — the whole object, so a dropped or invented
    // member fails as surely as a changed one.
    let expected_targets = json!([
        { "Id": "first", "Arn": target_arn("queue-first") },
        { "Id": "second", "Arn": target_arn("queue-second-v2"), "InputPath": "$.detail" },
        {
            "Id": "third",
            "Arn": target_arn("queue-third"),
            "Input": r#"{"literal":true}"#,
            "RoleArn": format!("arn:aws:iam::{}:role/invoke", engine::emulator_account_id()),
            "SqsParameters": { "MessageGroupId": "orders" },
            "RetryPolicy": {
                "MaximumRetryAttempts": 2,
                "MaximumEventAgeInSeconds": 600,
            },
            "DeadLetterConfig": { "Arn": target_arn("queue-dlq") },
        },
        { "Id": "fourth", "Arn": target_arn("queue-fourth") },
    ]);
    assert_eq!(
        json!(targets),
        expected_targets,
        "every target must be reported exactly as it was stored, got {:?}",
        listed.body
    );

    // `RemoveTargets` is still a scaffold in this crate, so it answers the scaffold's
    // honest 501 rather than removing anything — never an empty 200 a caller could not
    // tell from a real removal.
    let answer = emulator
        .call(
            "RemoveTargets",
            json!({ "Rule": "with-targets", "Ids": ["second"] }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (501, "NotImplemented"),
        "RemoveTargets is scaffolded here, got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        emulator.list_target_ids("with-targets", None).await,
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
            "fourth".to_string()
        ],
        "a scaffolded operation must not have changed the stored targets"
    );
}

/// A listing is bounded by `Limit` and resumed by `NextToken`: the rows differ along the
/// page size, and every page size must enumerate the same records exactly once.
#[tokio::test]
async fn rule_and_target_pages_are_bounded_by_limit_and_resumed_by_next_token() {
    let emulator = emulator();
    for index in 0..7 {
        emulator
            .put_pattern_rule(&format!("page-{index}"), &pattern_for("com.demo.pages"))
            .await;
    }
    emulator
        .call(
            "PutTargets",
            json!({
                "Rule": "page-0",
                "Targets": (0..7)
                    .map(|index| json!({
                        "Id": format!("target-{index}"),
                        "Arn": target_arn(&format!("queue-{index}")),
                    }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;

    let all_rules: Vec<String> = (0..7).map(|index| format!("page-{index}")).collect();
    let all_targets: Vec<String> = (0..7).map(|index| format!("target-{index}")).collect();
    for limit in [1_i64, 2, 3, 7, 100] {
        let rules = emulator
            .list_rule_names(&json!({ "Limit": limit }))
            .await;
        assert_eq!(
            rules, all_rules,
            "case Limit={limit}: paging must enumerate every rule exactly once"
        );
        let targets = emulator.list_target_ids("page-0", Some(limit)).await;
        assert_eq!(
            targets, all_targets,
            "case Limit={limit}: paging must enumerate every target exactly once"
        );

        // One page is never longer than `Limit`.
        let page = emulator
            .call("ListRules", json!({ "Limit": limit }))
            .await;
        let returned = page.body["Rules"].as_array().map_or(0, Vec::len);
        assert!(
            returned <= usize::try_from(limit).unwrap_or(usize::MAX),
            "case Limit={limit}: a page carried {returned} rules"
        );
    }

    // A `NamePrefix` filters the listing, and the filter composes with paging.
    let filtered = emulator
        .list_rule_names(&json!({ "NamePrefix": "page-1", "Limit": 1 }))
        .await;
    assert_eq!(
        filtered,
        vec!["page-1".to_string()],
        "a NamePrefix must filter the listing"
    );

    // A `NextToken` this operation never issued is refused rather than served as if it
    // were a position: `ListTargetsByRule` issues the index the next page starts at, so
    // a non-numeric token and one past the end are both none of its own.
    for (case, token) in [
        ("a token that is not a number", "not-a-number"),
        ("a token past the end of the list", "999999"),
    ] {
        let answer = emulator
            .call(
                "ListTargetsByRule",
                json!({ "Rule": "page-0", "NextToken": token }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "case {case:?}: a token this operation never issued must be refused, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

/// A published event is matched against the enabled, pattern-carrying rules of the bus it
/// was sent to, by the event-pattern language — one row per documented content filter,
/// each with an event that matches and one that does not.
#[tokio::test]
async fn an_event_matches_the_patterns_of_the_rules_on_its_bus() {
    let emulator = emulator();

    // (rule, pattern, an event that must match, an event that must not)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "literal",
            pattern_for("com.demo.literal"),
            json!({ "Source": "com.demo.literal", "Detail": "{}" }),
            json!({ "Source": "com.demo.other", "Detail": "{}" }),
        ),
        (
            "prefix",
            json!({ "source": [{ "prefix": "com.demo.pre" }] }),
            json!({ "Source": "com.demo.prefixed", "Detail": "{}" }),
            json!({ "Source": "com.demo.suffixed", "Detail": "{}" }),
        ),
        (
            "suffix",
            json!({ "source": [{ "suffix": ".tail" }] }),
            json!({ "Source": "com.demo.tail", "Detail": "{}" }),
            json!({ "Source": "com.demo.tail.more", "Detail": "{}" }),
        ),
        (
            "wildcard",
            json!({ "source": [{ "wildcard": "com.*.wild" }] }),
            json!({ "Source": "com.demo.wild", "Detail": "{}" }),
            json!({ "Source": "com.demo.tame", "Detail": "{}" }),
        ),
        (
            "equals-ignore-case",
            json!({ "detail-type": [{ "equals-ignore-case": "order created" }] }),
            json!({ "Source": "com.demo.case", "DetailType": "Order Created", "Detail": "{}" }),
            json!({ "Source": "com.demo.case", "DetailType": "Order Shipped", "Detail": "{}" }),
        ),
        (
            "anything-but",
            json!({ "source": [{ "anything-but": ["com.demo.excluded"] }] }),
            json!({ "Source": "com.demo.included", "Detail": "{}" }),
            json!({ "Source": "com.demo.excluded", "Detail": "{}" }),
        ),
        (
            "numeric",
            json!({ "detail": { "amount": [{ "numeric": [">", 10] }] } }),
            json!({ "Source": "com.demo.numeric", "Detail": r#"{"amount":11}"# }),
            json!({ "Source": "com.demo.numeric", "Detail": r#"{"amount":9}"# }),
        ),
        (
            "exists",
            json!({ "detail": { "optional": [{ "exists": true }] } }),
            json!({ "Source": "com.demo.exists", "Detail": r#"{"optional":"here"}"# }),
            json!({ "Source": "com.demo.exists", "Detail": r#"{"other":"here"}"# }),
        ),
        (
            "cidr",
            json!({ "detail": { "ip": [{ "cidr": "10.0.0.0/24" }] } }),
            json!({ "Source": "com.demo.cidr", "Detail": r#"{"ip":"10.0.0.7"}"# }),
            json!({ "Source": "com.demo.cidr", "Detail": r#"{"ip":"10.0.1.7"}"# }),
        ),
        (
            "or",
            json!({ "$or": [{ "source": ["com.demo.left"] }, { "source": ["com.demo.right"] }] }),
            json!({ "Source": "com.demo.right", "Detail": "{}" }),
            json!({ "Source": "com.demo.neither", "Detail": "{}" }),
        ),
        (
            "nested-and",
            json!({ "source": ["com.demo.both"], "detail": { "kind": ["widget"] } }),
            json!({ "Source": "com.demo.both", "Detail": r#"{"kind":"widget"}"# }),
            json!({ "Source": "com.demo.both", "Detail": r#"{"kind":"gadget"}"# }),
        ),
        (
            // A nested field the event does not carry at all, rather than one carrying
            // another value: the pattern still must not match.
            "nested-detail-missing",
            json!({ "detail": { "deep": { "leaf": ["here"] } } }),
            json!({ "Source": "com.demo.deep", "Detail": r#"{"deep":{"leaf":"here"}}"# }),
            json!({ "Source": "com.demo.deep", "Detail": r#"{"deep":{"other":"here"}}"# }),
        ),
        (
            "anything-but-with-prefix",
            json!({ "source": [{ "anything-but": { "prefix": "com.demo.skip" } }] }),
            json!({ "Source": "com.demo.keep", "Detail": "{}" }),
            json!({ "Source": "com.demo.skipped", "Detail": "{}" }),
        ),
        (
            "prefix-equals-ignore-case",
            json!({ "source": [{ "prefix": { "equals-ignore-case": "COM.DEMO.CASED" } }] }),
            json!({ "Source": "com.demo.cased.tail", "Detail": "{}" }),
            json!({ "Source": "com.other.cased", "Detail": "{}" }),
        ),
        (
            "exists-false",
            json!({ "detail": { "absent": [{ "exists": false }] } }),
            json!({ "Source": "com.demo.absent", "Detail": r#"{"other":1}"# }),
            json!({ "Source": "com.demo.absent", "Detail": r#"{"absent":1}"# }),
        ),
        (
            // Both ends of a numeric RANGE, which one comparison cannot express.
            "numeric-range",
            json!({ "detail": { "amount": [{ "numeric": [">=", 10, "<=", 20] }] } }),
            json!({ "Source": "com.demo.range", "Detail": r#"{"amount":20}"# }),
            json!({ "Source": "com.demo.range", "Detail": r#"{"amount":21}"# }),
        ),
        (
            // A number is compared numerically, not textually: `3` and `3.0` are one
            // value, and `30` is not.
            "numeric-equality-across-forms",
            json!({ "detail": { "count": [{ "numeric": ["=", 3] }] } }),
            json!({ "Source": "com.demo.equal", "Detail": r#"{"count":3.0}"# }),
            json!({ "Source": "com.demo.equal", "Detail": r#"{"count":30}"# }),
        ),
        (
            "wildcard-many-stars",
            json!({ "source": [{ "wildcard": "com.*.many.*.stars" }] }),
            json!({ "Source": "com.demo.many.more.stars", "Detail": "{}" }),
            json!({ "Source": "com.demo.many.stars.more", "Detail": "{}" }),
        ),
        (
            // An inner run the value does not carry: the wildcard's prefix and suffix
            // match, so only the middle decides.
            "wildcard-inner-run-absent",
            json!({ "source": [{ "wildcard": "com.*inner*.stars" }] }),
            json!({ "Source": "com.demo.inner.part.stars", "Detail": "{}" }),
            json!({ "Source": "com.demo.outer.part.stars", "Detail": "{}" }),
        ),
        (
            // A LIST field of the envelope: any one member matching is enough.
            "resources-list",
            json!({ "resources": [target_arn("watched-queue")] }),
            json!({
                "Source": "com.demo.resources",
                "Detail": "{}",
                "Resources": [target_arn("other-queue"), target_arn("watched-queue")],
            }),
            json!({
                "Source": "com.demo.resources",
                "Detail": "{}",
                "Resources": [target_arn("other-queue")],
            }),
        ),
        (
            // Envelope fields the SERVICE fills in, not the entry: the account that owns
            // the emulator and the region the call was signed for. The row only passes if
            // both are present and matched, since the matching event differs from the
            // other one in its `source` alone.
            "envelope-account-and-region",
            json!({
                "account": [engine::emulator_account_id()],
                "region": [REGION],
                "source": ["com.demo.envelope"],
            }),
            json!({ "Source": "com.demo.envelope", "Detail": "{}" }),
            json!({ "Source": "com.demo.elsewhere", "Detail": "{}" }),
        ),
    ];

    for (case, pattern, _, _) in &cases {
        emulator.put_pattern_rule(case, pattern).await;
    }

    for (case, pattern, matching, not_matching) in &cases {
        let matched = emulator.put_event(matching.clone()).await;
        assert!(
            emulator
                .matched_rules(DEFAULT_BUS, &matched)
                .contains(&(*case).to_string()),
            "case {case:?}: pattern {pattern} must match {matching}, matched {:?}",
            emulator.matched_rules(DEFAULT_BUS, &matched)
        );
        let unmatched = emulator.put_event(not_matching.clone()).await;
        assert!(
            !emulator
                .matched_rules(DEFAULT_BUS, &unmatched)
                .contains(&(*case).to_string()),
            "case {case:?}: pattern {pattern} must NOT match {not_matching}, matched {:?}",
            emulator.matched_rules(DEFAULT_BUS, &unmatched)
        );
    }

    // The same envelope fields with a region the call was NOT signed for match nothing,
    // which is what makes the `envelope-account-and-region` row above an assertion about
    // the envelope rather than about `source`.
    emulator
        .put_pattern_rule(
            "envelope-region-mismatch",
            &json!({ "region": ["us-gov-west-1"], "source": ["com.demo.envelope"] }),
        )
        .await;
    let event_id = emulator
        .put_event(json!({ "Source": "com.demo.envelope", "Detail": "{}" }))
        .await;
    assert!(
        !emulator
            .matched_rules(DEFAULT_BUS, &event_id)
            .contains(&"envelope-region-mismatch".to_string()),
        "a pattern naming another region must match nothing, matched {:?}",
        emulator.matched_rules(DEFAULT_BUS, &event_id)
    );
}

/// A disabled rule matches nothing, and enabling it again makes it match: "You can
/// disable a rule using DisableRule."
#[tokio::test]
async fn a_disabled_rule_matches_nothing() {
    let emulator = emulator();
    let answer = emulator
        .call(
            "PutRule",
            json!({
                "Name": "switchable",
                "EventPattern": pattern_for("com.demo.switch").to_string(),
                "State": DISABLED,
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "PutRule answered {} {:?}",
        answer.status, answer.body
    );

    let entry = json!({ "Source": "com.demo.switch", "Detail": "{}" });
    let while_disabled = emulator.put_event(entry.clone()).await;
    assert_eq!(
        emulator.matched_rules(DEFAULT_BUS, &while_disabled),
        Vec::<String>::new(),
        "a disabled rule must match nothing"
    );

    let enabled = emulator
        .call(
            "PutRule",
            json!({
                "Name": "switchable",
                "EventPattern": pattern_for("com.demo.switch").to_string(),
                "State": ENABLED,
            }),
        )
        .await;
    assert_eq!(
        enabled.status, 200,
        "re-enabling answered {} {:?}",
        enabled.status, enabled.body
    );
    let while_enabled = emulator.put_event(entry).await;
    assert_eq!(
        emulator.matched_rules(DEFAULT_BUS, &while_enabled),
        vec!["switchable".to_string()],
        "an enabled rule matches the event its pattern names"
    );
}

/// `PutEvents` answers one outcome per entry, index-aligned with the request: an accepted
/// entry carries a unique `EventId`, a refused one carries its reason, and
/// `FailedEntryCount` counts the refused ones.
#[tokio::test]
async fn put_events_reports_one_unique_id_per_accepted_entry_and_counts_the_rest() {
    let emulator = emulator();
    // The modelled maximum batch (`PutEventsRequestEntryList`, `@length(min: 1,
    // max: 10)`), mixing every outcome one entry can have: accepted, malformed `Detail`,
    // a `Detail` that is valid JSON but not an object, and an entry naming a bus the
    // account does not hold.
    let answer = emulator
        .call(
            "PutEvents",
            json!({
                "Entries": [
                    { "Source": "com.demo.ok", "Detail": "{}" },
                    { "Source": "com.demo.malformed", "Detail": "not json" },
                    { "Source": "com.demo.ok", "DetailType": "Second", "Detail": r#"{"a":1}"# },
                    { "Source": "com.demo.not-an-object", "Detail": "[1,2,3]" },
                    { "Source": "com.demo.ok", "Detail": r#"{"b":2}"# },
                    { "Source": "com.demo.ok", "Resources": [target_arn("queue")], "Detail": "{}" },
                    { "Source": "com.demo.ok", "Detail": "{}", "EventBusName": DEFAULT_BUS },
                    { "Source": "com.demo.absent-bus", "Detail": "{}", "EventBusName": "never-created" },
                    { "Source": "com.demo.ok", "DetailType": "Eighth", "Detail": "{}" },
                    { "Source": "com.demo.ok", "Detail": "{}" },
                ],
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "PutEvents answered {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["FailedEntryCount"],
        json!(3),
        "three of the ten entries are refused, got {:?}",
        answer.body
    );
    let entries = answer.body["Entries"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        entries.len(),
        10,
        "one outcome per entry, index-aligned, got {entries:?}"
    );
    let accepted = [0_usize, 2, 4, 5, 6, 8, 9];
    let ids: BTreeSet<String> = accepted
        .into_iter()
        .map(|index| {
            entries[index]["EventId"]
                .as_str()
                .unwrap_or_else(|| panic!("entry {index} must carry an EventId, got {entries:?}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        ids.len(),
        accepted.len(),
        "every accepted entry carries its OWN id, got {ids:?}"
    );
    // Each id is a UUID: 8-4-4-4-12 lowercase hex, as EventBridge answers.
    for id in &ids {
        let groups: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "an EventId must be shaped like a UUID, got {id:?}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "an EventId carries only hex digits and dashes, got {id:?}"
        );
    }
    // A malformed entry and one naming an absent bus fail for DIFFERENT reasons, each on
    // its own row.
    for index in [1_usize, 3] {
        assert_eq!(
            entries[index]["ErrorCode"],
            json!("InvalidArgument"),
            "entry {index} must name its reason, got {entries:?}"
        );
        assert_eq!(
            entries[index]["EventId"],
            Value::Null,
            "a refused entry carries no id, got {entries:?}"
        );
    }
    assert_eq!(
        entries[7]["ErrorCode"],
        json!("ResourceNotFoundException"),
        "an entry naming a bus the account does not hold names THAT reason, got {entries:?}"
    );
    assert_eq!(
        emulator.recorded_event_count(DEFAULT_BUS),
        accepted.len(),
        "only the accepted entries are recorded"
    );
}

/// A request no rule can be built from is refused with the error for WHY — never stored,
/// never a 2xx. The rows span each reason the service states.
#[tokio::test]
async fn put_rule_refuses_a_rule_it_cannot_honour() {
    let emulator = emulator();
    // (case, request, expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "neither an event pattern nor a schedule",
            json!({ "Name": "empty-rule" }),
            "ValidationException",
        ),
        (
            "an event pattern that is not JSON at all",
            json!({ "Name": "bad-json", "EventPattern": "not json" }),
            "InvalidEventPatternException",
        ),
        (
            "an event pattern whose JSON is truncated",
            json!({ "Name": "truncated", "EventPattern": r#"{"source":["# }),
            "InvalidEventPatternException",
        ),
        (
            "an event pattern whose field holds no candidate list",
            json!({ "Name": "bad-shape", "EventPattern": r#"{"source":"com.demo"}"# }),
            "InvalidEventPatternException",
        ),
        (
            "an event pattern naming a filter that does not exist",
            json!({
                "Name": "bad-filter",
                "EventPattern": r#"{"source":[{"starts-with":"com."}]}"#,
            }),
            "InvalidEventPatternException",
        ),
        (
            "a schedule expression in neither documented form",
            json!({ "Name": "bad-schedule", "ScheduleExpression": "every 5 minutes" }),
            "ValidationException",
        ),
        (
            "a rate schedule with no unit",
            json!({ "Name": "bad-rate", "ScheduleExpression": "rate(5)" }),
            "ValidationException",
        ),
        (
            "a bus the account does not hold",
            json!({
                "Name": "no-such-bus",
                "EventBusName": "never-created",
                "EventPattern": pattern_for("com.demo").to_string(),
            }),
            "ResourceNotFoundException",
        ),
        (
            "a rate schedule of zero",
            json!({ "Name": "zero-rate", "ScheduleExpression": "rate(0 minutes)" }),
            "ValidationException",
        ),
        (
            "a rate schedule in a unit the reference does not list",
            json!({ "Name": "bad-unit", "ScheduleExpression": "rate(5 fortnights)" }),
            "ValidationException",
        ),
        (
            "a rate schedule whose value is signed",
            json!({ "Name": "signed-rate", "ScheduleExpression": "rate(-5 minutes)" }),
            "ValidationException",
        ),
        (
            "a rate schedule whose value carries a plus sign",
            json!({ "Name": "plus-rate", "ScheduleExpression": "rate(+5 minutes)" }),
            "ValidationException",
        ),
        (
            "a cron schedule with the wrong number of fields",
            json!({ "Name": "short-cron", "ScheduleExpression": "cron(0 20 * * ?)" }),
            "ValidationException",
        ),
        (
            "a wildcard with consecutive wildcard characters",
            json!({
                "Name": "double-star",
                "EventPattern": r#"{"source":[{"wildcard":"com.**.demo"}]}"#,
            }),
            "InvalidEventPatternException",
        ),
        (
            "an event pattern that is not a JSON object",
            json!({ "Name": "array-pattern", "EventPattern": r#"["com.demo"]"# }),
            "InvalidEventPatternException",
        ),
        (
            "a numeric filter whose operator does not exist",
            json!({
                "Name": "bad-numeric",
                "EventPattern": r#"{"detail":{"n":[{"numeric":["~",1]}]}}"#,
            }),
            "InvalidEventPatternException",
        ),
        (
            "a numeric filter whose operator carries no value",
            json!({
                "Name": "valueless-numeric",
                "EventPattern": r#"{"detail":{"n":[{"numeric":[">"]}]}}"#,
            }),
            "InvalidEventPatternException",
        ),
    ];

    for (case, request, expected_code) in &cases {
        let answer = emulator.call("PutRule", request.clone()).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {} {:?}",
            answer.status,
            answer.body
        );
        // The `__type` is pinned verbatim, namespace included, so the envelope's own
        // shape is asserted somewhere and not only its last segment.
        assert_eq!(
            answer.error_type(),
            *expected_code,
            "case {case:?}: expected the bare shape name as `__type`, got {:?}",
            answer.body
        );
    }

    assert_eq!(
        emulator.stored_rule_names(DEFAULT_BUS),
        Vec::<String>::new(),
        "a refused PutRule must store nothing"
    );
    assert_eq!(
        emulator.stored_bus_count(),
        0,
        "a refused PutRule must not materialise the default bus either"
    );

    // The forms the service DOES accept, so the rows above cannot be passing because
    // every schedule or pattern is refused.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "a rate schedule in each documented unit",
            json!({ "Name": "rates", "ScheduleExpression": "rate(1 minute)" }),
        ),
        (
            "a rate schedule in hours",
            json!({ "Name": "rate-hours", "ScheduleExpression": "rate(2 hours)" }),
        ),
        (
            "a rate schedule in days",
            json!({ "Name": "rate-days", "ScheduleExpression": "rate(3 days)" }),
        ),
        (
            "a six-field cron schedule",
            json!({ "Name": "cron", "ScheduleExpression": "cron(0 20 * * ? *)" }),
        ),
        (
            "a wildcard with one star",
            json!({
                "Name": "single-star",
                "EventPattern": r#"{"source":[{"wildcard":"com.*.demo"}]}"#,
            }),
        ),
        (
            "a numeric range",
            json!({
                "Name": "numeric-ok",
                "EventPattern": r#"{"detail":{"n":[{"numeric":[">=",1,"<",2]}]}}"#,
            }),
        ),
    ];
    for (case, request) in &accepted {
        let answer = emulator.call("PutRule", request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: an accepted form answered {} {:?}",
            answer.status, answer.body
        );
    }
}

/// An operation naming no stored rule answers `ResourceNotFoundException`, whichever
/// operation it is, and creates nothing.
#[tokio::test]
async fn an_operation_naming_no_stored_rule_answers_resource_not_found() {
    let emulator = emulator();
    let cases: Vec<(&str, Value)> = vec![
        ("DescribeRule", json!({ "Name": "never-created" })),
        ("DeleteRule", json!({ "Name": "never-created" })),
        ("ListTargetsByRule", json!({ "Rule": "never-created" })),
        (
            "PutTargets",
            json!({
                "Rule": "never-created",
                "Targets": [{ "Id": "one", "Arn": target_arn("queue") }],
            }),
        ),
    ];

    for (operation, request) in &cases {
        let answer = emulator.call(operation, request.clone()).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ResourceNotFoundException"),
            "case {operation}: expected 400 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
    assert_eq!(
        emulator.stored_rule_names(DEFAULT_BUS),
        Vec::<String>::new(),
        "a refused operation must not create the rule it did not find"
    );
}

/// A rule that still carries targets is not deleted: "Before you can delete the rule, you
/// must remove all associated targets, using RemoveTargets." A rule carrying none is
/// deleted and every reader stops reporting it.
#[tokio::test]
async fn delete_rule_removes_the_record_once_its_targets_are_gone() {
    let emulator = emulator();
    emulator
        .put_pattern_rule("with-targets", &pattern_for("com.demo.delete"))
        .await;
    emulator
        .put_pattern_rule("without-targets", &pattern_for("com.demo.delete"))
        .await;
    let answer = emulator
        .call(
            "PutTargets",
            json!({
                "Rule": "with-targets",
                "Targets": [{ "Id": "one", "Arn": target_arn("queue-one") }],
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "PutTargets answered {} {:?}",
        answer.status, answer.body
    );

    // `Force` is documented for MANAGED rules ("If this is a managed rule, created by an
    // Amazon Web Services service on your behalf, you must specify Force as True"), and
    // this emulator creates none, so it lifts nothing: both values refuse a rule that
    // still carries targets.
    for force in [Value::Null, json!(false), json!(true)] {
        let mut request = json!({ "Name": "with-targets" });
        if !force.is_null() {
            request["Force"] = force.clone();
        }
        let refused = emulator.call("DeleteRule", request).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ValidationException"),
            "case Force={force}: a rule with targets must not be deleted, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            emulator.stored_rule_names(DEFAULT_BUS),
            vec!["with-targets".to_string(), "without-targets".to_string()],
            "case Force={force}: the refused delete must leave both rules in place"
        );
    }

    let deleted = emulator
        .call("DeleteRule", json!({ "Name": "without-targets" }))
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteRule answered {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        emulator.stored_rule_names(DEFAULT_BUS),
        vec!["with-targets".to_string()],
        "only the deleted rule is gone from the store"
    );
    let described = emulator
        .call("DescribeRule", json!({ "Name": "without-targets" }))
        .await;
    assert_eq!(
        (described.status, described.error_code()),
        (400, "ResourceNotFoundException"),
        "a deleted rule is gone from every reader, got {} {:?}",
        described.status,
        described.body
    );
    assert_eq!(
        emulator.list_rule_names(&json!({})).await,
        vec!["with-targets".to_string()],
        "a deleted rule is gone from the listing too"
    );
}

/// A reporting call leaves the store exactly as it found it: the rows are every read this
/// service serves, each run twice, with the store compared before and after.
#[tokio::test]
async fn a_reporting_call_leaves_the_store_as_it_found_it() {
    let populated = emulator();
    populated
        .put_pattern_rule("read-me", &pattern_for("com.demo.read"))
        .await;
    populated
        .call(
            "PutTargets",
            json!({
                "Rule": "read-me",
                "Targets": [{ "Id": "one", "Arn": target_arn("queue-one") }],
            }),
        )
        .await;
    let event = populated
        .put_event(json!({ "Source": "com.demo.read", "Detail": "{}" }))
        .await;

    let before = (
        populated.stored_rule_names(DEFAULT_BUS),
        populated.recorded_event_count(DEFAULT_BUS),
        populated.matched_rules(DEFAULT_BUS, &event),
    );
    let reads: Vec<(&str, Value, u16)> = vec![
        ("DescribeRule", json!({ "Name": "read-me" }), 200),
        ("ListRules", json!({}), 200),
        ("ListRules", json!({ "NamePrefix": "read" }), 200),
        ("ListTargetsByRule", json!({ "Rule": "read-me" }), 200),
        // Still a scaffold in this crate, so its honest 501 is what it answers — and a
        // scaffold writes nothing either.
        (
            "ListRuleNamesByTarget",
            json!({ "TargetArn": target_arn("queue-one") }),
            501,
        ),
    ];

    for (operation, request, expected_status) in &reads {
        for pass in 1..=2 {
            let answer = populated.call(operation, request.clone()).await;
            assert_eq!(
                answer.status, *expected_status,
                "case {operation} pass {pass}: expected HTTP {expected_status}, got {} {:?}",
                answer.status, answer.body
            );
            let after = (
                populated.stored_rule_names(DEFAULT_BUS),
                populated.recorded_event_count(DEFAULT_BUS),
                populated.matched_rules(DEFAULT_BUS, &event),
            );
            assert_eq!(
                after, before,
                "case {operation} pass {pass}: a read must change nothing a later caller sees"
            );
        }
    }

    // A read on a FRESH store must not materialise the account's default bus either:
    // the bus comes into being only on a path that writes. On an empty store a read
    // that names a rule answers the modelled `ResourceNotFoundException` (400) while a
    // listing still answers 200 with nothing, and a scaffold still answers its 501 — so
    // each row's expected status is derived from the one it answered above.
    let reader = emulator();
    for (operation, request, implemented_status) in &reads {
        let names_a_rule = request.get("Name").is_some() || request.get("Rule").is_some();
        let expected_status = if *implemented_status == 200 && names_a_rule {
            400
        } else {
            *implemented_status
        };
        let answer = reader.call(operation, request.clone()).await;
        assert_eq!(
            answer.status, expected_status,
            "case {operation}: a read on an empty store must answer HTTP {expected_status}, \
             got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reader.stored_bus_count(),
            0,
            "case {operation}: a read must not materialise the default bus"
        );
    }
    // Nor must a REFUSED mutation.
    let refused = reader
        .call("DeleteRule", json!({ "Name": "never-created" }))
        .await;
    assert_eq!(
        (refused.status, reader.stored_bus_count()),
        (400, 0),
        "a refused mutation must not materialise the default bus, got {:?}",
        refused.body
    );
}

/// Matching is bounded by WORK, not by wall clock: a wildcard of many runs against a
/// long value spends work linear in the two sizes rather than their product, which is
/// what the match budget exists for — and it reports no match.
///
/// This drives the matcher directly (`state::event_pattern`), so the COST is assertable
/// rather than only the answer, and then pins the same input end to end through
/// `PutEvents`.
#[tokio::test]
async fn matching_a_large_value_against_a_large_wildcard_stays_cheap() {
    use service_com_amazonaws_cloudwatchevents_awsevents::state::event_pattern;

    // The adversarial shape for a backtracking matcher: many `*`-separated runs, and a
    // value made only of the run character so every run can start anywhere. The final
    // `b` is absent from the value, so no match is possible and a naive matcher would
    // explore every split.
    let wildcard = format!("{}b", "a*".repeat(1_000));
    let observed = "a".repeat(200_000);
    let pattern = event_pattern::parse(&json!({ "source": [{ "wildcard": wildcard }] }).to_string())
        .expect("the pattern is one this service accepts");
    let event = json!({ "source": observed });

    let mut budget = event_pattern::MatchBudget::new(event_pattern::MAX_MATCH_STEPS_PER_REQUEST);
    let matched = event_pattern::matches_within(&pattern, &event, &mut budget);
    assert!(
        !matched,
        "the value carries no {:?}, so the wildcard cannot match",
        'b'
    );
    // Linear in the two inputs: the matcher charges their lengths, so twice that is a
    // generous ceiling a quadratic walk could not stay under.
    let linear_cost = (wildcard.len() + observed.len()) as u64;
    assert!(
        budget.spent() <= linear_cost * 2,
        "matching must cost work linear in the inputs: spent {} steps for a {}-byte \
         wildcard against a {}-byte value (ceiling {})",
        budget.spent(),
        wildcard.len(),
        observed.len(),
        linear_cost * 2
    );
    assert!(
        !budget.exhausted(),
        "a linear match must not exhaust the request's budget, spent {}",
        budget.spent()
    );

    // The same input end to end: the rule is stored, the event is recorded, and it
    // matched nothing.
    let emulator = emulator();
    emulator
        .put_pattern_rule(
            "adversarial",
            &json!({ "source": [{ "wildcard": format!("{}b", "a*".repeat(1_000)) }] }),
        )
        .await;
    let event_id = emulator
        .put_event(json!({ "Source": "a".repeat(200_000), "Detail": "{}" }))
        .await;
    assert_eq!(
        emulator.matched_rules(DEFAULT_BUS, &event_id),
        Vec::<String>::new(),
        "the adversarial pattern must match nothing"
    );
}

/// A bus holds a bounded number of rules: the rule past the quota is refused with the
/// modelled `LimitExceededException`, while an UPDATE at the quota adds no record.
#[tokio::test]
async fn a_bus_holds_a_bounded_number_of_rules() {
    let emulator = emulator();
    for index in 0..RULES_PER_BUS {
        let answer = emulator
            .call(
                "PutRule",
                json!({
                    "Name": format!("rule-{index:04}"),
                    "EventPattern": pattern_for("com.demo.quota").to_string(),
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "rule {index} is within the quota: answered {} {:?}",
            answer.status, answer.body
        );
    }

    let over = emulator
        .call(
            "PutRule",
            json!({
                "Name": "one-too-many",
                "EventPattern": pattern_for("com.demo.quota").to_string(),
            }),
        )
        .await;
    assert_eq!(
        (over.status, over.error_code()),
        (400, "LimitExceededException"),
        "the rule past the quota must be refused, got {} {:?}",
        over.status,
        over.body
    );

    // An update replaces a record rather than adding one, so the quota does not refuse it.
    let update = emulator
        .call(
            "PutRule",
            json!({
                "Name": "rule-0000",
                "EventPattern": pattern_for("com.demo.updated").to_string(),
            }),
        )
        .await;
    assert_eq!(
        update.status, 200,
        "an update at the quota adds no record: answered {} {:?}",
        update.status, update.body
    );
    let described = emulator
        .call("DescribeRule", json!({ "Name": "rule-0000" }))
        .await;
    assert_eq!(
        described.string("EventPattern"),
        pattern_for("com.demo.updated").to_string(),
        "the update must still have been stored, got {:?}",
        described.body
    );
    assert_eq!(
        emulator.stored_rule_names(DEFAULT_BUS).len(),
        RULES_PER_BUS,
        "the store must hold exactly the quota"
    );
}

/// The per-bus event log keeps a bounded window of RECORDS, evicting oldest-first.
#[tokio::test]
async fn a_bus_retains_a_bounded_window_of_events_oldest_first() {
    let emulator = emulator();
    let mut first = String::new();
    let mut last = String::new();
    // One entry past the window, so exactly one event has to have been evicted.
    for index in 0..=RECORDED_EVENTS_PER_BUS {
        let id = emulator
            .put_event(json!({ "Source": format!("com.demo.{index}"), "Detail": "{}" }))
            .await;
        if index == 0 {
            first = id.clone();
        }
        last = id;
    }

    let held = emulator.recorded_event_count(DEFAULT_BUS);
    assert_eq!(
        held, RECORDED_EVENTS_PER_BUS,
        "the log must stay at its retention window, holds {held}"
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

/// The same log is bounded by the BYTES it retains as well as by its record count: a few
/// dozen large events are evicted long before the record bound.
#[tokio::test]
async fn a_bus_retains_a_bounded_number_of_bytes_oldest_first() {
    let emulator = emulator();
    // Each entry carries a ~200 KB detail, so about twenty fit in the window — far below
    // the record bound, which is the point: the byte bound is what binds here.
    let detail = json!({ "p": "x".repeat(200_000) }).to_string();
    let mut first = String::new();
    let mut last = String::new();
    for index in 0..40 {
        let id = emulator
            .put_event(json!({ "Source": "com.demo.big", "Detail": detail }))
            .await;
        if index == 0 {
            first = id.clone();
        }
        last = id;
    }

    assert!(
        emulator.retained_bytes(DEFAULT_BUS) <= RECORDED_BYTES_PER_BUS,
        "the log must stay inside its byte window, holds {} bytes",
        emulator.retained_bytes(DEFAULT_BUS)
    );
    assert!(
        emulator.recorded_event_count(DEFAULT_BUS) < 40,
        "the byte window must have evicted something, holds {} of 40 events",
        emulator.recorded_event_count(DEFAULT_BUS)
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

/// An event pattern is bounded in size and in nesting depth: just inside each bound is
/// stored, just outside is refused as an invalid pattern.
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

    // The rows differ along which bound is crossed and by how much: just inside, and
    // just outside.
    let cases: [(&str, String, u16); 4] = [
        (
            "a pattern just inside the size bound",
            json!({ "source": ["x".repeat(PATTERN_BYTES - 64)] }).to_string(),
            200,
        ),
        (
            "a pattern past the size bound",
            json!({ "source": ["x".repeat(PATTERN_BYTES)] }).to_string(),
            400,
        ),
        (
            // The builder wraps `{"x":["deep"]}` — itself one object level — in `levels`
            // more, so `PATTERN_DEPTH - 1` wrappers is the deepest pattern inside the
            // bound and one more is past it. Adjacent rows, so an off-by-one in the
            // implementation's limit fails.
            "a pattern at the nesting bound",
            nested(PATTERN_DEPTH - 1).to_string(),
            200,
        ),
        (
            "a pattern one level past the nesting bound",
            nested(PATTERN_DEPTH).to_string(),
            400,
        ),
    ];

    for (index, (case, pattern, expected_status)) in cases.into_iter().enumerate() {
        let answer = emulator
            .call(
                "PutRule",
                json!({ "Name": format!("bounded-{index}"), "EventPattern": pattern }),
            )
            .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status}, got {} {:?}",
            answer.status, answer.body
        );
        if expected_status != 200 {
            assert_eq!(
                answer.error_code(),
                "InvalidEventPatternException",
                "case {case:?}: expected InvalidEventPatternException, got {:?}",
                answer.body
            );
        }
    }
}

/// One `PutEvents` entry is bounded by the size EventBridge documents: the entry at the
/// limit is accepted and the one past it fails on its own row, leaving the rest of the
/// batch stored.
#[tokio::test]
async fn put_events_refuses_an_entry_past_the_documented_size() {
    let emulator = emulator();

    /// A `Detail` document of exactly `bytes` bytes: `{"p":"xx…x"}` is 8 bytes of
    /// envelope plus the padding.
    fn detail_of(bytes: usize) -> String {
        format!("{{\"p\":\"{}\"}}", "x".repeat(bytes - 8))
    }

    // Entry size is `Source` + `DetailType` + `Resources` + `Detail` (+ 14 for `Time`),
    // per "Calculating PutEvents event entry size". `Source` is one byte in both rows, so
    // the first lands exactly ON the limit and the second one byte past it.
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
        entries[1]["ErrorCode"],
        json!("InvalidArgument"),
        "an entry past the limit names its reason, got {:?}",
        entries[1]
    );
    assert_eq!(
        emulator.recorded_event_count(DEFAULT_BUS),
        1,
        "only the accepted entry is recorded"
    );
}

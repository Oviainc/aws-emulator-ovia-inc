// SPDX-License-Identifier: CC0-1.0

//! The `Negate` flag of a Call Analytics category rule, end to end over the wire.
//!
//! THE RULE UNDER TEST: a stored category rule RECORDS its filter's `Negate` flag — as
//! the request sent it, and as `false` when the request left the member out — so every
//! operation that reports a category reports the flag as a boolean, on every member of
//! the `Rule` union.
//!
//! The flag is an either/or over the two things a filter can match, and each of the
//! union's four filters documents both halves of it — "Set to TRUE to flag the absence
//! of the phrase that you specified in your request. Set to FALSE to flag the presence",
//! and the same sentence pair for silence, interruptions and sentiments
//! (`smithy_query(query="shape",
//! shape="Rule,NonTalkTimeFilter,InterruptionFilter,TranscriptFilter,SentimentFilter")`)
//! — so a rule naming no flag matches the PRESENCE of what it describes, which is the
//! `false` half. A reported rule that leaves the member out answers nothing at all about
//! which half the rule matches, and loses the flag when the reported rules are sent back
//! into an update.
//!
//! The rows span the axes the rule generalises over: which member of the union the rule
//! uses, whether the request names the flag (and which value it names), how many rules
//! the category holds, and which of the four reporting operations answers —
//! `CreateCallAnalyticsCategory`, `GetCallAnalyticsCategory`,
//! `UpdateCallAnalyticsCategory` and `ListCallAnalyticsCategories`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator.

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// What the service records a rule's `Negate` flag as when the request leaves the member
/// out: the half of the flag that matches the presence of what the filter describes.
const NEGATE_WHEN_OMITTED: bool = false;

/// Each rule of `rules` as the service RECORDS it: the same filter with its `Negate`
/// flag materialized as [`NEGATE_WHEN_OMITTED`] where the request left the member out,
/// and untouched where the request named it.
///
/// `pub(crate)` so every suite that asserts a reported rule list against the one it sent
/// (`crate::call_analytics_categories`, `crate::call_analytics_category_updates`,
/// `crate::call_analytics_rule_time_ranges`,
/// `crate::call_analytics_category_update_rule_validation`,
/// `crate::call_analytics_category_quota`,
/// `crate::get_call_analytics_category_reads`) expresses the rule through this one
/// function rather than a second copy of it.
pub(crate) fn as_recorded(rules: &Value) -> Value {
    let listed = rules
        .as_array()
        .unwrap_or_else(|| panic!("a rule list must be a JSON array, got {rules:?}"));
    Value::Array(listed.iter().map(as_recorded_rule).collect())
}

/// One rule as [`as_recorded`] reports it. A rule is a union, so it is a JSON object
/// carrying exactly one member: the filter.
fn as_recorded_rule(rule: &Value) -> Value {
    let mut recorded = rule.clone();
    let members = recorded
        .as_object_mut()
        .unwrap_or_else(|| panic!("a rule must be a JSON object, got {rule:?}"));
    for filter in members.values_mut() {
        let filter = filter
            .as_object_mut()
            .unwrap_or_else(|| panic!("a filter must be a JSON object, got {rule:?}"));
        filter
            .entry("Negate")
            .or_insert_with(|| json!(NEGATE_WHEN_OMITTED));
    }
    recorded
}

/// Every member of the `Rule` union, each with the members its filter models
/// `@required` plus a time range where one is useful — the whole union, from
/// `smithy_query(query="shape", shape="Rule")`, so a member the flag is not recorded on
/// fails here.
fn filters() -> Vec<(&'static str, Value)> {
    vec![
        // `NonTalkTimeFilter` models no required member; `Threshold` is a
        // `TimestampMilliseconds` (`@range(min: 0, max: 14400000)`).
        (
            "NonTalkTimeFilter",
            json!({"Threshold": 30_000, "RelativeTimeRange": {"First": 30}}),
        ),
        // `InterruptionFilter` models none either; `ParticipantRole` is the modelled
        // enum `AGENT | CUSTOMER`.
        (
            "InterruptionFilter",
            json!({"Threshold": 4_000, "ParticipantRole": "AGENT"}),
        ),
        // `TranscriptFilter$TranscriptFilterType` (enum `EXACT`) and `$Targets`
        // (`@length(min: 1)`) are both `@required`.
        (
            "TranscriptFilter",
            json!({
                "TranscriptFilterType": "EXACT",
                "Targets": ["goodbye"],
                "ParticipantRole": "CUSTOMER"
            }),
        ),
        // `SentimentFilter$Sentiments` is `@required` and `@length(min: 1, max: 1)`;
        // `SentimentValue` is `POSITIVE | NEGATIVE | NEUTRAL | MIXED`.
        (
            "SentimentFilter",
            json!({
                "Sentiments": ["NEGATIVE"],
                "AbsoluteTimeRange": {"StartTime": 0, "EndTime": 30_000}
            }),
        ),
    ]
}

/// What a row does with the flag: leave the member out, or name either of the `Boolean`
/// values it accepts.
fn flags() -> Vec<(&'static str, Option<bool>)> {
    vec![
        ("the flag left out", None),
        ("the flag sent false", Some(false)),
        ("the flag sent true", Some(true)),
    ]
}

/// One rule: `members` (a filter body) under `member` (its union member name), carrying
/// `flag` when the row names one.
fn rule(member: &str, members: &Value, flag: Option<bool>) -> Value {
    let mut body = members.clone();
    if let Some(flag) = flag {
        body["Negate"] = json!(flag);
    }
    json!({member: body})
}

/// One row: one rule, and the flag the reported rule must carry.
struct Row {
    case: String,
    name: String,
    rule: Value,
    expected_flag: bool,
}

/// The rows the table spans: every member of the `Rule` union against every way a
/// request can treat the flag.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, (member, members)) in filters().iter().enumerate() {
        for (flag_index, (flag_case, flag)) in flags().iter().enumerate() {
            rows.push(Row {
                case: format!("{member} / {flag_case}"),
                name: format!("negate.{index}_{flag_index}"),
                rule: rule(member, members, *flag),
                expected_flag: flag.unwrap_or(NEGATE_WHEN_OMITTED),
            });
        }
    }
    rows
}

/// The `CategoryProperties` of a successful answer.
fn properties(reply: &Reply, case: &str) -> Value {
    reply
        .ok(case)
        .get("CategoryProperties")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected CategoryProperties in the answer, got {:?}",
                reply.body
            )
        })
}

/// The `Rules` of a reported category.
fn reported_rules(properties: &Value, case: &str) -> Value {
    properties
        .get("Rules")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected Rules in {properties:?}"))
}

/// Assert `reported` is `sent` as the service records it, and that the one filter it
/// carries reports `expected_flag` as a JSON boolean.
fn records_the_flag(reported: &Value, sent: &Value, expected_flag: bool, case: &str) {
    assert_eq!(
        reported,
        &as_recorded(sent),
        "case {case:?}: expected the rules sent with every Negate flag recorded, got \
         {reported:?}"
    );
    let listed = reported
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Rules list, got {reported:?}"));
    for (index, rule) in listed.iter().enumerate() {
        let members = rule
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a rule object, got {rule:?}"));
        for (member, filter) in members {
            let flag = filter.get("Negate");
            assert_eq!(
                flag,
                Some(&json!(expected_flag)),
                "case {case:?}: Rules[{index}].{member}.Negate must be the boolean \
                 {expected_flag}, got {flag:?}"
            );
        }
    }
}

/// THE RULE on all four reporting surfaces: the create's answer, the read, the update's
/// answer and the listing each report the stored rule with its flag recorded.
#[tokio::test]
async fn every_rule_filter_reports_its_negate_flag_on_every_surface() {
    let transcribe = Transcribe::new();
    let rows = rows();

    for row in &rows {
        let case = row.case.as_str();
        let sent = json!([row.rule]);

        // The create's own echo of the category it stored.
        let created = transcribe
            .call(
                "CreateCallAnalyticsCategory",
                json!({"CategoryName": row.name, "Rules": sent}),
            )
            .await;
        let created = properties(&created, case);
        records_the_flag(
            &reported_rules(&created, case),
            &sent,
            row.expected_flag,
            &format!("{case} / create"),
        );

        // The read of the stored record.
        let read = transcribe
            .call(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": row.name}),
            )
            .await;
        let read = properties(&read, case);
        records_the_flag(
            &reported_rules(&read, case),
            &sent,
            row.expected_flag,
            &format!("{case} / read"),
        );
        assert_eq!(
            read, created,
            "case {case:?}: the read must report what the create stored"
        );
    }

    // The listing reports every stored category the same way the read does.
    let listed = transcribe
        .call("ListCallAnalyticsCategories", json!({"MaxResults": 100}))
        .await;
    let body = listed.ok("the listing");
    let categories = body
        .get("Categories")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a Categories list, got {body:?}"));
    for row in &rows {
        let case = format!("{} / list", row.case);
        let found = categories
            .iter()
            .find(|category| category["CategoryName"] == json!(row.name))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected {} among the listed categories, got {:?}",
                    row.name,
                    categories
                        .iter()
                        .map(|category| category["CategoryName"].clone())
                        .collect::<Vec<_>>()
                )
            });
        records_the_flag(
            &reported_rules(found, &case),
            &json!([row.rule]),
            row.expected_flag,
            &case,
        );
    }

    // The update REPLACES a category's rules, so every row's rule is also exercised as a
    // replacement — on a category created with a rule no row sends, so an answer echoing
    // the stored rules instead of the replacement cannot pass.
    let name = "negate.updates";
    transcribe
        .call(
            "CreateCallAnalyticsCategory",
            json!({
                "CategoryName": name,
                "Rules": [{"TranscriptFilter": {
                    "TranscriptFilterType": "EXACT",
                    "Targets": ["the rule the updates replace"],
                    "Negate": true
                }}]
            }),
        )
        .await
        .ok("the category the updates replace the rules of");
    for row in &rows {
        let case = format!("{} / update", row.case);
        let sent = json!([row.rule]);
        let updated = transcribe
            .call(
                "UpdateCallAnalyticsCategory",
                json!({"CategoryName": name, "Rules": sent}),
            )
            .await;
        let updated = properties(&updated, &case);
        records_the_flag(
            &reported_rules(&updated, &case),
            &sent,
            row.expected_flag,
            &case,
        );

        let read = transcribe
            .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
            .await;
        assert_eq!(
            properties(&read, &case),
            updated,
            "case {case:?}: the stored category must be the updated one"
        );
    }
}

/// THE RULE over a rule LIST: a category whose rules span every member of the union and
/// every way of treating the flag reports each one's flag, in the order the request
/// listed them. `RuleList` is modelled `@length(min: 1, max: 20)`, and the table is
/// twelve rules long.
#[tokio::test]
async fn a_rule_list_records_the_flag_of_every_rule_in_it() {
    let transcribe = Transcribe::new();
    let case = "a category whose rules span the union and all three flag settings";
    let name = "negate.every-rule";

    let rows = rows();
    let sent = Value::Array(rows.iter().map(|row| row.rule.clone()).collect());

    let created = transcribe
        .call(
            "CreateCallAnalyticsCategory",
            json!({"CategoryName": name, "Rules": sent}),
        )
        .await;
    let created = reported_rules(&properties(&created, case), case);
    assert_eq!(
        created,
        as_recorded(&sent),
        "case {case:?}: the create must report every rule with its flag recorded, got \
         {created:?}"
    );

    let read = transcribe
        .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
        .await;
    let read = reported_rules(&properties(&read, case), case);
    assert_eq!(
        read, created,
        "case {case:?}: the read must report what the create stored, got {read:?}"
    );

    // Each rule in the list reports the flag ITS row asked for, at its own position.
    let listed = read
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Rules list, got {read:?}"));
    assert_eq!(
        listed.len(),
        rows.len(),
        "case {case:?}: expected {} rules, got {listed:?}",
        rows.len()
    );
    for (row, reported) in rows.iter().zip(listed) {
        let found = reported
            .as_object()
            .and_then(|members| members.values().next())
            .and_then(|filter| filter.get("Negate"))
            .cloned();
        assert_eq!(
            found,
            Some(json!(row.expected_flag)),
            "case {:?}: expected Negate {} at its own position, got {found:?}",
            row.case,
            row.expected_flag
        );
    }
}

/// THE RULE as a ROUND TRIP: the rules a category reports, sent back into an update
/// unchanged, report exactly the same rules — a client that reads a category and writes
/// it back loses no flag, however many times it does so.
#[tokio::test]
async fn reported_rules_round_trip_through_an_update_unchanged() {
    let transcribe = Transcribe::new();

    for row in rows() {
        let case = row.case.as_str();
        let name = format!("roundtrip.{}", row.name);
        let created = transcribe
            .call(
                "CreateCallAnalyticsCategory",
                json!({"CategoryName": name, "Rules": [row.rule]}),
            )
            .await;
        let mut reported = reported_rules(&properties(&created, case), case);

        for pass in 1..=2 {
            let case = format!("{case} / round trip {pass}");
            let updated = transcribe
                .call(
                    "UpdateCallAnalyticsCategory",
                    json!({"CategoryName": name, "Rules": reported.clone()}),
                )
                .await;
            let answered = reported_rules(&properties(&updated, &case), &case);
            assert_eq!(
                answered, reported,
                "case {case:?}: writing the reported rules back must report them \
                 unchanged, got {answered:?}"
            );
            records_the_flag(&answered, &answered, row.expected_flag, &case);
            reported = answered;
        }
    }
}

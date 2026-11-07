// SPDX-License-Identifier: CC0-1.0

//! The documents this service keeps OPAQUE round-trip byte for byte.
//!
//! The rule under test: **a member this service accepts as a JSON DOCUMENT is persisted as the
//! exact text the request carried and reported back unchanged, and every value derived from it
//! is computed from those same bytes** — the service parses such a document only to judge it,
//! never to re-render it, so its whitespace, its member order, the form its numbers and
//! escapes are written in and the bytes of its non-ASCII characters all survive a write and
//! any number of reads.
//!
//! Two members carry such a document, and both are walked here:
//!
//! * `DashboardBody` — "The detailed information about the dashboard, including what widgets
//!   are included and their location on the dashboard."
//!   (`smithy_query(query="operation",operation="GetDashboard")` targets `DashboardBody`, a
//!   string the model constrains in no other way), read back by `GetDashboard` and measured by
//!   `ListDashboards`: "The size of the dashboard, in bytes." (`DashboardEntry$Size`, a long),
//!   so the reported size is the byte length of the submitted text.
//! * `Definition` — "The definition of the rule, as a JSON object."
//!   (`smithy_query(query="shape",shape="InsightRule")`, member `Definition`, an
//!   `InsightRuleDefinition`: a 1..8192 string whose pattern `^[\x00-\x7F]+$` admits ASCII
//!   only), read back by `DescribeInsightRules`.
//!
//! The rows differ along exactly what the rule generalises over: where the whitespace is, in
//! which order the members are written, which of several equivalent forms a number or a
//! character takes, whether a member is repeated, how many bytes a character takes, and
//! whether the document carries anything at all. The one document a recorded request happened
//! to submit is a single row.

use crate::harness::{build_router, ok};
use serde_json::{json, Value};

/// A string member of a response body.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Every dashboard document the rule is walked over: `(case, the exact text submitted)`.
///
/// Each one satisfies the widget schema `PutDashboard` judges a body against, so what a row
/// varies is the TEXT and nothing else.
fn dashboard_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "no whitespace between tokens",
            r#"{"widgets":[{"type":"text","x":0,"y":0,"width":6,"height":2,"properties":{"markdown":"compact"}}]}"#
                .to_string(),
        ),
        (
            "a space after every ':' and ','",
            r#"{"widgets": [{"type": "text", "x": 0, "y": 0, "width": 6, "height": 2, "properties": {"markdown": "compact"}}]}"#
                .to_string(),
        ),
        (
            "indented over several lines",
            "{\n  \"widgets\": [\n    {\n      \"type\": \"text\",\n      \"properties\": {\n        \"markdown\": \"indented\"\n      }\n    }\n  ]\n}"
                .to_string(),
        ),
        (
            "tabs and carriage returns between tokens",
            "{\t\"widgets\":\r\n[{\"type\":\t\"text\",\"properties\":{\"markdown\":\"tabbed\"}}]}"
                .to_string(),
        ),
        (
            "whitespace around the whole document",
            "   {\"widgets\":[]}\n".to_string(),
        ),
        (
            "members written in a different order",
            r#"{"widgets":[{"properties":{"markdown":"reordered"},"height":2,"width":6,"y":0,"x":0,"type":"text"}]}"#
                .to_string(),
        ),
        (
            "numbers written in forms that denote the same values",
            r#"{"widgets":[{"type":"text","x":0.0,"y":-0,"width":6.000,"height":2e0,"properties":{"markdown":"numbers"}}]}"#
                .to_string(),
        ),
        (
            "an integer past the precision of a double",
            r#"{"widgets":[{"type":"text","properties":{"markdown":"big","revision":9007199254740993}}]}"#
                .to_string(),
        ),
        (
            "characters written as escapes instead of literally",
            r#"{"widgets":[{"type":"text","properties":{"markdown":"\u0023 Heading\/One"}}]}"#
                .to_string(),
        ),
        (
            "multi-byte characters written literally",
            r#"{"widgets":[{"type":"text","properties":{"markdown":"héllo ✅ 日本"}}]}"#.to_string(),
        ),
        (
            "a member written twice",
            r#"{"widgets":[],"widgets":[]}"#.to_string(),
        ),
        ("a document carrying nothing", "{}".to_string()),
        ("no widget at all", r#"{"widgets":[]}"#.to_string()),
    ]
}

/// The one `DashboardEntry` `ListDashboards` reports for `name`.
async fn listed_entry(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Option<Value> {
    let listed = ok(
        router,
        "ListDashboards",
        &json!({"DashboardNamePrefix": name}),
    )
    .await;
    listed
        .get("DashboardEntries")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("DashboardName").and_then(Value::as_str) == Some(name))
        .cloned()
}

#[tokio::test]
async fn every_dashboard_document_reads_back_as_the_text_that_was_submitted() {
    let name = "opaque-board";
    for (case, document) in dashboard_documents() {
        let router = build_router();
        ok(
            &router,
            "PutDashboard",
            &json!({"DashboardName": name, "DashboardBody": document}),
        )
        .await;

        // Read twice: the answer is the submitted text, and it does not drift between reads.
        for read in 1..=2 {
            let got = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
            assert_eq!(
                text(&got, "DashboardBody").as_deref(),
                Some(document.as_str()),
                "case {case:?}, read {read}: expected the submitted document byte for byte, \
                 got {got}"
            );
        }

        // "The size of the dashboard, in bytes." (`DashboardEntry$Size`) — the byte length of
        // those same bytes, which for a document holding a multi-byte character is more than
        // its character count.
        let entry = listed_entry(&router, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the stored dashboard must be listed"));
        let expected_size = i64::try_from(document.len()).expect("a test document fits in i64");
        assert_eq!(
            entry.get("Size").and_then(Value::as_i64),
            Some(expected_size),
            "case {case:?}: expected Size to be the {expected_size} bytes submitted, got {entry}"
        );

        // Submitting the document a read returned changes nothing, so a client that
        // round-trips a dashboard converges instead of rewriting it on every pass.
        let returned = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        let returned = text(&returned, "DashboardBody").unwrap_or_default();
        ok(
            &router,
            "PutDashboard",
            &json!({"DashboardName": name, "DashboardBody": returned}),
        )
        .await;
        let after = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        assert_eq!(
            text(&after, "DashboardBody").as_deref(),
            Some(document.as_str()),
            "case {case:?}: writing back what the read returned must leave the document as it \
             was, got {after}"
        );
        let entry_after = listed_entry(&router, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the stored dashboard must be listed"));
        assert_eq!(
            entry_after.get("Size").and_then(Value::as_i64),
            Some(expected_size),
            "case {case:?}: the size after the round trip must still be {expected_size}, got \
             {entry_after}"
        );
    }
}

#[tokio::test]
async fn a_multi_byte_document_is_measured_in_bytes_rather_than_characters() {
    // The row that makes the units load-bearing: its characters outnumber neither its bytes nor
    // the other way round by accident — a document holding non-ASCII text has strictly more
    // bytes than characters, so a size counted in characters is a different number.
    let document = r#"{"widgets":[{"type":"text","properties":{"markdown":"héllo ✅ 日本"}}]}"#;
    assert!(
        document.len() > document.chars().count(),
        "case \"the multi-byte document\": the row must hold a character wider than one byte, \
         got {} bytes for {} characters",
        document.len(),
        document.chars().count()
    );

    let router = build_router();
    let name = "wide-board";
    ok(
        &router,
        "PutDashboard",
        &json!({"DashboardName": name, "DashboardBody": document}),
    )
    .await;
    let entry = listed_entry(&router, name)
        .await
        .expect("case \"the multi-byte document\": the stored dashboard must be listed");
    assert_eq!(
        entry.get("Size").and_then(Value::as_i64),
        Some(i64::try_from(document.len()).expect("a test document fits in i64")),
        "case \"the multi-byte document\": expected the byte length {}, got {entry}",
        document.len()
    );
}

#[tokio::test]
async fn an_update_stores_the_text_the_second_write_submitted() {
    let router = build_router();
    let name = "updated-board";
    // Two texts that denote the SAME structure and differ only in whitespace: the second write
    // is still a change, because what is stored is the text and not a parse of it.
    let first = r#"{"widgets":[{"type":"text","properties":{"markdown":"one"}}]}"#;
    let second = r#"{"widgets": [ {"type": "text", "properties": {"markdown": "one"}} ]}"#;
    for (case, document) in [("the first write", first), ("the second write", second)] {
        ok(
            &router,
            "PutDashboard",
            &json!({"DashboardName": name, "DashboardBody": document}),
        )
        .await;
        let got = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        assert_eq!(
            text(&got, "DashboardBody").as_deref(),
            Some(document),
            "case {case:?}: expected the text this write submitted, got {got}"
        );
        let entry = listed_entry(&router, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the stored dashboard must be listed"));
        assert_eq!(
            entry.get("Size").and_then(Value::as_i64),
            Some(i64::try_from(document.len()).expect("a test document fits in i64")),
            "case {case:?}: expected Size to follow the stored text, got {entry}"
        );
    }
}

/// Every Contributor Insights rule definition the rule is walked over: `(case, the exact text
/// submitted)`.
///
/// Each satisfies the rule syntax `PutInsightRule` judges a definition against and holds ASCII
/// only, which is all `InsightRuleDefinition`'s pattern admits — so, as above, what a row
/// varies is the text.
fn rule_definitions() -> Vec<(&'static str, String)> {
    vec![
        (
            "no whitespace between tokens",
            r#"{"Schema":{"Name":"CloudWatchLogRule","Version":1},"LogGroupNames":["API-Gateway-Access-Logs"],"LogFormat":"JSON","Contribution":{"Keys":["$.ip"]}}"#
                .to_string(),
        ),
        (
            "a space after every ':' and ','",
            r#"{"Schema": {"Name": "CloudWatchLogRule", "Version": 1}, "LogGroupNames": ["API-Gateway-Access-Logs"], "LogFormat": "JSON", "Contribution": {"Keys": ["$.ip"]}}"#
                .to_string(),
        ),
        (
            "indented over several lines",
            "{\n  \"Schema\": {\n    \"Name\": \"CloudWatchLogRule\",\n    \"Version\": 1\n  },\n  \"LogGroupNames\": [\n    \"API-Gateway-Access-Logs\"\n  ],\n  \"LogFormat\": \"JSON\",\n  \"Contribution\": {\n    \"Keys\": [\n      \"$.ip\"\n    ]\n  }\n}"
                .to_string(),
        ),
        (
            "members written in a different order, with whitespace around the document",
            " {\"Contribution\":{\"Keys\":[\"$.ip\"]},\"LogFormat\":\"JSON\",\"LogGroupNames\":[\"API-Gateway-Access-Logs\"],\"Schema\":{\"Version\":1,\"Name\":\"CloudWatchLogRule\"}}\n"
                .to_string(),
        ),
        (
            "characters written as escapes instead of literally",
            r#"{"Schema":{"Name":"CloudWatchLogRule","Version":1},"LogGroupNames":["\u0041PI-Gateway-Access-Logs"],"LogFormat":"JSON","Contribution":{"Keys":["$.ip"]}}"#
                .to_string(),
        ),
        (
            "a filter value written in exponent form",
            r#"{"Schema":{"Name":"CloudWatchLogRule","Version":1},"LogGroupNames":["API-Gateway-Access-Logs"],"LogFormat":"JSON","AggregateOn":"Sum","Contribution":{"Keys":["$.ip"],"ValueOf":"$.bytes","Filters":[{"Match":"$.status","In":[2e2]}]}}"#
                .to_string(),
        ),
        (
            "tabs between tokens",
            "{\t\"Schema\":{\"Name\":\"CloudWatchLogRule\",\"Version\":1},\t\"LogGroupNames\":[\"API-Gateway-Access-Logs\"],\"LogFormat\":\"CLF\",\"Contribution\":{\"Keys\":[\"$.ip\"]}}"
                .to_string(),
        ),
    ]
}

#[tokio::test]
async fn every_rule_definition_reads_back_as_the_text_that_was_submitted() {
    let name = "opaque-rule";
    for (case, definition) in rule_definitions() {
        let router = build_router();
        ok(
            &router,
            "PutInsightRule",
            &json!({"RuleName": name, "RuleDefinition": definition}),
        )
        .await;

        for read in 1..=2 {
            let described = ok(&router, "DescribeInsightRules", &json!({})).await;
            let rule = described
                .get("InsightRules")
                .and_then(Value::as_array)
                .and_then(|rules| {
                    rules
                        .iter()
                        .find(|rule| rule.get("Name").and_then(Value::as_str) == Some(name))
                        .cloned()
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the stored rule must be described, got {described}")
                });
            assert_eq!(
                text(&rule, "Definition").as_deref(),
                Some(definition.as_str()),
                "case {case:?}, read {read}: expected the submitted definition byte for byte, \
                 got {rule}"
            );
        }
    }
}

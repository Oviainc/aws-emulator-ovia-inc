// SPDX-License-Identifier: CC0-1.0

//! The TEXT `PutInsightRule` refuses a rule body it cannot read as a Contributor Insights
//! rule definition with.
//!
//! The rule under test: **a `RuleDefinition` that is not a readable Contributor Insights rule
//! definition is refused `InvalidParameterValue` with the service's fixed reason-code
//! sentence for an unreadable rule body — the same sentence whatever part of the syntax the
//! body gets wrong, carrying neither the rule name nor anything of the submitted body** —
//! while a body the syntax does allow is accepted and read back. The model constrains
//! `PutInsightRuleInput$RuleDefinition` only as a 1..8192 ASCII string
//! (`smithy_query(query="operation",operation="PutInsightRule")`), so the body's syntax is
//! the service's own check, and this is how the service reports the failure of that check.
//!
//! The rows differ along WHICH part of the rule syntax the body gets wrong (it is not JSON at
//! all, it is JSON that is not an object, it is an object missing or mis-naming the schema,
//! the log groups, the log format, the contribution or the aggregation), along the body's SIZE
//! (the minimal object through a body of thousands of characters) and along the rule NAME the
//! request carries — so the recorded minimal-object body is one row among many, and a single
//! fixed sentence has to answer all of them.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, error_message, ok};

/// The sentence this service refuses an unreadable Contributor Insights rule body with: a
/// reason code naming what was wrong with the body, and a fixed sentence for that class with
/// no variable part.
const UNPARSEABLE_RULE_BODY: &str = "INVALID_RULE_BODY: The RuleBody could not be parsed.";

/// The wire error code of that refusal, as an SDK of this service sees it
/// (`PutInsightRule` errors `InvalidParameterValueException`, awsQueryError
/// `InvalidParameterValue`).
const REFUSAL_CODE: &str = "InvalidParameterValueException";

/// A rule definition in the Contributor Insights rule syntax, with `overrides` merged over
/// it. An override of `null` REMOVES the member, which is how a row drops one.
fn definition(overrides: Value) -> String {
    let mut rule = json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": ["API-Gateway-Access-Logs"],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    });
    let members = rule.as_object_mut().expect("the base rule is an object");
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        if value.is_null() {
            members.remove(member);
        } else {
            members.insert(member.clone(), value.clone());
        }
    }
    serde_json::to_string(&rule).expect("the definition serializes")
}

/// Every rule name `DescribeInsightRules` reported.
fn reported_names(body: &Value) -> Vec<String> {
    body.get("InsightRules")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .filter_map(|rule| rule.get("Name").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn every_unreadable_rule_body_is_refused_with_the_one_reason_code_sentence() {
    // A body whose log groups are named by a few hundred non-strings: well inside the
    // member's 8192-character bound, so the service and not the modelled constraint refuses
    // it — and long enough that a message echoing the body could not hide it.
    let long_invalid_body = definition(json!({"LogGroupNames": (0..300).collect::<Vec<i32>>()}));
    assert!(
        long_invalid_body.len() > 1_000 && long_invalid_body.len() <= 8_192,
        "case \"a long body\": the row must stay inside the member's bound, got {} characters",
        long_invalid_body.len()
    );

    // Rows: (case, the rule name the request carries, the body it submits).
    let cases: Vec<(&str, String, String)> = vec![
        // The minimal JSON object: syntactically valid JSON, no rule definition at all. The
        // name is the shortest the member admits (1..128), and no character of the fixed
        // sentence, so a name echoed into the message would show.
        ("the minimal JSON object", "Q".to_string(), "{}".to_string()),
        (
            "an object with members the syntax does not define",
            "unrelated-members".to_string(),
            json!({"Rule": "contributors", "Version": 1}).to_string(),
        ),
        (
            "text that is not JSON",
            "not-json".to_string(),
            "not json at all".to_string(),
        ),
        (
            "JSON that is an array",
            "json-array".to_string(),
            "[]".to_string(),
        ),
        (
            "JSON that is a string",
            "json-string".to_string(),
            "\"rule\"".to_string(),
        ),
        (
            "JSON that is a number",
            "json-number".to_string(),
            "17".to_string(),
        ),
        (
            "truncated JSON",
            "truncated".to_string(),
            "{\"Schema\": {\"Name\": \"CloudWatchLogRule\"".to_string(),
        ),
        (
            "no schema",
            "no-schema".to_string(),
            definition(json!({"Schema": null})),
        ),
        (
            "a schema at another version",
            "schema-v99".to_string(),
            definition(json!({"Schema": {"Name": "CloudWatchLogRule", "Version": 99}})),
        ),
        (
            "the schema of a managed rule",
            "managed-schema".to_string(),
            definition(json!({"Schema": {"Name": "ServiceLogRule", "Version": 1}})),
        ),
        (
            "no log group named",
            "no-log-group".to_string(),
            definition(json!({"LogGroupNames": null})),
        ),
        (
            "an empty log group list",
            "empty-log-groups".to_string(),
            definition(json!({"LogGroupNames": []})),
        ),
        (
            "log group names that are not strings",
            "numeric-log-groups".to_string(),
            definition(json!({"LogGroupNames": [7]})),
        ),
        ("a long body", "long-body".to_string(), long_invalid_body),
        (
            "no log format",
            "no-log-format".to_string(),
            definition(json!({"LogFormat": null})),
        ),
        (
            "a log format the syntax does not name",
            "text-log-format".to_string(),
            definition(json!({"LogFormat": "TEXT"})),
        ),
        (
            "no contribution",
            "no-contribution".to_string(),
            definition(json!({"Contribution": null})),
        ),
        (
            "a contribution with no keys",
            "no-keys".to_string(),
            definition(json!({"Contribution": {"ValueOf": "$.bytes"}})),
        ),
        (
            "a contribution with five keys",
            "five-keys".to_string(),
            definition(json!({"Contribution": {"Keys": ["a", "b", "c", "d", "e"]}})),
        ),
        (
            "a contribution with five filters",
            "five-filters".to_string(),
            definition(json!({"Contribution": {
                "Keys": ["$.ip"],
                "Filters": [
                    {"Match": "$.a"}, {"Match": "$.b"}, {"Match": "$.c"},
                    {"Match": "$.d"}, {"Match": "$.e"},
                ],
            }})),
        ),
        (
            "filters that are not a list",
            "string-filters".to_string(),
            definition(json!({"Contribution": {"Keys": ["$.ip"], "Filters": "none"}})),
        ),
        (
            "an aggregation the syntax does not name",
            "averaged".to_string(),
            definition(json!({"AggregateOn": "Average"})),
        ),
        (
            "summing with no field to sum",
            // A 128-character name, the longest the member admits: the refusal reads the same
            // whatever name the request carries.
            "n".repeat(128),
            definition(json!({"AggregateOn": "Sum"})),
        ),
    ];

    for (case, name, body) in cases {
        let router = build_router();
        let (status, response) = call(
            &router,
            "PutInsightRule",
            &json!({"RuleName": name, "RuleDefinition": body}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            REFUSAL_CODE,
            "case {case:?}: expected {REFUSAL_CODE}, got {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, UNPARSEABLE_RULE_BODY,
            "case {case:?}: expected the fixed sentence {UNPARSEABLE_RULE_BODY:?}, got \
             {message:?}"
        );
        // The sentence has no variable part: neither the rule name nor the body it rejected
        // appears in it.
        assert!(
            !message.contains(name.as_str()) && !message.contains(&body),
            "case {case:?}: the refusal must echo neither the rule name nor the body, got \
             {message:?}"
        );
        // A refused request persists nothing.
        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        assert_eq!(
            reported_names(&described),
            Vec::<String>::new(),
            "case {case:?}: a refused PutInsightRule must persist nothing, got {described}"
        );
    }
}

#[tokio::test]
async fn a_readable_rule_body_is_stored_rather_than_refused() {
    // The counter-rows: bodies the Contributor Insights rule syntax does allow, spanning both
    // log formats, both ways of naming log groups, the four-key and four-filter bounds and
    // both aggregations. Without them the refusal above would also pass against a handler
    // that refused every body.
    let cases: Vec<(&str, String)> = vec![
        ("the documented example", definition(json!({}))),
        (
            "the CLF log format with mapped fields",
            definition(json!({
                "LogFormat": "CLF",
                "Fields": {"4": "IpAddress", "7": "StatusCode"},
                "Contribution": {"Keys": ["IpAddress"]},
            })),
        ),
        (
            "log groups named by ARN",
            definition(json!({
                "LogGroupARNArray": ["arn:aws:logs:us-east-1:000000000000:log-group:/app"],
                "LogGroupNames": null,
            })),
        ),
        (
            "four contribution keys and four filters",
            definition(json!({"Contribution": {
                "Keys": ["$.ip", "$.userAgent", "$.path", "$.status"],
                "Filters": [
                    {"Match": "$.httpMethod", "In": ["PUT"]},
                    {"Match": "$.status", "In": [200]},
                    {"Match": "$.path", "StartsWith": ["/api"]},
                    {"Match": "$.userAgent", "NotIn": ["bot"]},
                ],
            }})),
        ),
        (
            "counting occurrences",
            definition(json!({"AggregateOn": "Count"})),
        ),
        (
            "summing the field the contribution names",
            definition(json!({
                "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.requestBytes"},
                "AggregateOn": "Sum",
            })),
        ),
    ];

    for (case, body) in cases {
        let router = build_router();
        let (status, response) = call(
            &router,
            "PutInsightRule",
            &json!({"RuleName": "readable-rule", "RuleDefinition": body}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a body the syntax allows must be accepted, got {status} with \
             {response}"
        );
        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        let stored = described
            .get("InsightRules")
            .and_then(Value::as_array)
            .and_then(|rules| rules.first())
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            stored.get("Definition").and_then(Value::as_str),
            Some(body.as_str()),
            "case {case:?}: the accepted body must round-trip through DescribeInsightRules, \
             got {described}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CloudWatch Contributor Insights rule configuration loop:
//! `PutInsightRule` creating and updating a rule, and `DescribeInsightRules` reading back
//! what it persisted.
//!
//! Every table row is a distinct input along what the rule under test generalises over (the
//! rule name's length and characters, which state the request asks for or omits, the log
//! format, how the log groups are named, how many contribution keys and filters the
//! definition carries, what it aggregates on, whether the rule already exists, and how many
//! rules a page holds), so the recorded happy path is one row among many rather than the
//! specification.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};

/// The `Schema` every rule a caller creates carries: "For rules that you create, this is
/// always `{"Name": "CloudWatchLogRule", "Version": 1}`" (`InsightRule$Schema`).
const CREATED_RULE_SCHEMA: &str = "{\"Name\": \"CloudWatchLogRule\", \"Version\": 1}";

/// A `PutInsightRule` request with only the members the model makes required.
fn put_request(name: &str, definition: &str) -> Value {
    json!({"RuleName": name, "RuleDefinition": definition})
}

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

/// The rule `DescribeInsightRules` reported under `name`, if any.
fn rule_named(body: &Value, name: &str) -> Option<Value> {
    body.get("InsightRules")?
        .as_array()?
        .iter()
        .find(|rule| rule.get("Name").and_then(Value::as_str) == Some(name))
        .cloned()
}

/// Every rule name `DescribeInsightRules` reported, in the order it reported them.
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
async fn every_rule_form_round_trips_through_describe_insight_rules() {
    // Rows differ along the name's length and characters, the definition's log format, how
    // its log groups are named, how many keys and filters its contribution carries and what
    // it aggregates on, which state the request asks for (or omits), whether the rule is
    // evaluated on transformed logs, and whether it is created with tags.
    let four_keys = json!({"Keys": ["$.ip", "$.userAgent", "$.path", "$.status"]});
    let four_filters = json!({
        "Keys": ["$.ip"],
        "ValueOf": "$.requestBytes",
        "Filters": [
            {"Match": "$.httpMethod", "In": ["PUT"]},
            {"Match": "$.status", "In": [200]},
            {"Match": "$.path", "StartsWith": ["/api"]},
            {"Match": "$.userAgent", "NotIn": ["bot"]},
        ],
    });
    let cases: Vec<(&str, String, String, Value, &str, bool)> = vec![
        (
            "required members only",
            "insight-rule".to_string(),
            definition(json!({})),
            json!({}),
            "ENABLED",
            false,
        ),
        (
            "a one-character name, explicitly enabled",
            "r".to_string(),
            definition(json!({"LogGroupNames": ["one", "two", "three"]})),
            json!({"RuleState": "ENABLED"}),
            "ENABLED",
            false,
        ),
        (
            "a 128-character name, explicitly disabled",
            "n".repeat(128),
            definition(json!({"AggregateOn": "Count"})),
            json!({"RuleState": "DISABLED"}),
            "DISABLED",
            false,
        ),
        (
            "a name with the spaces and punctuation the pattern allows",
            "My rule (v2) / high-cardinality".to_string(),
            definition(json!({
                "LogFormat": "CLF",
                "Fields": {"4": "IpAddress", "7": "StatusCode"},
                "Contribution": {"Keys": ["IpAddress"], "ValueOf": "StatusCode"},
                "AggregateOn": "Sum",
            })),
            json!({"ApplyOnTransformedLogs": true}),
            "ENABLED",
            true,
        ),
        (
            "log groups named by ARN, four contribution keys",
            "arn-selected-rule".to_string(),
            definition(json!({
                "LogGroupARNArray": [
                    "arn:aws:logs:us-east-1:000000000000:log-group:/aws/lambda/one",
                    "arn:aws:logs:us-east-1:000000000000:log-group:/aws/lambda/two",
                ],
                "LogGroupNames": null,
                "Contribution": four_keys,
            })),
            json!({"RuleState": "DISABLED", "ApplyOnTransformedLogs": false}),
            "DISABLED",
            false,
        ),
        (
            "four filters, a summed value and tags",
            "summing-rule".to_string(),
            definition(json!({"Contribution": four_filters, "AggregateOn": "Sum"})),
            json!({"Tags": [{"Key": "team", "Value": "observability"}]}),
            "ENABLED",
            false,
        ),
    ];

    for (case, name, rule_definition, extra, expected_state, expected_transformed) in cases {
        let router = build_router();
        let mut request = put_request(&name, &rule_definition);
        for (member, value) in extra.as_object().expect("the row is an object") {
            request[member.as_str()] = value.clone();
        }

        let put = ok(&router, "PutInsightRule", &request).await;
        assert_eq!(
            put,
            json!({}),
            "case {case:?}: PutInsightRule models no output member, got {put}"
        );

        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        let rule = rule_named(&described, &name)
            .unwrap_or_else(|| panic!("case {case:?}: expected the rule back, got {described}"));
        assert_eq!(
            rule.get("Definition").and_then(Value::as_str),
            Some(rule_definition.as_str()),
            "case {case:?}: the definition must round-trip verbatim, got {rule}"
        );
        assert_eq!(
            rule.get("State").and_then(Value::as_str),
            Some(expected_state),
            "case {case:?}: expected state {expected_state:?}, got {rule}"
        );
        assert_eq!(
            rule.get("Schema").and_then(Value::as_str),
            Some(CREATED_RULE_SCHEMA),
            "case {case:?}: a rule the caller created carries the CloudWatchLogRule schema, \
             got {rule}"
        );
        assert_eq!(
            rule.get("ManagedRule").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a rule the caller created is not managed, got {rule}"
        );
        assert_eq!(
            rule.get("ApplyOnTransformedLogs").and_then(Value::as_bool),
            Some(expected_transformed),
            "case {case:?}: expected ApplyOnTransformedLogs {expected_transformed}, got {rule}"
        );
        // Reading twice changes nothing a later caller sees.
        let again = ok(&router, "DescribeInsightRules", &json!({})).await;
        assert_eq!(
            again, described,
            "case {case:?}: a second DescribeInsightRules must report the same rules, got {again}"
        );
    }
}

#[tokio::test]
async fn a_definition_outside_the_rule_syntax_is_refused() {
    // Rows differ along which part of the Contributor Insights rule syntax the definition
    // gets wrong — and the accepted rows span the shapes the syntax does allow, including
    // the four-key and four-filter bounds and both aggregations.
    let cases: Vec<(&str, String, bool)> = vec![
        ("the documented example", definition(json!({})), true),
        (
            "a four-key contribution",
            definition(json!({"Contribution": {"Keys": ["a", "b", "c", "d"]}})),
            true,
        ),
        (
            "a five-key contribution",
            definition(json!({"Contribution": {"Keys": ["a", "b", "c", "d", "e"]}})),
            false,
        ),
        (
            "four filters",
            definition(json!({"Contribution": {
                "Keys": ["$.ip"],
                "Filters": [{"Match": "$.a"}, {"Match": "$.b"}, {"Match": "$.c"}, {"Match": "$.d"}],
            }})),
            true,
        ),
        (
            "five filters",
            definition(json!({"Contribution": {
                "Keys": ["$.ip"],
                "Filters": [
                    {"Match": "$.a"}, {"Match": "$.b"}, {"Match": "$.c"},
                    {"Match": "$.d"}, {"Match": "$.e"},
                ],
            }})),
            false,
        ),
        ("aggregating on Count", definition(json!({"AggregateOn": "Count"})), true),
        (
            "aggregating on Sum over the field the contribution names",
            definition(json!({
                "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.requestBytes"},
                "AggregateOn": "Sum",
            })),
            true,
        ),
        (
            "aggregating on Sum with no field to sum",
            definition(json!({"AggregateOn": "Sum"})),
            false,
        ),
        (
            "aggregating on Sum with an empty field to sum",
            definition(json!({
                "Contribution": {"Keys": ["$.ip"], "ValueOf": ""},
                "AggregateOn": "Sum",
            })),
            false,
        ),
        (
            "aggregating on a statistic the syntax does not name",
            definition(json!({"AggregateOn": "Average"})),
            false,
        ),
        ("the CLF log format", definition(json!({"LogFormat": "CLF"})), true),
        (
            "a log format the syntax does not name",
            definition(json!({"LogFormat": "TEXT"})),
            false,
        ),
        ("no log format at all", definition(json!({"LogFormat": null})), false),
        (
            "log groups named by ARN",
            definition(json!({
                "LogGroupARNArray": ["arn:aws:logs:us-east-1:000000000000:log-group:/app"],
                "LogGroupNames": null,
            })),
            true,
        ),
        ("no log group named", definition(json!({"LogGroupNames": null})), false),
        (
            "an empty log group list",
            definition(json!({"LogGroupNames": []})),
            false,
        ),
        (
            "log group names that are not strings",
            definition(json!({"LogGroupNames": [7]})),
            false,
        ),
        ("no schema", definition(json!({"Schema": null})), false),
        (
            "a schema at another version",
            definition(json!({"Schema": {"Name": "CloudWatchLogRule", "Version": 99}})),
            false,
        ),
        (
            "a schema with no version",
            definition(json!({"Schema": {"Name": "CloudWatchLogRule"}})),
            false,
        ),
        (
            "the schema of a managed rule",
            definition(json!({"Schema": {"Name": "ServiceLogRule", "Version": 1}})),
            false,
        ),
        ("no contribution", definition(json!({"Contribution": null})), false),
        (
            "a contribution with no keys",
            definition(json!({"Contribution": {"ValueOf": "$.bytes"}})),
            false,
        ),
        (
            "a contribution with an empty key list",
            definition(json!({"Contribution": {"Keys": []}})),
            false,
        ),
        (
            "filters that are not a list",
            definition(json!({"Contribution": {"Keys": ["$.ip"], "Filters": "none"}})),
            false,
        ),
        ("a definition that is not JSON", "not json at all".to_string(), false),
        ("a definition that is a JSON array", "[]".to_string(), false),
        ("a definition that is a JSON string", "\"rule\"".to_string(), false),
    ];

    for (case, rule_definition, expected_ok) in cases {
        let router = build_router();
        let (status, body) = call(
            &router,
            "PutInsightRule",
            &put_request("syntax-rule", &rule_definition),
        )
        .await;
        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        if expected_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: a definition the syntax allows must be accepted, got {status} \
                 {body}"
            );
            let rule = rule_named(&described, "syntax-rule").unwrap_or_default();
            assert_eq!(
                rule.get("Definition").and_then(Value::as_str),
                Some(rule_definition.as_str()),
                "case {case:?}: the accepted definition must round-trip, got {described}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected a 400 InvalidParameterValueException, got {status} {body}"
            );
            // A refused request persists nothing.
            assert_eq!(
                reported_names(&described),
                Vec::<String>::new(),
                "case {case:?}: a refused PutInsightRule must persist nothing, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn rule_state_is_accepted_only_where_the_model_names_it() {
    // "The state of the rule. Valid values are ENABLED and DISABLED."
    // (`PutInsightRuleInput$RuleState`) — rows span both named values, the omitted member,
    // and values that only differ from a named one in case or wording.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        ("enabled", json!("ENABLED"), Some("ENABLED")),
        ("disabled", json!("DISABLED"), Some("DISABLED")),
        ("omitted", Value::Null, Some("ENABLED")),
        ("lowercase", json!("enabled"), None),
        ("mixed case", json!("Disabled"), None),
        ("another word entirely", json!("ACTIVE"), None),
        ("a named value with trailing space", json!("ENABLED "), None),
    ];

    for (case, requested, expected_state) in cases {
        let router = build_router();
        let mut request = put_request("state-rule", &definition(json!({})));
        if !requested.is_null() {
            request["RuleState"] = requested.clone();
        }

        let (status, body) = call(&router, "PutInsightRule", &request).await;
        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        match expected_state {
            Some(state) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the state to be accepted, got {status} {body}"
                );
                assert_eq!(
                    rule_named(&described, "state-rule")
                        .as_ref()
                        .and_then(|rule| rule.get("State"))
                        .and_then(Value::as_str),
                    Some(state),
                    "case {case:?}: expected state {state:?}, got {described}"
                );
            }
            None => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidParameterValueException"),
                    "case {case:?}: expected a 400 InvalidParameterValueException, got {status} \
                     {body}"
                );
                assert_eq!(
                    reported_names(&described),
                    Vec::<String>::new(),
                    "case {case:?}: a refused PutInsightRule must persist nothing, got {described}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_update_replaces_the_rule_it_names_and_creates_none() {
    let router = build_router();
    ok(
        &router,
        "PutInsightRule",
        &json!({
            "RuleName": "updated-rule",
            "RuleDefinition": definition(json!({})),
            "RuleState": "ENABLED",
            "Tags": [{"Key": "stage", "Value": "create"}],
        }),
    )
    .await;
    ok(
        &router,
        "PutInsightRule",
        &put_request("other-rule", &definition(json!({}))),
    )
    .await;

    // Rows differ in what the update changes: the definition's log format and aggregation,
    // the state, whether the rule is evaluated on transformed logs, and whether the update
    // carries tags at all.
    let updates: Vec<(&str, Value, &str, bool)> = vec![
        (
            "a summed definition and a disabled state",
            json!({
                "RuleName": "updated-rule",
                "RuleDefinition": definition(json!({
                    "Contribution": {"Keys": ["$.ip"], "ValueOf": "$.bytes"},
                    "AggregateOn": "Sum",
                })),
                "RuleState": "DISABLED",
                "ApplyOnTransformedLogs": true,
                "Tags": [{"Key": "stage", "Value": "update"}],
            }),
            "DISABLED",
            true,
        ),
        (
            "back to the bare required members",
            put_request("updated-rule", &definition(json!({"LogFormat": "CLF"}))),
            "ENABLED",
            false,
        ),
    ];

    for (case, update, expected_state, expected_transformed) in updates {
        ok(&router, "PutInsightRule", &update).await;
        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        // An update replaces the rule it names and creates no second one.
        assert_eq!(
            reported_names(&described),
            vec!["other-rule".to_string(), "updated-rule".to_string()],
            "case {case:?}: an update must replace the rule, not add one, got {described}"
        );

        let rule = rule_named(&described, "updated-rule").unwrap_or_default();
        assert_eq!(
            rule.get("Definition"),
            update.get("RuleDefinition"),
            "case {case:?}: the definition must be exactly what the update configured, got {rule}"
        );
        assert_eq!(
            rule.get("State").and_then(Value::as_str),
            Some(expected_state),
            "case {case:?}: expected state {expected_state:?}, got {rule}"
        );
        assert_eq!(
            rule.get("ApplyOnTransformedLogs").and_then(Value::as_bool),
            Some(expected_transformed),
            "case {case:?}: expected ApplyOnTransformedLogs {expected_transformed}, got {rule}"
        );
        // The rule the update did not name is untouched.
        let untouched = rule_named(&described, "other-rule").unwrap_or_default();
        assert_eq!(
            untouched.get("Definition").and_then(Value::as_str),
            Some(definition(json!({})).as_str()),
            "case {case:?}: the rule the update did not name must be untouched, got {untouched}"
        );
    }
}

#[tokio::test]
async fn describe_insight_rules_pages_through_every_rule_in_name_order() {
    let router = build_router();
    // Created out of order, so the reported order is the operation's and not the insertion
    // order.
    let created = ["delta", "alpha", "echo", "charlie", "bravo"];
    for name in created {
        ok(
            &router,
            "PutInsightRule",
            &put_request(name, &definition(json!({}))),
        )
        .await;
    }
    let mut expected: Vec<String> = created.iter().map(ToString::to_string).collect();
    expected.sort();

    // Rows differ in how many rules a page holds: fewer than the rules, exactly as many,
    // more than the rules, and the single-rule page the model's lower bound allows.
    for page_size in [1_usize, 2, 4, 5, 7] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "DescribeInsightRules", &request).await;
            let names = reported_names(&page);
            assert!(
                names.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} rules, got {page}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages"
            );
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must report every rule exactly once, in name order"
        );
    }

    // A token this service did not issue is the `InvalidNextToken` the operation models.
    // Each token is one this service could not have minted: a token that is not the opaque
    // encoding at all, and one whose decoding is not text.
    for token in ["not-a-token", "zzzz", "fffe"] {
        let (status, body) = call(
            &router,
            "DescribeInsightRules",
            &json!({"NextToken": token}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNextToken"),
            "case {token:?}: expected a 400 InvalidNextToken, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn describe_insight_rules_reports_an_empty_list_before_any_rule_is_created() {
    let router = build_router();
    let described = ok(&router, "DescribeInsightRules", &json!({})).await;
    assert_eq!(
        reported_names(&described),
        Vec::<String>::new(),
        "case \"no rules configured\": expected no rules, got {described}"
    );
    assert_eq!(
        described.get("NextToken"),
        None,
        "case \"no rules configured\": expected no continuation token, got {described}"
    );
}

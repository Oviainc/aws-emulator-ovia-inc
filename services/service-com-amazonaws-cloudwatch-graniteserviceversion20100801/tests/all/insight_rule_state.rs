// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the batch Contributor Insights rule state operations:
//! `EnableInsightRules` and `DisableInsightRules`, read back through
//! `DescribeInsightRules`.
//!
//! The rule under test: **a batch Contributor Insights rule state operation moves every rule
//! it was named and holds to the state that operation names, changes nothing else about those
//! rules and no rule it was not named, and reports each name it holds no rule for as one entry
//! of `Failures`** — "An array listing the rules that could not be disabled."
//! (`DisableInsightRulesOutput$Failures`), empty when every named rule was processed ("This
//! array is empty if the API operation was successful for all the rules specified in the
//! request.", `PartialFailure`).
//!
//! Every table row is a distinct input along what the rule generalises over (which of the two
//! operations is called, the state each rule already holds, how many names one request
//! carries, whether a name is held or unknown or repeated, the name's length and the
//! characters its pattern allows, and whether the rule was created by `PutInsightRule` or by
//! `PutManagedInsightRules`), so the recorded case is one row among many rather than the
//! specification.

use crate::harness::{
    build_router, build_router_with_engine, call, error_code, error_message, ok,
    publish_managed_rule_template,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The two batch state operations and the state each moves a rule to: "The state of the rule.
/// Valid values are ENABLED and DISABLED." (`PutInsightRuleInput$RuleState`).
const STATE_OPERATIONS: [(&str, &str); 2] = [
    ("EnableInsightRules", "ENABLED"),
    ("DisableInsightRules", "DISABLED"),
];

/// How this service words a request whose `RuleNames` array carries no entry: the state
/// operations read that array as the set of rule states to write, so an empty one asks for no
/// state at all.
const EMPTY_RULE_STATES: &str = "EMPTY_RULE_STATES: RuleStates cannot be an empty array.";

/// A rule definition in the Contributor Insights rule syntax.
fn definition() -> String {
    json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": ["API-Gateway-Access-Logs"],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    })
    .to_string()
}

/// Create the rule `name` in `rule_state`, as `PutInsightRule` stores it.
async fn given_rule(router: &MultiServiceRouter, name: &str, rule_state: &str) {
    ok(
        router,
        "PutInsightRule",
        &json!({
            "RuleName": name,
            "RuleDefinition": definition(),
            "RuleState": rule_state,
        }),
    )
    .await;
}

/// The rule `DescribeInsightRules` reports under `name`, if any.
async fn described_rule(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let body = ok(router, "DescribeInsightRules", &json!({})).await;
    body.get("InsightRules")?
        .as_array()?
        .iter()
        .find(|rule| rule.get("Name").and_then(Value::as_str) == Some(name))
        .cloned()
}

/// The `State` `DescribeInsightRules` reports for `name`, or the empty string when it reports
/// no such rule.
async fn state_of(router: &MultiServiceRouter, name: &str) -> String {
    described_rule(router, name)
        .await
        .as_ref()
        .and_then(|rule| rule.get("State"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `Failures` entries an answer carries, in the order it carried them.
fn failures(body: &Value) -> Vec<Value> {
    body.get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One member of a failure entry, as a string.
fn member(entry: &Value, name: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the state-operation table: the rules that exist before the call, each with the
/// state it was created in, and the rule names the request carries.
struct Row {
    /// What this row is, named in every assertion message.
    case: String,
    /// The rules created before the call, as (name, created state).
    held: Vec<(String, String)>,
    /// The rule names the request carries, in request order.
    named: Vec<String>,
}

/// One table row, from the borrowed names a case states.
fn row(case: &str, held: &[(&str, &str)], named: &[&str]) -> Row {
    Row {
        case: case.to_string(),
        held: held
            .iter()
            .map(|(name, rule_state)| ((*name).to_string(), (*rule_state).to_string()))
            .collect(),
        named: named.iter().map(|name| (*name).to_string()).collect(),
    }
}

#[tokio::test]
async fn every_named_rule_this_service_holds_moves_to_the_state_the_operation_names() {
    // Each row is one request: the rules that exist beforehand (name, state) and the names the
    // request carries. Rows span a single name, several names, a name already in the target
    // state, a name no rule answers to, a mixture of held and unknown names, the same name
    // twice, a one-character name, a 128-character name and a name carrying the spaces and
    // punctuation `^[\x20-\x7E]+$` allows.
    let long_name = "n".repeat(128);
    let odd_name = "My rule (v2) / high-cardinality";
    let rows: Vec<Row> = vec![
        row("one held rule", &[("one", "ENABLED")], &["one"]),
        row(
            "one held rule already in the target state",
            &[("already", "DISABLED")],
            &["already"],
        ),
        row(
            "three held rules, two of them named",
            &[
                ("first", "ENABLED"),
                ("second", "DISABLED"),
                ("third", "ENABLED"),
            ],
            &["first", "third"],
        ),
        row(
            "every held rule named",
            &[("alpha", "DISABLED"), ("bravo", "ENABLED")],
            &["alpha", "bravo"],
        ),
        row(
            "one name no rule answers to",
            &[("held", "ENABLED")],
            &["no-such-rule"],
        ),
        row(
            "a held name and two unknown ones",
            &[("held", "DISABLED")],
            &["unknown one", "held", "unknown two"],
        ),
        row(
            "the same held name twice",
            &[("twice", "ENABLED")],
            &["twice", "twice"],
        ),
        row(
            "the same unknown name twice",
            &[("held", "ENABLED")],
            &["ghost", "ghost"],
        ),
        row("a one-character name", &[("r", "ENABLED")], &["r"]),
        row(
            "a 128-character name",
            &[(long_name.as_str(), "DISABLED")],
            &[long_name.as_str()],
        ),
        row(
            "a name with the spaces and punctuation the pattern allows",
            &[(odd_name, "ENABLED")],
            &[odd_name],
        ),
        row("no rule configured at all", &[], &["nothing here"]),
    ];

    for (operation, target_state) in STATE_OPERATIONS {
        for Row {
            case: row_case,
            held,
            named,
        } in &rows
        {
            let case = format!("{operation} / {row_case}");
            let router = build_router();
            for (name, rule_state) in held {
                given_rule(&router, name, rule_state).await;
            }

            let answered = ok(&router, operation, &json!({"RuleNames": named})).await;

            // Every named rule this service holds now reports the operation's state, and
            // every rule the request did not name keeps the state it was created in.
            for (name, created_state) in held {
                let expected = if named.contains(name) {
                    target_state
                } else {
                    created_state.as_str()
                };
                let actual = state_of(&router, name).await;
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected rule {name:?} in state {expected:?}, got \
                     {actual:?}"
                );
            }

            // One failure entry per named rule this service holds none of, in the order the
            // request named them.
            let expected_failures: Vec<String> = named
                .iter()
                .filter(|name| !held.iter().any(|(held_name, _)| held_name == *name))
                .cloned()
                .collect();
            let reported = failures(&answered);
            assert_eq!(
                reported
                    .iter()
                    .map(|entry| member(entry, "FailureResource"))
                    .collect::<Vec<String>>(),
                expected_failures,
                "case {case:?}: expected a failure entry per unknown name, got {answered}"
            );
            for entry in &reported {
                let resource = member(entry, "FailureResource");
                assert!(
                    !member(entry, "ExceptionType").is_empty()
                        && !member(entry, "FailureCode").is_empty(),
                    "case {case:?}: a failure entry carries the type and code of the error, \
                     got {entry}"
                );
                let description = member(entry, "FailureDescription");
                assert!(
                    description.contains(&resource),
                    "case {case:?}: the description must name the rule {resource:?}, got \
                     {entry}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_state_change_leaves_every_other_member_of_the_rule_alone() {
    // Rows differ in what the rule was created with: whether it names a state explicitly,
    // which log format and aggregation its definition carries, whether it is evaluated on
    // transformed logs, and whether it carries tags.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the required members only",
            json!({"RuleName": "plain", "RuleDefinition": definition()}),
        ),
        (
            "a disabled rule evaluated on transformed logs",
            json!({
                "RuleName": "transformed",
                "RuleDefinition": definition(),
                "RuleState": "DISABLED",
                "ApplyOnTransformedLogs": true,
            }),
        ),
        (
            "a summing rule created with tags",
            json!({
                "RuleName": "summing",
                "RuleDefinition": json!({
                    "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
                    "LogGroupARNArray": ["arn:aws:logs:us-east-1:000000000000:log-group:/app"],
                    "LogFormat": "CLF",
                    "Contribution": {"Keys": ["IpAddress"], "ValueOf": "StatusCode"},
                    "AggregateOn": "Sum",
                })
                .to_string(),
                "Tags": [{"Key": "team", "Value": "observability"}],
            }),
        ),
    ];

    for (row, request) in rows {
        let router = build_router();
        ok(&router, "PutInsightRule", &request).await;
        let name = request["RuleName"].as_str().unwrap_or_default();
        let created = described_rule(&router, name)
            .await
            .unwrap_or_else(|| panic!("case {row:?}: expected the created rule back"));

        for (operation, target_state) in STATE_OPERATIONS {
            let case = format!("{operation} / {row}");
            let answered = ok(&router, operation, &json!({"RuleNames": [name]})).await;
            assert_eq!(
                failures(&answered),
                Vec::<Value>::new(),
                "case {case:?}: a request naming only held rules reports no failure, got \
                 {answered}"
            );

            let after = described_rule(&router, name)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: expected the rule back"));
            assert_eq!(
                after.get("State").and_then(Value::as_str),
                Some(target_state),
                "case {case:?}: expected state {target_state:?}, got {after}"
            );
            for member_name in [
                "Name",
                "Schema",
                "Definition",
                "ManagedRule",
                "ApplyOnTransformedLogs",
            ] {
                assert_eq!(
                    after.get(member_name),
                    created.get(member_name),
                    "case {case:?}: {member_name} must be left alone, got {after}"
                );
            }
            // The rule's tags are not part of its state either.
            let tags = ok(
                &router,
                "ListTagsForResource",
                &json!({
                    "ResourceARN": format!(
                        "arn:aws:cloudwatch:us-east-1:000000000000:insight-rule/{name}"
                    ),
                }),
            )
            .await;
            let expected_tags = request.get("Tags").cloned().unwrap_or_else(|| json!([]));
            assert_eq!(
                tags.get("Tags").cloned().unwrap_or_else(|| json!([])),
                expected_tags,
                "case {case:?}: the rule keeps the tags it was created with, got {tags}"
            );
        }

        // Reading the rules twice reports the same thing: a state change is the only write.
        let first = ok(&router, "DescribeInsightRules", &json!({})).await;
        let second = ok(&router, "DescribeInsightRules", &json!({})).await;
        assert_eq!(
            first, second,
            "case {row:?}: a second DescribeInsightRules must report the same rules, got \
             {second}"
        );
    }
}

#[tokio::test]
async fn a_managed_rule_is_enabled_and_disabled_by_the_same_operations() {
    // "The rules can be enabled, disabled, and deleted using EnableInsightRules,
    // DisableInsightRules, and DeleteInsightRules." (`PutManagedInsightRules`) — so a managed
    // rule follows the same rule as one a caller wrote, whatever resource it was enabled for.
    let resources = [
        ("a DynamoDB table", "arn:aws:dynamodb:us-east-1:123456789012:table/Orders"),
        ("an S3 bucket with neither Region nor account", "arn:aws:s3:::access-logs"),
    ];

    for (row, resource_arn) in resources {
        let (router, engine) = build_router_with_engine();
        // A managed rule is enabled from a template the service publishes for the resource
        // (`ManagedRule$TemplateName`), so the row's template is published first.
        publish_managed_rule_template(&engine, "ServiceLogRule-DynamoDB", resource_arn);
        ok(
            &router,
            "PutManagedInsightRules",
            &json!({
                "ManagedRules": [{
                    "TemplateName": "ServiceLogRule-DynamoDB",
                    "ResourceARN": resource_arn,
                }],
            }),
        )
        .await;
        let listed = ok(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource_arn}),
        )
        .await;
        let name = listed["ManagedRules"][0]["RuleState"]["RuleName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: expected a managed rule, got {listed}"))
            .to_string();

        for (operation, target_state) in STATE_OPERATIONS {
            let case = format!("{operation} / {row}");
            let answered = ok(&router, operation, &json!({"RuleNames": [&name]})).await;
            assert_eq!(
                failures(&answered),
                Vec::<Value>::new(),
                "case {case:?}: a managed rule is one this service holds, got {answered}"
            );
            let actual = state_of(&router, &name).await;
            assert_eq!(
                actual, target_state,
                "case {case:?}: expected state {target_state:?}, got {actual:?}"
            );
            // The managed-rule listing reports the same state as the rule itself.
            let listed = ok(
                &router,
                "ListManagedInsightRules",
                &json!({"ResourceARN": resource_arn}),
            )
            .await;
            assert_eq!(
                listed["ManagedRules"][0]["RuleState"]["State"].as_str(),
                Some(target_state),
                "case {case:?}: the managed-rule listing reports that state too, got {listed}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_naming_no_rule_is_refused_and_changes_nothing() {
    for (operation, _) in STATE_OPERATIONS {
        let router = build_router();
        given_rule(&router, "untouched", "ENABLED").await;

        let (status, body) = call(&router, operation, &json!({"RuleNames": []})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {operation:?}: expected a 400 InvalidParameterValueException, got {status} \
             {body}"
        );
        assert_eq!(
            error_message(&body),
            EMPTY_RULE_STATES,
            "case {operation:?}: expected the empty-array wording, got {body}"
        );
        let actual = state_of(&router, "untouched").await;
        assert_eq!(
            actual, "ENABLED",
            "case {operation:?}: a refused request changes no rule, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn enabling_and_disabling_round_trips_a_rule_through_both_states() {
    let router = build_router();
    given_rule(&router, "round-trip", "ENABLED").await;

    // The sequence of operations a caller drives, and the state the rule holds after each:
    // the state follows the LAST operation that named the rule, whatever it held before.
    let sequence = [
        ("DisableInsightRules", "DISABLED"),
        ("DisableInsightRules", "DISABLED"),
        ("EnableInsightRules", "ENABLED"),
        ("DisableInsightRules", "DISABLED"),
        ("EnableInsightRules", "ENABLED"),
        ("EnableInsightRules", "ENABLED"),
    ];
    for (step, (operation, expected)) in sequence.iter().enumerate() {
        let case = format!("step {step}: {operation}");
        ok(&router, operation, &json!({"RuleNames": ["round-trip"]})).await;
        let actual = state_of(&router, "round-trip").await;
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected state {expected:?}, got {actual:?}"
        );
    }
}

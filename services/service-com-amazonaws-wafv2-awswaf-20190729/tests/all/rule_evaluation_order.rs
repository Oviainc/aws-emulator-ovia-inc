// SPDX-License-Identifier: CC0-1.0

//! The order a stored rule list reads back in, end to end over the wire.
//!
//! The rule these suites assert is the one `Rule$Priority` documents: "If you define
//! more than one `Rule` in a `WebACL`, WAF evaluates each request against the
//! `Rules` in order based on the value of `Priority`. WAF processes rules with lower
//! priority first. The priorities don't need to be consecutive, but they must all be
//! different." `Priority` IS the evaluation order, so a saved rule list is held in
//! that order and every read hands it back in ascending `Priority`, whatever order
//! the save submitted the rules in — and a client that reads an entity back to
//! reason about evaluation order, or to diff it against a template, sees the order
//! the rules actually run in.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows differ in the submission order (already ascending, reversed, rotated,
//! interleaved), in the priority values (0, consecutive, non-consecutive, far
//! apart), in the number of rules, in which entity holds them (a rule group, a web
//! ACL) and in which operation saved them (create, update). The harness is the one in
//! [`crate::web_acl_lifecycle`]: every call goes through the router's front door, so
//! what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{sqli_statement, visibility, Waf};

/// A declared capacity above what any rule list below costs.
const AMPLE_CAPACITY: i64 = 500;

/// One rule, named `name` and carrying `priority`.
fn priced_rule(name: &str, priority: i64) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": sqli_statement(),
        "Action": { "Block": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// One submission: `(name, priority)` pairs in the order the request lists them.
struct OrderCase {
    case: &'static str,
    submitted: &'static [(&'static str, i64)],
}

/// The rows: every way a submission order can differ from the evaluation order,
/// over priority values that are consecutive, non-consecutive and far apart.
fn order_cases() -> &'static [OrderCase] {
    &[
        OrderCase {
            case: "one rule",
            submitted: &[("only", 0)],
        },
        OrderCase {
            case: "already in ascending order",
            submitted: &[("first", 0), ("second", 1), ("third", 2)],
        },
        OrderCase {
            case: "submitted in descending order",
            submitted: &[("last", 20), ("middle", 10), ("first", 1)],
        },
        OrderCase {
            case: "non-consecutive priorities, rotated",
            submitted: &[("r-low", 700), ("r-mid", 5), ("r-high", 42)],
        },
        OrderCase {
            case: "interleaved, with the zero priority in the middle",
            submitted: &[("b", 3), ("a", 0), ("d", 9), ("c", 4)],
        },
        OrderCase {
            case: "far apart, ascending but for the last",
            submitted: &[("x", 1), ("y", 1_000), ("z", 999_999), ("w", 2)],
        },
    ]
}

/// The rules of `submitted`, as the request lists them.
fn submitted_rules(submitted: &[(&str, i64)]) -> Value {
    Value::Array(
        submitted
            .iter()
            .map(|(name, priority)| priced_rule(name, *priority))
            .collect(),
    )
}

/// The rule names of `submitted` in ascending `Priority` — the order every read
/// must answer.
fn evaluation_order(submitted: &[(&str, i64)]) -> Vec<String> {
    let mut ordered: Vec<(&str, i64)> = submitted.to_vec();
    ordered.sort_by_key(|(_, priority)| *priority);
    ordered
        .into_iter()
        .map(|(name, _)| name.to_string())
        .collect()
}

/// The `Name` of each rule in `rules`, in the order they were answered.
fn rule_names(rules: &Value) -> Vec<String> {
    rules
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .map(|rule| rule["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Each rule's `(Name, Priority)` as answered, so a reordering that moved a priority
/// off its rule would show up.
fn name_priority_pairs(rules: &Value) -> Vec<(String, i64)> {
    rules
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .map(|rule| {
                    (
                        rule["Name"].as_str().unwrap_or_default().to_string(),
                        rule["Priority"].as_i64().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `GetRuleGroup` answers a rule group's rules in ascending `Priority`, however the
/// list was last saved — by a create or by an update.
#[tokio::test]
async fn get_rule_group_answers_the_rules_in_evaluation_order() {
    for row in order_cases() {
        let waf = Waf::new();
        let name = "ordered-group";
        let created = waf
            .ok(
                "CreateRuleGroup",
                json!({
                    "Name": name,
                    "Scope": "REGIONAL",
                    "Capacity": AMPLE_CAPACITY,
                    "VisibilityConfig": visibility(name),
                    "Rules": submitted_rules(row.submitted),
                }),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .expect("Summary.Id")
            .to_string();
        let lock_token = created["Summary"]["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string();
        let got = waf
            .ok(
                "GetRuleGroup",
                json!({ "Name": name, "Scope": "REGIONAL", "Id": &id }),
            )
            .await;
        let answered = rule_names(&got["RuleGroup"]["Rules"]);
        assert_eq!(
            answered,
            evaluation_order(row.submitted),
            "case {:?}: GetRuleGroup must answer the rules in ascending Priority, \
             got {answered:?}",
            row.case
        );
        let pairs = name_priority_pairs(&got["RuleGroup"]["Rules"]);
        let mut expected: Vec<(String, i64)> = row
            .submitted
            .iter()
            .map(|(name, priority)| ((*name).to_string(), *priority))
            .collect();
        expected.sort_by_key(|(_, priority)| *priority);
        assert_eq!(
            pairs, expected,
            "case {:?}: each rule must keep its own Priority, got {pairs:?}",
            row.case
        );

        // The same list submitted REVERSED by an update reads back the same way: the
        // order is derived from the priorities, not kept from the request.
        let mut reversed: Vec<(&str, i64)> = row.submitted.to_vec();
        reversed.reverse();
        waf.ok(
            "UpdateRuleGroup",
            json!({
                "Name": name,
                "Scope": "REGIONAL",
                "Id": &id,
                "LockToken": &lock_token,
                "VisibilityConfig": visibility(name),
                "Rules": submitted_rules(&reversed),
            }),
        )
        .await;
        let updated = waf
            .ok(
                "GetRuleGroup",
                json!({ "Name": name, "Scope": "REGIONAL", "Id": &id }),
            )
            .await;
        let answered = rule_names(&updated["RuleGroup"]["Rules"]);
        assert_eq!(
            answered,
            evaluation_order(row.submitted),
            "case {:?}: GetRuleGroup must answer the rules in ascending Priority after \
             an update, got {answered:?}",
            row.case
        );
    }
}

/// `GetWebACL` answers a web ACL's rules in ascending `Priority` too — the order is
/// a property of a rule LIST, not of the entity that holds it — however the list was
/// last saved, by a create or by an update.
#[tokio::test]
async fn get_web_acl_answers_the_rules_in_evaluation_order() {
    for row in order_cases() {
        let waf = Waf::new();
        let name = "ordered-acl";
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": name,
                    "Scope": "REGIONAL",
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(name),
                    "Rules": submitted_rules(row.submitted),
                }),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .expect("Summary.Id")
            .to_string();
        let lock_token = created["Summary"]["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string();
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": name, "Scope": "REGIONAL", "Id": &id }),
            )
            .await;
        let answered = rule_names(&got["WebACL"]["Rules"]);
        assert_eq!(
            answered,
            evaluation_order(row.submitted),
            "case {:?}: GetWebACL must answer the rules in ascending Priority after a \
             create, got {answered:?}",
            row.case
        );

        // The same list submitted REVERSED by an update reads back the same way: the
        // order is derived from the priorities, not kept from the request.
        let mut reversed: Vec<(&str, i64)> = row.submitted.to_vec();
        reversed.reverse();
        waf.ok(
            "UpdateWebACL",
            json!({
                "Name": name,
                "Scope": "REGIONAL",
                "Id": &id,
                "LockToken": &lock_token,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(name),
                "Rules": submitted_rules(&reversed),
            }),
        )
        .await;
        let updated = waf
            .ok(
                "GetWebACL",
                json!({ "Name": name, "Scope": "REGIONAL", "Id": &id }),
            )
            .await;
        let answered = rule_names(&updated["WebACL"]["Rules"]);
        assert_eq!(
            answered,
            evaluation_order(row.submitted),
            "case {:?}: GetWebACL must answer the rules in ascending Priority after an \
             update, got {answered:?}",
            row.case
        );
    }
}

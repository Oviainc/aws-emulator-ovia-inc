// SPDX-License-Identifier: CC0-1.0

//! How a save is refused when its rules cost more capacity than they are allowed,
//! end to end over the wire.
//!
//! The rule asserted here: a rule list that needs more web ACL capacity units than
//! the capacity it must fit inside is refused as an INVALID PARAMETER — the capacity
//! is a member of the save, so the response carries the settings it was declared in
//! as `Field`, the capacity the rules actually cost as `Parameter` (the figure
//! `CheckCapacity` publishes for those rules, never the under-declared value the
//! request sent) and WAF's capacity sentence as `Reason`, with the message composed
//! from those three. The resource-quota exception is a different class: it is what
//! WAF answers when an account holds as many of a resource as it may
//! (`WAFLimitsExceededException` is "you exceeded your resource limit … the maximum
//! number of WebACL objects that you can create for an Amazon Web Services
//! account"), which no member of the request can correct.
//!
//! The rows span what the rule generalises over: both operations that compare a
//! rule list against a capacity (`CreateRuleGroup` against the capacity the request
//! declares, `UpdateRuleGroup` against the group's immutable one), several rule
//! lists (one statement, several statements, a nested logical statement, a reference
//! to another entity), each at several positions below the price (one WCU below, far
//! below, and the published minimum of `CapacityUnit`), and both scopes. The price
//! is never written out as a literal: it is read from `CheckCapacity`, the report
//! `RuleGroup$Capacity`'s own doc tells a caller to size the limit against.
//!
//! Every call goes through the router's front door, the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::MAX_RULE_GROUPS_PER_SCOPE;

use crate::rule_group_lifecycle::{byte_match_statement, create_body};
use crate::web_acl_lifecycle::{
    create_ip_set, nested_statement, rule, sqli_statement, visibility, xss_statement, Waf,
};

/// The `ParameterExceptionField` value WAF names for a rule group's settings
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists
/// `RULE_GROUP`). Written out here rather than imported, so the table does not
/// restate the code under test.
const FIELD_RULE_GROUP: &str = "RULE_GROUP";

/// The `ErrorReason` WAF carries when a rule list does not fit the capacity it is
/// saved under. See [`FIELD_RULE_GROUP`].
const CAPACITY_REASON: &str = "You exceeded the capacity limit for a rule group or web ACL.";

/// The smallest capacity a save can declare: `CapacityUnit` is `range 1..`
/// (`smithy_query(query="shape",shape="CapacityUnit")`).
const MIN_CAPACITY: i64 = 1;

/// The error code of a refusal, as its `__type` carries it.
fn error_code(body: &Value) -> &str {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// A blocking rule at `priority`, so a list of several rules is well formed.
fn prioritised(name: &str, priority: i64, statement: Value) -> Value {
    let mut rule = rule(name, statement);
    rule["Priority"] = json!(priority);
    rule
}

/// The WCUs this service publishes for `rules`, as `CheckCapacity` reports them:
/// "you can check the capacity for a set of rules using CheckCapacity"
/// (`RuleGroup$Capacity`'s doc), so no per-statement figure is written out here.
async fn published_capacity(waf: &Waf, scope: &str, rules: &Value) -> i64 {
    let checked = waf
        .ok("CheckCapacity", json!({ "Scope": scope, "Rules": rules }))
        .await;
    checked["Capacity"]
        .as_i64()
        .unwrap_or_else(|| panic!("CheckCapacity must answer a Capacity, got {checked}"))
}

/// Assert that `body` is WAF's capacity refusal, reporting `required` as the
/// capacity the rules cost.
fn assert_capacity_refusal(body: &Value, required: i64, case: &str) {
    assert_eq!(
        error_code(body),
        "WAFInvalidParameterException",
        "case {case:?}: a capacity that the rules do not fit is an invalid parameter, \
         got {body}"
    );
    assert_eq!(
        (
            body["Field"].as_str(),
            body["Parameter"].as_str(),
            body["Reason"].as_str()
        ),
        (
            Some(FIELD_RULE_GROUP),
            Some(required.to_string().as_str()),
            Some(CAPACITY_REASON)
        ),
        "case {case:?}: the refusal must name the rule group's settings, the \
         capacity the rules cost ({required}) and WAF's capacity reason, got {body}"
    );
    assert_eq!(
        body["message"].as_str().unwrap_or_default(),
        format!(
            "Error reason: {CAPACITY_REASON}, field: {FIELD_RULE_GROUP}, \
             parameter: {required}"
        ),
        "case {case:?}: the message must be composed from the three members, got {body}"
    );
    assert!(
        body["SourceType"].is_null(),
        "case {case:?}: the refusal must not be the quota exception, got {body}"
    );
}

/// One rule list the table is driven over.
struct RuleList {
    /// What the list is called in an assertion.
    case: &'static str,
    /// The rules themselves.
    rules: Value,
}

/// The rule lists the table spans: one statement, several statements of different
/// kinds, a nested logical statement and a reference to another stored entity — so
/// the price a list is judged against is reached through every form of statement
/// this service prices.
async fn rule_lists(waf: &Waf, scope: &str) -> Vec<RuleList> {
    let (_id, ip_set_arn, _lock) =
        create_ip_set(waf, scope, &format!("capacity-refusal-ips-{scope}")).await;
    vec![
        RuleList {
            case: "one byte-match rule",
            rules: json!([rule("byte", byte_match_statement())]),
        },
        RuleList {
            case: "a SQLi and an XSS rule",
            rules: json!([
                prioritised("sqli", 0, sqli_statement()),
                prioritised("xss", 1, xss_statement()),
            ]),
        },
        RuleList {
            case: "three rules of three kinds",
            rules: json!([
                prioritised("byte", 0, byte_match_statement()),
                prioritised("sqli", 1, sqli_statement()),
                prioritised(
                    "ips",
                    2,
                    json!({ "IPSetReferenceStatement": { "ARN": &ip_set_arn } })
                ),
            ]),
        },
        RuleList {
            case: "a nested logical statement",
            rules: json!([rule(
                "nested",
                json!({
                    "AndStatement": {
                        "Statements": [byte_match_statement(), sqli_statement()]
                    }
                })
            )]),
        },
        RuleList {
            case: "a deeply nested statement",
            rules: json!([rule("deep", nested_statement(3, xss_statement()))]),
        },
    ]
}

/// A `CreateRuleGroup` whose rules cost more than the `Capacity` it declares is
/// refused as the invalid parameter, naming the capacity those rules cost, and
/// stores nothing — while the same rules at that capacity are saved.
#[tokio::test]
async fn create_rule_group_refuses_an_under_declared_capacity_as_an_invalid_parameter() {
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        let waf = Waf::new();
        for (index, list) in rule_lists(&waf, scope).await.into_iter().enumerate() {
            let required = published_capacity(&waf, scope, &list.rules).await;
            assert!(
                required > MIN_CAPACITY,
                "case {:?}: the row needs a price above the smallest declarable \
                 capacity to have a capacity below it, got {required}",
                list.case
            );

            // Every position below the price: one WCU below it, far below it, and
            // the smallest capacity the model lets a request declare.
            for (position, declared) in [
                ("one WCU below the price", required - 1),
                ("far below the price", required / 2),
                ("at the smallest declarable capacity", MIN_CAPACITY),
            ] {
                let case = format!("{scope}, {}, {position}", list.case);
                let name = format!("under-declared-{index}-{declared}");
                let (status, body) = waf
                    .call(
                        "CreateRuleGroup",
                        create_body(&name, scope, declared, None, &list.rules, None),
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: an under-declared capacity must be refused, got \
                     status {status} and body {body}"
                );
                assert_capacity_refusal(&body, required, &case);
                assert!(
                    body["Summary"].is_null(),
                    "case {case:?}: a refused save must answer no summary, got {body}"
                );

                // Nothing was stored: the same (Scope, Name) is still free for a
                // save that declares the capacity the refusal reported.
                let created = waf
                    .ok(
                        "CreateRuleGroup",
                        create_body(&name, scope, required, None, &list.rules, None),
                    )
                    .await;
                let id = created["Summary"]["Id"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the retry must answer a Summary.Id, got {created}")
                    })
                    .to_string();
                let got = waf
                    .ok(
                        "GetRuleGroup",
                        json!({ "Name": &name, "Scope": scope, "Id": &id }),
                    )
                    .await;
                assert_eq!(
                    got["RuleGroup"]["Capacity"].as_i64(),
                    Some(required),
                    "case {case:?}: the capacity the refusal reported must be enough \
                     to save those rules, got {got}"
                );
                // One group per (Scope, Name): the next position needs the name free
                // again.
                waf.ok(
                    "DeleteRuleGroup",
                    json!({
                        "Name": &name,
                        "Scope": scope,
                        "Id": &id,
                        "LockToken": created["Summary"]["LockToken"],
                    }),
                )
                .await;
            }
        }
    }
}

/// An `UpdateRuleGroup` whose new rules cost more than the group's immutable
/// capacity is refused the same way, naming the capacity those rules cost, and the
/// stored group is left exactly as it was.
#[tokio::test]
async fn update_rule_group_refuses_rules_above_the_immutable_capacity_the_same_way() {
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        let waf = Waf::new();
        for (index, list) in rule_lists(&waf, scope).await.into_iter().enumerate() {
            let required = published_capacity(&waf, scope, &list.rules).await;
            for (position, declared) in [
                ("one WCU below the price", required - 1),
                ("at the smallest declarable capacity", MIN_CAPACITY),
            ] {
                let case = format!("{scope}, {}, {position}", list.case);
                let name = format!("immutable-{index}-{declared}");
                // The group is created EMPTY at a capacity the new rules do not
                // fit, so only the update is refused.
                let created = waf
                    .ok(
                        "CreateRuleGroup",
                        create_body(&name, scope, declared, None, &json!([]), None),
                    )
                    .await;
                let id = created["Summary"]["Id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: Summary.Id, got {created}"))
                    .to_string();
                let lock = created["Summary"]["LockToken"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: Summary.LockToken, got {created}"))
                    .to_string();

                let (status, body) = waf
                    .call(
                        "UpdateRuleGroup",
                        json!({
                            "Name": &name,
                            "Scope": scope,
                            "Id": &id,
                            "LockToken": &lock,
                            "VisibilityConfig": visibility(&name),
                            "Rules": &list.rules,
                        }),
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: rules above the immutable capacity must be \
                     refused, got status {status} and body {body}"
                );
                assert_capacity_refusal(&body, required, &case);

                let got = waf
                    .ok(
                        "GetRuleGroup",
                        json!({ "Name": &name, "Scope": scope, "Id": &id }),
                    )
                    .await;
                assert_eq!(
                    (
                        got["RuleGroup"]["Rules"].clone(),
                        got["RuleGroup"]["Capacity"].as_i64(),
                        got["LockToken"].as_str()
                    ),
                    (json!([]), Some(declared), Some(lock.as_str())),
                    "case {case:?}: a refused update must change nothing at all, got \
                     {got}"
                );
            }
        }
    }
}

/// The two classes stay apart: a rule list that does not fit its capacity is the
/// invalid parameter, while holding as many rule groups as a scope may is the
/// resource-quota exception — which carries no `Field`, `Parameter` or `Reason` to
/// correct a member by.
#[tokio::test]
async fn a_resource_quota_is_still_the_quota_exception() {
    let waf = Waf::new();
    let scope = "REGIONAL";
    let rules = json!([rule("byte", byte_match_statement())]);
    let required = published_capacity(&waf, scope, &rules).await;

    // One group short of the bound, each declaring enough capacity for its rules,
    // so nothing here is refused for capacity.
    for index in 0..MAX_RULE_GROUPS_PER_SCOPE {
        waf.ok(
            "CreateRuleGroup",
            create_body(
                &format!("quota-rg-{index}"),
                scope,
                required,
                None,
                &rules,
                None,
            ),
        )
        .await;
    }

    let case = "one rule group past the per-scope quota";
    let body = waf
        .refused(
            "CreateRuleGroup",
            create_body("quota-rg-overflow", scope, required, None, &rules, None),
            "WAFLimitsExceededException",
            case,
        )
        .await;
    assert!(
        body["Field"].is_null() && body["Parameter"].is_null() && body["Reason"].is_null(),
        "case {case:?}: the quota exception models none of the diagnosis members, got \
         {body}"
    );

    // And in the very same scope, an under-declared capacity is still the invalid
    // parameter: the quota is about how many groups the scope holds, the capacity
    // about one save's own rules.
    let other_scope = "CLOUDFRONT";
    let body = waf
        .refused(
            "CreateRuleGroup",
            create_body(
                "quota-rg-under-declared",
                other_scope,
                MIN_CAPACITY,
                None,
                &rules,
                None,
            ),
            "WAFInvalidParameterException",
            "an under-declared capacity in a scope under its quota",
        )
        .await;
    assert_capacity_refusal(
        &body,
        published_capacity(&waf, other_scope, &rules).await,
        "an under-declared capacity in a scope under its quota",
    );
}

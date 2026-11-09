// SPDX-License-Identifier: CC0-1.0

//! What a sample calls a rule of a rule group when the web ACL did NOT leave the
//! group's own action in force: `SampledHTTPRequest$RuleNameWithinRuleGroup`, end to
//! end over the wire.
//!
//! The rule this suite pins: a sample names the rule that acted on the request by the
//! GROUP that holds it — the member "the name of the `Rule` that the request matched.
//! For managed rule groups, the format for this name is `<vendor name>#<managed rule
//! group name>#<rule name>`. For your own rule groups, the format for this name is
//! `<rule group name>#<rule name>`" — and that qualification states WHERE the rule
//! lives, so it does not depend on which action ended up being applied: a rule the web
//! ACL excluded, overrode, or counted as part of the whole group is named exactly as
//! one whose own action terminated the request. The last segment stays the group's own
//! declared rule name, which is what lets a client feed it back into
//! `DescribeManagedRuleGroup` to decide an exclusion or an override.
//!
//! The table spans the class rather than one recorded call: one row per way a web ACL
//! can put something in the place of a group rule's own action — the group's action in
//! force, the whole group counted by the web ACL rule's `OverrideAction`, the group's
//! rules excluded by `ExcludedRules`, overridden to count and overridden to challenge
//! by `RuleActionOverrides` — over two different AWS-vendor groups and a rule group
//! this account owns, whose qualification is one segment rather than two. The names
//! taken out of force are read from the group's OWN declaration rather than written
//! here, and every row re-checks the last segment against that declaration.
//!
//! `crate::sampled_rule_provenance` covers which rule a sample names when the group's
//! own action terminates the request, and `crate::counted_rule_samples` covers which
//! dimensions hold a sample at all; this suite covers the name a sample carries once
//! the web ACL has overridden the rule that produced it.
//!
//! The requests are driven through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the SAME
//! shared engine the service is mounted on — this service models no data-plane endpoint
//! of its own, because the requests a web ACL decides arrive at the protected
//! resource's endpoint — and the samples are then read over the wire, so what the table
//! observes is what an SDK observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge;

use crate::rule_group_lifecycle::create_rule_group;
use crate::sampled_rule_provenance::{declared_rule_names, request_at, samples_of, SCOPE, VENDOR};
use crate::web_acl_lifecycle::{base64_of, visibility, Waf, TEST_REGION};

/// The separator the member's documented format joins its segments with.
const SEPARATOR: char = '#';

/// A managed rule group whose rules inspect the QUERY ARGUMENTS, so a row drives it
/// with an injected fragment in the query string.
const SQLI_GROUP: &str = "AWSManagedRulesSQLiRuleSet";

/// A managed rule group whose rules inspect the PATH, so a row drives it with a path
/// alone — a group the reported instance did not name, and one whose vendor/group
/// segments differ from [`SQLI_GROUP`]'s.
const ADMIN_GROUP: &str = "AWSManagedRulesAdminProtectionRuleSet";

/// A query string carrying a SQL-injection tautology in an argument's value.
const INJECTED_QUERY: &str = "q=%27%20OR%201%3D1%20--";

/// A stub protected resource: an API Gateway REST API stage, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one. Each row binds its own, so
/// no two web ACLs decide one request.
fn stage_arn(row: usize) -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/b1c2d3e4f{row}/stages/prod")
}

/// A web ACL rule that runs the managed rule group `(VENDOR, group)` under
/// `override_action`, with `statement_members` (`ExcludedRules`,
/// `RuleActionOverrides`) beside the statement's own members.
fn managed_rule(
    name: &str,
    group: &str,
    statement_members: Value,
    override_action: Value,
) -> Value {
    let mut statement = json!({
        "ManagedRuleGroupStatement": { "VendorName": VENDOR, "Name": group }
    });
    for (member, value) in statement_members.as_object().into_iter().flatten() {
        statement["ManagedRuleGroupStatement"][member] = value.clone();
    }
    json!({
        "Name": name,
        "Priority": 0,
        "Statement": statement,
        "OverrideAction": override_action,
        "VisibilityConfig": visibility(name),
    })
}

/// A web ACL rule that runs the rule group `arn` this account owns, under
/// `override_action`.
fn own_group_rule(name: &str, arn: &str, override_action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": 0,
        "Statement": { "RuleGroupReferenceStatement": { "ARN": arn } },
        "OverrideAction": override_action,
        "VisibilityConfig": visibility(name),
    })
}

/// `ExcludedRules` naming every rule the group declares.
fn excluding(declared: &[String]) -> Value {
    Value::Array(
        declared
            .iter()
            .map(|name| json!({ "Name": name }))
            .collect(),
    )
}

/// `RuleActionOverrides` putting `action_to_use` in the place of the own action of
/// every rule the group declares.
fn overriding(declared: &[String], action_to_use: Value) -> Value {
    Value::Array(
        declared
            .iter()
            .map(|name| json!({ "Name": name, "ActionToUse": action_to_use }))
            .collect(),
    )
}

/// One way a web ACL can put something in the place of a group rule's own action.
struct Case {
    case: &'static str,
    /// The web ACL's `Rules`.
    rules: Value,
    /// The dimension read: the `MetricName` of the web ACL rule that runs the group.
    metric: &'static str,
    /// The request's path and query string.
    path: &'static str,
    query: &'static str,
    /// Whether the decision lets the request reach the origin.
    allowed: bool,
    /// The segments that must precede the group-local rule name: a managed group's
    /// vendor and name, a customer-owned group's name.
    group: Vec<String>,
    /// Every group-local name the group declares — the set the last segment has to
    /// come from.
    declares: Vec<String>,
    /// The `ActionValue` one sample of the dimension must report.
    action: &'static str,
    /// `SampledHTTPRequest$OverriddenAction` on that sample: the action the group's
    /// rule is CONFIGURED for, where the web ACL put something else in its place.
    overridden: Option<&'static str>,
}

#[tokio::test]
async fn a_sample_names_a_group_rule_by_its_group_whatever_action_the_web_acl_left_in_force() {
    let (waf, engine) = Waf::sharing_engine();

    // The rule names each vendor's group declares, read from the group's own
    // declaration: the rows take them out of force BY NAME, and every row's last
    // segment has to be one of them.
    let sqli_declared = declared_rule_names(&waf, SQLI_GROUP).await;
    let admin_declared = declared_rule_names(&waf, ADMIN_GROUP).await;
    for (group, declared) in [
        (SQLI_GROUP, &sqli_declared),
        (ADMIN_GROUP, &admin_declared),
    ] {
        assert!(
            !declared.is_empty(),
            "case the group's own declaration: DescribeManagedRuleGroup must declare \
             the rules of {group}, got none"
        );
    }

    // A rule group this account owns, whose one rule BLOCKS what it matches: the last
    // row runs it with the whole group counted, and such a group is named by its own
    // name alone.
    let own_group = "own-upload-guard";
    let own_rule = "deny-upload";
    let (_, own_group_arn, _) = create_rule_group(
        &waf,
        SCOPE,
        own_group,
        100,
        json!([{
            "Name": own_rule,
            "Priority": 0,
            "Statement": {
                "ByteMatchStatement": {
                    "SearchString": base64_of("/upload"),
                    "FieldToMatch": { "UriPath": {} },
                    "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                    "PositionalConstraint": "ENDS_WITH",
                }
            },
            "Action": { "Block": {} },
            "VisibilityConfig": visibility(own_rule),
        }]),
    )
    .await;

    let managed_group = |group: &str| vec![VENDOR.to_string(), group.to_string()];
    let cases = [
        Case {
            case: "a managed group's rule with the group's own action in force",
            rules: json!([managed_rule(
                "sqli-in-force",
                SQLI_GROUP,
                json!({}),
                json!({ "None": {} })
            )]),
            metric: "sqli-in-force",
            path: "/prod/orders",
            query: INJECTED_QUERY,
            allowed: false,
            group: managed_group(SQLI_GROUP),
            declares: sqli_declared.clone(),
            action: "BLOCK",
            overridden: None,
        },
        Case {
            case: "a managed group the web ACL rule's OverrideAction counted whole",
            rules: json!([managed_rule(
                "sqli-counted",
                SQLI_GROUP,
                json!({}),
                json!({ "Count": {} })
            )]),
            metric: "sqli-counted",
            path: "/prod/orders",
            query: INJECTED_QUERY,
            allowed: true,
            group: managed_group(SQLI_GROUP),
            declares: sqli_declared.clone(),
            action: "COUNT",
            overridden: Some("BLOCK"),
        },
        Case {
            case: "a managed group's rules the web ACL excluded by name",
            rules: json!([managed_rule(
                "admin-excluded",
                ADMIN_GROUP,
                json!({ "ExcludedRules": excluding(&admin_declared) }),
                json!({ "None": {} })
            )]),
            metric: "admin-excluded",
            path: "/prod/admin",
            query: "",
            allowed: true,
            group: managed_group(ADMIN_GROUP),
            declares: admin_declared.clone(),
            action: "EXCLUDED_AS_COUNT",
            overridden: Some("BLOCK"),
        },
        Case {
            case: "a managed group's rules the web ACL overrode to count",
            rules: json!([managed_rule(
                "admin-overridden-count",
                ADMIN_GROUP,
                json!({
                    "RuleActionOverrides":
                        overriding(&admin_declared, json!({ "Count": {} }))
                }),
                json!({ "None": {} })
            )]),
            metric: "admin-overridden-count",
            path: "/prod/admin",
            query: "",
            allowed: true,
            group: managed_group(ADMIN_GROUP),
            declares: admin_declared.clone(),
            action: "COUNT",
            overridden: Some("BLOCK"),
        },
        Case {
            case: "a managed group's rules the web ACL overrode to a DIFFERENT \
                   terminating action",
            rules: json!([managed_rule(
                "admin-overridden-challenge",
                ADMIN_GROUP,
                json!({
                    "RuleActionOverrides":
                        overriding(&admin_declared, json!({ "Challenge": {} }))
                }),
                json!({ "None": {} })
            )]),
            metric: "admin-overridden-challenge",
            path: "/prod/admin",
            query: "",
            allowed: false,
            group: managed_group(ADMIN_GROUP),
            declares: admin_declared.clone(),
            action: "CHALLENGE",
            overridden: Some("BLOCK"),
        },
        Case {
            case: "a rule of a group this account owns, counted whole, which names one \
                   group segment rather than two",
            rules: json!([own_group_rule(
                "own-counted",
                &own_group_arn,
                json!({ "Count": {} })
            )]),
            metric: "own-counted",
            path: "/prod/upload",
            query: "",
            allowed: true,
            group: vec![own_group.to_string()],
            declares: vec![own_rule.to_string()],
            action: "COUNT",
            overridden: Some("BLOCK"),
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let web_acl_name = format!("overridden-{row}");
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": web_acl_name,
                    "Scope": SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&web_acl_name),
                    "Rules": case.rules,
                }),
            )
            .await;
        let web_acl_arn = created["Summary"]["ARN"]
            .as_str()
            .expect("Summary.ARN")
            .to_string();
        let resource = stage_arn(row);
        waf.ok(
            "AssociateWebACL",
            json!({ "WebACLArn": web_acl_arn, "ResourceArn": resource }),
        )
        .await;

        let decision = edge::decide(&*engine, &resource, &request_at(case.path, case.query))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the associated web acl must decide the request",
                    case.case
                )
            });
        assert_eq!(
            decision.action.is_allow(),
            case.allowed,
            "case {:?}: expected allowed={}, got the action {:?}",
            case.case,
            case.allowed,
            decision.action
        );

        let samples = samples_of(&waf, &web_acl_arn, case.metric).await;
        assert!(
            !samples.is_empty(),
            "case {:?}: expected the {:?} dimension to hold a sample of the decided \
             request, got none",
            case.case,
            case.metric
        );

        // EVERY sample of the dimension names a rule that lives in the group the web
        // ACL rule runs, so every one of them carries the group's qualification.
        for sample in &samples {
            let named = sample["RuleNameWithinRuleGroup"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a group-qualified rule name, got none in \
                         {sample}",
                        case.case
                    )
                });
            let segments: Vec<&str> = named.split(SEPARATOR).collect();
            assert_eq!(
                segments.len(),
                case.group.len() + 1,
                "case {:?}: expected {} {SEPARATOR}-separated segment(s) — the group's \
                 own and the rule's — got {named:?}",
                case.case,
                case.group.len() + 1
            );
            for (position, expected) in case.group.iter().enumerate() {
                assert_eq!(
                    segments[position], expected,
                    "case {:?}: expected segment {position} to name the group as \
                     {expected:?}, got {named:?}",
                    case.case
                );
            }
            let local = segments.last().copied().unwrap_or_default();
            assert!(
                case.declares.iter().any(|declared| declared == local),
                "case {:?}: expected the last segment to be one of the names the group \
                 declares {:?}, got {local:?} in {named:?}",
                case.case,
                case.declares
            );
        }

        // And the sample for the action the web ACL left in force reports what the
        // group's rule is CONFIGURED for beside it.
        let acted = samples
            .iter()
            .find(|sample| sample["Action"].as_str() == Some(case.action))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the {:?} dimension to hold a sample reporting \
                     Action {:?}, got {samples:?}",
                    case.case, case.metric, case.action
                )
            });
        assert_eq!(
            acted["OverriddenAction"].as_str(),
            case.overridden,
            "case {:?}: expected OverriddenAction {:?} on the sample reporting Action \
             {:?}, got {acted}",
            case.case,
            case.overridden,
            case.action
        );
    }
}

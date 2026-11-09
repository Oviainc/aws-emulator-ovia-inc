// SPDX-License-Identifier: CC0-1.0

//! Which rule a sample NAMES: `SampledHTTPRequest$RuleNameWithinRuleGroup`, end to end
//! over the wire.
//!
//! The rule this suite pins: a sample names the terminating rule by its PROVENANCE —
//! the member "the name of the `Rule` that the request matched. For managed rule
//! groups, the format for this name is `<vendor name>#<managed rule group name>#<rule
//! name>`. For your own rule groups, the format for this name is `<rule group
//! name>#<rule name>`. If the rule is not in a rule group, this field is absent." So
//! the value is the segments that name the GROUP the rule lives in, then the rule's
//! group-local name, joined by `#`; a rule of the web ACL itself, and the
//! `DefaultAction`, name no group and so carry no value at all.
//!
//! The table spans the class rather than one recorded call: one row per kind of rule
//! that can decide a request — a rule of the web ACL, the default action, a rule inside
//! each of two different AWS-vendor managed rule groups, and a rule inside a
//! customer-owned rule group — so the qualification is read off the statement that
//! named the group rather than fixed for one group. For a managed group the last
//! segment is cross-checked against `DescribeManagedRuleGroup`'s own declaration, which
//! is what lets a client feed it back to decide an exclusion or an override.
//!
//! The requests are driven through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the SAME
//! shared engine the service is mounted on — this service models no data-plane endpoint
//! of its own, because the requests a web ACL decides arrive at the protected
//! resource's endpoint — and the sample is then read over the wire, so what the table
//! observes is what an SDK observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};

use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{base64_of, visibility, Waf, TEST_REGION};

/// The `Scope` every web ACL here lives in: an API Gateway stage is a REGIONAL
/// protected resource.
pub(crate) const SCOPE: &str = "REGIONAL";

/// The separator the member's documented format joins its segments with.
const SEPARATOR: char = '#';

/// The vendor of the managed rule groups this suite references
/// (`ManagedRuleGroupStatement$VendorName`).
pub(crate) const VENDOR: &str = "AWS";

/// A stub protected resource: an API Gateway REST API stage, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one. The stage itself belongs to
/// another service; what matters here is that the binding is keyed by this ARN, and
/// each row binds its own so no two web ACLs decide one request.
fn stage_arn(row: usize) -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/a1b2c3d4e{row}/stages/prod")
}

/// One request as the edge inspects it.
pub(crate) fn request_at(path: &str, query_string: &str) -> InspectedRequest {
    InspectedRequest {
        method: "GET".to_string(),
        uri_path: path.to_string(),
        query_string: query_string.to_string(),
        headers: vec![("user-agent".to_string(), "sample-provenance".to_string())],
        body: Vec::new(),
        client_ip: "198.51.100.5".to_string(),
    }
}

/// A web ACL rule that runs the managed rule group `(VENDOR, name)` with the group's
/// own rule actions in force (`OverrideAction` `None`).
fn managed_group_rule(rule_name: &str, group: &str) -> Value {
    json!({
        "Name": rule_name,
        "Priority": 0,
        "Statement": {
            "ManagedRuleGroupStatement": { "VendorName": VENDOR, "Name": group }
        },
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility(rule_name),
    })
}

/// The rule names `DescribeManagedRuleGroup` declares for `(VENDOR, group)` — the set
/// the last segment of a managed group's qualified rule name has to come from.
pub(crate) async fn declared_rule_names(waf: &Waf, group: &str) -> Vec<String> {
    let described = waf
        .ok(
            "DescribeManagedRuleGroup",
            json!({ "Scope": SCOPE, "VendorName": VENDOR, "Name": group }),
        )
        .await;
    described["Rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|declared| declared["Name"].as_str().map(str::to_string))
        .collect()
}

/// The samples `GetSampledRequests` holds for the `metric` dimension of `web_acl_arn`.
pub(crate) async fn samples_of(waf: &Waf, web_acl_arn: &str, metric: &str) -> Vec<Value> {
    let answered = waf
        .ok(
            "GetSampledRequests",
            json!({
                "WebAclArn": web_acl_arn,
                "RuleMetricName": metric,
                "Scope": SCOPE,
                "MaxItems": 50,
                // The whole sampling horizon, so the window never decides which samples
                // the row sees: that is `web_acl_edge_reports`'s own table.
                "TimeWindow": { "StartTime": 0, "EndTime": 99_999_999_999_i64 },
            }),
        )
        .await;
    answered["SampledRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One kind of rule that can decide a request, and the name a sample of that decision
/// carries.
struct Case {
    case: &'static str,
    /// The web ACL's `Rules`.
    rules: Value,
    /// The dimension the sample is drawn under: the `MetricName` of the web ACL rule
    /// that decided, or of the web ACL itself when the `DefaultAction` did.
    metric: String,
    /// The request's path and query string.
    path: &'static str,
    query: &'static str,
    /// Whether the decision terminates the request.
    blocked: bool,
    /// The segments that must precede the group-local rule name, or empty when the rule
    /// is in no rule group and the member must be absent.
    group: Vec<String>,
    /// The managed rule group whose own declaration the last segment must appear in,
    /// for a rule that lives in one.
    declared_by: Option<&'static str>,
}

#[tokio::test]
async fn a_sample_names_the_terminating_rule_by_the_group_that_holds_it() {
    let (waf, engine) = Waf::sharing_engine();

    // A customer-owned rule group, so one row's terminating rule lives in a group this
    // account owns rather than in a vendor's.
    let own_group = "own-upload-guard";
    let (_, own_group_arn, _) = create_rule_group(
        &waf,
        SCOPE,
        own_group,
        100,
        json!([{
            "Name": "deny-upload",
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
            "VisibilityConfig": visibility("deny-upload"),
        }]),
    )
    .await;

    let cases = [
        Case {
            case: "a rule of the web ACL itself, which is in no rule group",
            rules: json!([{
                "Name": "deny-orders",
                "Priority": 0,
                "Statement": {
                    "ByteMatchStatement": {
                        "SearchString": base64_of("/orders"),
                        "FieldToMatch": { "UriPath": {} },
                        "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                        "PositionalConstraint": "ENDS_WITH",
                    }
                },
                "Action": { "Block": {} },
                "VisibilityConfig": visibility("deny-orders"),
            }]),
            metric: "deny-orders".to_string(),
            path: "/prod/orders",
            query: "",
            blocked: true,
            group: Vec::new(),
            declared_by: None,
        },
        Case {
            case: "the web ACL's DefaultAction, which is no rule at all",
            rules: json!([{
                "Name": "deny-orders",
                "Priority": 0,
                "Statement": {
                    "ByteMatchStatement": {
                        "SearchString": base64_of("/orders"),
                        "FieldToMatch": { "UriPath": {} },
                        "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                        "PositionalConstraint": "ENDS_WITH",
                    }
                },
                "Action": { "Block": {} },
                "VisibilityConfig": visibility("deny-orders"),
            }]),
            // The web ACL's own dimension, named per row below.
            metric: String::new(),
            path: "/prod/admin",
            query: "",
            blocked: false,
            group: Vec::new(),
            declared_by: None,
        },
        Case {
            case: "a rule inside the AWS SQLi managed rule group",
            rules: json!([managed_group_rule("sqliset", "AWSManagedRulesSQLiRuleSet")]),
            metric: "sqliset".to_string(),
            path: "/prod/orders",
            query: "q=%27%20OR%201%3D1%20--",
            blocked: true,
            group: vec![VENDOR.to_string(), "AWSManagedRulesSQLiRuleSet".to_string()],
            declared_by: Some("AWSManagedRulesSQLiRuleSet"),
        },
        Case {
            case: "a rule inside a DIFFERENT AWS managed rule group, so the \
                   qualification follows the statement",
            rules: json!([managed_group_rule(
                "adminset",
                "AWSManagedRulesAdminProtectionRuleSet"
            )]),
            metric: "adminset".to_string(),
            path: "/prod/admin",
            query: "",
            blocked: true,
            group: vec![
                VENDOR.to_string(),
                "AWSManagedRulesAdminProtectionRuleSet".to_string(),
            ],
            declared_by: Some("AWSManagedRulesAdminProtectionRuleSet"),
        },
        Case {
            case: "a rule inside a rule group this account owns",
            rules: json!([{
                "Name": "own-group-ref",
                "Priority": 0,
                "Statement": { "RuleGroupReferenceStatement": { "ARN": own_group_arn } },
                "OverrideAction": { "None": {} },
                "VisibilityConfig": visibility("own-group-ref"),
            }]),
            metric: "own-group-ref".to_string(),
            path: "/prod/upload",
            query: "",
            blocked: true,
            group: vec![own_group.to_string()],
            declared_by: None,
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let web_acl_name = format!("provenance-{row}");
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
            !case.blocked,
            "case {:?}: expected blocked={}, got the action {:?}",
            case.case,
            case.blocked,
            decision.action
        );

        let metric = if case.metric.is_empty() {
            web_acl_name.clone()
        } else {
            case.metric.clone()
        };
        let samples = samples_of(&waf, &web_acl_arn, &metric).await;
        assert_eq!(
            samples.len(),
            1,
            "case {:?}: expected exactly the one evaluated request in the {metric:?} \
             dimension, got {samples:?}",
            case.case
        );
        let named = samples[0]["RuleNameWithinRuleGroup"].as_str();

        if case.group.is_empty() {
            assert_eq!(
                named, None,
                "case {:?}: a rule in no rule group names none, got {named:?} in {}",
                case.case, samples[0]
            );
            continue;
        }

        let named = named.unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a group-qualified rule name, got none in {}",
                case.case, samples[0]
            )
        });
        let segments: Vec<&str> = named.split(SEPARATOR).collect();
        assert_eq!(
            segments.len(),
            case.group.len() + 1,
            "case {:?}: expected {} {SEPARATOR}-separated segment(s) — the group's own \
             and the rule's — got {named:?}",
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
            !local.is_empty(),
            "case {:?}: the last segment is the rule's own name, got {named:?}",
            case.case
        );
        if let Some(group) = case.declared_by {
            let declared = declared_rule_names(&waf, group).await;
            assert!(
                declared.iter().any(|name| name == local),
                "case {:?}: expected the last segment to be one of the names \
                 DescribeManagedRuleGroup declares for {group}, got {local:?} among \
                 {declared:?}",
                case.case
            );
        } else {
            assert_eq!(
                local, "deny-upload",
                "case {:?}: expected the group's own rule name as the last segment, got \
                 {named:?}",
                case.case
            );
        }
    }
}

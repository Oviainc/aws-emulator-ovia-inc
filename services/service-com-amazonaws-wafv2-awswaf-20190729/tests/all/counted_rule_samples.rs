// SPDX-License-Identifier: CC0-1.0

//! What `GetSampledRequests` reports for a rule that COUNTED a request, end to end
//! over the wire.
//!
//! The rule this suite pins: a web ACL records a sample of a request under the
//! dimension of EVERY rule that matched it, whatever the action that rule applied —
//! `RuleAction$Count` "enables WAF to count requests that match the rule, without
//! otherwise affecting what happens to the request", so a counted match is recorded
//! with `Action` `COUNT` (`ActionValue`) and the evaluation carries on to the next
//! rule — while the web ACL's own dimension holds only the requests its
//! `DefaultAction` decided. A rule of a referenced rule group is recorded under the
//! dimension of the web ACL rule that runs the group and named by
//! `RuleNameWithinRuleGroup`; one the web ACL took out of force reports the value for
//! that case (`EXCLUDED_AS_COUNT` for an excluded rule) beside the action it is
//! configured for in `OverriddenAction` ("the action that the rule group rule is
//! configured for, and not the action that was applied to the request").
//!
//! The table spans the class rather than one recorded call: one row per way a rule can
//! count a request — a rule of the web ACL matching a header, a path or a query
//! string, a counted rule that runs before a rule that terminates, two counted rules
//! at once, a counted rule with sampling turned off, a rule inside a referenced rule
//! group that counts, one the web ACL excluded, one it overrode to count, and a whole
//! group it counted — and each row states what every dimension must hold, including
//! the dimensions that must hold nothing.
//!
//! The requests are driven through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the SAME
//! shared engine the service is mounted on — this service models no data-plane endpoint
//! of its own, because the requests a web ACL decides arrive at the protected
//! resource's endpoint — and the samples are then read over the wire, so what the table
//! observes is what an SDK observes.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};

use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{base64_of, visibility, Waf, TEST_REGION};

/// The `Scope` every web ACL here lives in: an API Gateway stage is a REGIONAL
/// protected resource.
const SCOPE: &str = "REGIONAL";

/// The header the single-header rows match on, and the value they match.
const MARKER_HEADER: &str = "x-sample-marker";
const MARKER_VALUE: &str = "sample-me";

/// A stub protected resource: an API Gateway REST API stage, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one. Each row binds its own, so
/// no two web ACLs decide one request.
fn stage_arn(row: usize) -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/a1b2c3d4e{row}/stages/prod")
}

/// Seconds since the Unix epoch, now.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// One request as the edge inspects it: `headers` beside the user agent every row
/// sends.
fn request_with(path: &str, query_string: &str, headers: &[(&str, &str)]) -> InspectedRequest {
    let mut sent: Vec<(String, String)> = vec![("user-agent".to_string(), "counted".to_string())];
    sent.extend(
        headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string())),
    );
    InspectedRequest {
        method: "GET".to_string(),
        uri_path: path.to_string(),
        query_string: query_string.to_string(),
        headers: sent,
        body: Vec::new(),
        client_ip: "198.51.100.5".to_string(),
    }
}

/// A `ByteMatchStatement` that matches `value` EXACTLY in the single header `name`.
fn single_header_statement(name: &str, value: &str) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": base64_of(value),
            "FieldToMatch": { "SingleHeader": { "Name": name } },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": "EXACTLY",
        }
    })
}

/// A `ByteMatchStatement` that matches `value` anywhere in `field`.
fn contains_statement(field: Value, value: &str) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": base64_of(value),
            "FieldToMatch": field,
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": "CONTAINS",
        }
    })
}

/// A web ACL rule with its own `Action`, at `priority`, sampled under its own name.
fn acl_rule(name: &str, priority: i64, statement: Value, action: Value) -> Value {
    json!({
        "Name": name,
        "Priority": priority,
        "Statement": statement,
        "Action": action,
        "VisibilityConfig": visibility(name),
    })
}

/// A web ACL rule that runs the referenced rule group `arn`, carrying `reference`'s
/// extra members (`ExcludedRules`, `RuleActionOverrides`) and `override_action`.
fn group_reference_rule(name: &str, arn: &str, reference: Value, override_action: Value) -> Value {
    let mut statement = json!({ "RuleGroupReferenceStatement": { "ARN": arn } });
    if let Some(members) = reference.as_object() {
        for (member, value) in members {
            statement["RuleGroupReferenceStatement"][member] = value.clone();
        }
    }
    json!({
        "Name": name,
        "Priority": 0,
        "Statement": statement,
        "OverrideAction": override_action,
        "VisibilityConfig": visibility(name),
    })
}

/// What one dimension must hold after the row's request was decided.
struct Dimension {
    /// The `RuleMetricName` read, or the empty string for the web ACL's own metric.
    metric: &'static str,
    /// The `ActionValue` the one sample in this dimension must report, or `None` when
    /// the dimension must hold no sample at all.
    action: Option<&'static str>,
    /// `SampledHTTPRequest$RuleNameWithinRuleGroup`, for a rule that lives in a group.
    inner_rule: Option<&'static str>,
    /// `SampledHTTPRequest$OverriddenAction`, for a group rule whose action the web ACL
    /// put something else in the place of.
    overridden_action: Option<&'static str>,
}

/// A dimension holding exactly one sample with `action` and nothing else to report.
fn holds(metric: &'static str, action: &'static str) -> Dimension {
    Dimension {
        metric,
        action: Some(action),
        inner_rule: None,
        overridden_action: None,
    }
}

/// A dimension that must hold no sample at all.
fn holds_nothing(metric: &'static str) -> Dimension {
    Dimension {
        metric,
        action: None,
        inner_rule: None,
        overridden_action: None,
    }
}

/// One way a rule can count a request.
struct Case {
    case: &'static str,
    /// The web ACL's `Rules`, built once the referenced rule group exists.
    rules: Value,
    /// The request's path, query string and extra headers.
    path: &'static str,
    query: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    /// Whether the decision lets the request reach the origin.
    allowed: bool,
    /// What every dimension must hold.
    dimensions: Vec<Dimension>,
}

/// The samples `GetSampledRequests` holds for the `metric` dimension of `web_acl_arn`,
/// beside the `PopulationSize` they were drawn from.
async fn samples_of(waf: &Waf, web_acl_arn: &str, metric: &str) -> (Vec<Value>, i64) {
    let now = now_seconds();
    let answered = waf
        .ok(
            "GetSampledRequests",
            json!({
                "WebAclArn": web_acl_arn,
                "RuleMetricName": metric,
                "Scope": SCOPE,
                "MaxItems": 500,
                "TimeWindow": { "StartTime": now - 300, "EndTime": now + 300 },
            }),
        )
        .await;
    (
        answered["SampledRequests"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        answered["PopulationSize"].as_i64().unwrap_or_default(),
    )
}

#[tokio::test]
async fn every_rule_that_matched_samples_the_request_under_its_own_dimension() {
    let (waf, engine) = Waf::sharing_engine();

    // A rule group this account owns, whose one rule BLOCKS what it matches: the rows
    // below run it as it is, excluded, overridden to count, and counted whole.
    let group_name = "counted-group";
    let group_rule = "deny-upload";
    let (_, blocking_group, _) = create_rule_group(
        &waf,
        SCOPE,
        group_name,
        100,
        json!([{
            "Name": group_rule,
            "Priority": 0,
            "Statement": contains_statement(json!({ "UriPath": {} }), "/upload"),
            "Action": { "Block": {} },
            "VisibilityConfig": visibility(group_rule),
        }]),
    )
    .await;
    // And one whose rule COUNTS what it matches, for the row where the group's own
    // action is what counts the request.
    let counting_group_name = "counting-group";
    let counting_group_rule = "watch-upload";
    let (_, counting_group, _) = create_rule_group(
        &waf,
        SCOPE,
        counting_group_name,
        100,
        json!([{
            "Name": counting_group_rule,
            "Priority": 0,
            "Statement": contains_statement(json!({ "UriPath": {} }), "/upload"),
            "Action": { "Count": {} },
            "VisibilityConfig": visibility(counting_group_rule),
        }]),
    )
    .await;
    let cases = vec![
        Case {
            case: "a Count rule matching a single header EXACTLY",
            rules: json!([acl_rule(
                "count-marker",
                0,
                single_header_statement(MARKER_HEADER, MARKER_VALUE),
                json!({ "Count": {} }),
            )]),
            path: "/prod/orders",
            query: "",
            headers: vec![(MARKER_HEADER, MARKER_VALUE)],
            allowed: true,
            dimensions: vec![holds("count-marker", "COUNT"), holds("", "ALLOW")],
        },
        Case {
            case: "the same Count rule, for a request that does not carry the header",
            rules: json!([acl_rule(
                "count-marker",
                0,
                single_header_statement(MARKER_HEADER, MARKER_VALUE),
                json!({ "Count": {} }),
            )]),
            path: "/prod/orders",
            query: "",
            headers: Vec::new(),
            allowed: true,
            dimensions: vec![holds_nothing("count-marker"), holds("", "ALLOW")],
        },
        Case {
            case: "a Count rule matching the URI path",
            rules: json!([acl_rule(
                "count-path",
                0,
                contains_statement(json!({ "UriPath": {} }), "/orders"),
                json!({ "Count": {} }),
            )]),
            path: "/prod/orders",
            query: "",
            headers: Vec::new(),
            allowed: true,
            dimensions: vec![holds("count-path", "COUNT"), holds("", "ALLOW")],
        },
        Case {
            case: "a Count rule matching the query string",
            rules: json!([acl_rule(
                "count-query",
                0,
                contains_statement(json!({ "QueryString": {} }), "debug=1"),
                json!({ "Count": {} }),
            )]),
            path: "/prod/orders",
            query: "debug=1",
            headers: Vec::new(),
            allowed: true,
            dimensions: vec![holds("count-query", "COUNT"), holds("", "ALLOW")],
        },
        Case {
            case: "two Count rules that both match",
            rules: json!([
                acl_rule(
                    "count-path",
                    0,
                    contains_statement(json!({ "UriPath": {} }), "/orders"),
                    json!({ "Count": {} }),
                ),
                acl_rule(
                    "count-marker",
                    1,
                    single_header_statement(MARKER_HEADER, MARKER_VALUE),
                    json!({ "Count": {} }),
                ),
            ]),
            path: "/prod/orders",
            query: "",
            headers: vec![(MARKER_HEADER, MARKER_VALUE)],
            allowed: true,
            dimensions: vec![
                holds("count-path", "COUNT"),
                holds("count-marker", "COUNT"),
                holds("", "ALLOW"),
            ],
        },
        Case {
            case: "a Count rule that runs before a rule that terminates the evaluation",
            rules: json!([
                acl_rule(
                    "count-path",
                    0,
                    contains_statement(json!({ "UriPath": {} }), "/orders"),
                    json!({ "Count": {} }),
                ),
                acl_rule(
                    "deny-marker",
                    1,
                    single_header_statement(MARKER_HEADER, MARKER_VALUE),
                    json!({ "Block": {} }),
                ),
            ]),
            path: "/prod/orders",
            query: "",
            headers: vec![(MARKER_HEADER, MARKER_VALUE)],
            allowed: false,
            dimensions: vec![
                holds("count-path", "COUNT"),
                holds("deny-marker", "BLOCK"),
                // The `DefaultAction` decided nothing, so the web ACL's own dimension
                // holds nothing.
                holds_nothing(""),
            ],
        },
        Case {
            case: "a Count rule whose VisibilityConfig stores no samples",
            rules: json!([{
                "Name": "count-unsampled",
                "Priority": 0,
                "Statement": contains_statement(json!({ "UriPath": {} }), "/orders"),
                "Action": { "Count": {} },
                "VisibilityConfig": {
                    "SampledRequestsEnabled": false,
                    "CloudWatchMetricsEnabled": true,
                    "MetricName": "count-unsampled",
                },
            }]),
            path: "/prod/orders",
            query: "",
            headers: Vec::new(),
            allowed: true,
            dimensions: vec![holds_nothing("count-unsampled"), holds("", "ALLOW")],
        },
        Case {
            case: "a rule inside a referenced rule group whose own action counts",
            rules: json!([group_reference_rule(
                "runs-counting-group",
                &counting_group,
                json!({}),
                json!({ "None": {} }),
            )]),
            path: "/prod/upload",
            query: "",
            headers: Vec::new(),
            allowed: true,
            dimensions: vec![
                Dimension {
                    metric: "runs-counting-group",
                    action: Some("COUNT"),
                    inner_rule: Some("counting-group#watch-upload"),
                    overridden_action: None,
                },
                holds("", "ALLOW"),
            ],
        },
        Case {
            case: "a rule of a referenced rule group the web ACL EXCLUDED",
            rules: json!([group_reference_rule(
                "runs-group",
                &blocking_group,
                json!({ "ExcludedRules": [{ "Name": group_rule }] }),
                json!({ "None": {} }),
            )]),
            path: "/prod/upload",
            query: "",
            headers: Vec::new(),
            allowed: true,
            dimensions: vec![
                Dimension {
                    metric: "runs-group",
                    action: Some("EXCLUDED_AS_COUNT"),
                    inner_rule: Some("counted-group#deny-upload"),
                    overridden_action: Some("BLOCK"),
                },
                holds("", "ALLOW"),
            ],
        },
        Case {
            case: "a rule of a referenced rule group the web ACL overrode to Count",
            rules: json!([group_reference_rule(
                "runs-group",
                &blocking_group,
                json!({
                    "RuleActionOverrides": [{
                        "Name": group_rule,
                        "ActionToUse": { "Count": {} },
                    }]
                }),
                json!({ "None": {} }),
            )]),
            path: "/prod/upload",
            query: "",
            headers: Vec::new(),
            allowed: true,
            dimensions: vec![
                Dimension {
                    metric: "runs-group",
                    action: Some("COUNT"),
                    inner_rule: Some("counted-group#deny-upload"),
                    overridden_action: Some("BLOCK"),
                },
                holds("", "ALLOW"),
            ],
        },
        Case {
            case: "a whole referenced rule group the web ACL's OverrideAction counts",
            rules: json!([group_reference_rule(
                "runs-group",
                &blocking_group,
                json!({}),
                json!({ "Count": {} }),
            )]),
            path: "/prod/upload",
            query: "",
            headers: Vec::new(),
            allowed: true,
            dimensions: vec![
                Dimension {
                    metric: "runs-group",
                    action: Some("COUNT"),
                    inner_rule: Some("counted-group#deny-upload"),
                    overridden_action: Some("BLOCK"),
                },
                holds("", "ALLOW"),
            ],
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let web_acl_name = format!("counted-{row}");
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

        let request = request_with(case.path, case.query, &case.headers);
        let decision = edge::decide(&*engine, &resource, &request).unwrap_or_else(|| {
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

        for dimension in &case.dimensions {
            let metric = if dimension.metric.is_empty() {
                web_acl_name.as_str()
            } else {
                dimension.metric
            };
            let (samples, population) = samples_of(&waf, &web_acl_arn, metric).await;
            let Some(action) = dimension.action else {
                assert!(
                    samples.is_empty() && population == 0,
                    "case {:?}: the {metric:?} dimension must hold no sample, got \
                     {samples:?} over a population of {population}",
                    case.case
                );
                continue;
            };
            assert_eq!(
                samples.len(),
                1,
                "case {:?}: expected exactly the one decided request in the {metric:?} \
                 dimension, got {samples:?}",
                case.case
            );
            assert_eq!(
                population,
                1,
                "case {:?}: the {metric:?} dimension drew its sample from the one \
                 request it holds, got a population of {population}",
                case.case
            );
            let sample = &samples[0];
            assert_eq!(
                sample["Action"].as_str(),
                Some(action),
                "case {:?}: expected the {metric:?} dimension to report Action \
                 {action:?}, got {sample}",
                case.case
            );
            assert_eq!(
                sample["RuleNameWithinRuleGroup"].as_str(),
                dimension.inner_rule,
                "case {:?}: expected the {metric:?} dimension to name the rule \
                 {:?} within its group, got {sample}",
                case.case,
                dimension.inner_rule
            );
            assert_eq!(
                sample["OverriddenAction"].as_str(),
                dimension.overridden_action,
                "case {:?}: expected the {metric:?} dimension to report \
                 OverriddenAction {:?}, got {sample}",
                case.case,
                dimension.overridden_action
            );
            // Every sample is of the request that was decided: its headers are the
            // ones the request carried, in the order it carried them.
            let reported: Vec<(String, String)> = sample["Request"]["Headers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|header| {
                    (
                        header["Name"].as_str().unwrap_or_default().to_string(),
                        header["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            assert_eq!(
                reported, request.headers,
                "case {:?}: expected the {metric:?} dimension's sample to reproduce \
                 the request's headers {:?}, got {reported:?}",
                case.case, request.headers
            );
            // `HTTPRequest$URI` is the request line's target: the path, with the
            // query string when the request sent one.
            let sent_uri = if case.query.is_empty() {
                case.path.to_string()
            } else {
                format!("{}?{}", case.path, case.query)
            };
            assert_eq!(
                sample["Request"]["URI"].as_str(),
                Some(sent_uri.as_str()),
                "case {:?}: expected the {metric:?} dimension's sample to report the \
                 request's URI {sent_uri:?}, got {sample}",
                case.case
            );
            // `SampledHTTPRequest$Weight` is "a value that indicates how one result in
            // the response relates proportionally to other results", and the instant
            // lies in the window the sample was read over.
            assert!(
                sample["Weight"].as_i64().unwrap_or_default() >= 1,
                "case {:?}: expected a weight of at least one in the {metric:?} \
                 dimension, got {sample}",
                case.case
            );
            let at = sample["Timestamp"].as_f64().unwrap_or_default();
            let now = now_seconds() as f64;
            assert!(
                at > now - 300.0 && at <= now + 1.0,
                "case {:?}: expected the {metric:?} dimension's sample to be \
                 timestamped inside the window it was read over ([{}, {now}]), got {at}",
                case.case,
                now - 300.0
            );
        }
    }
}

/// A counted rule's dimension grows with the traffic it counts, and a window that
/// holds none of it reports none: the population is what the data plane recorded, not
/// a constant.
#[tokio::test]
async fn a_counted_dimension_holds_every_request_the_rule_counted() {
    let (waf, engine) = Waf::sharing_engine();

    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": "counted-population",
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("counted-population"),
                "Rules": [acl_rule(
                    "count-marker",
                    0,
                    single_header_statement(MARKER_HEADER, MARKER_VALUE),
                    json!({ "Count": {} }),
                )],
            }),
        )
        .await;
    let web_acl_arn = created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();
    let resource = stage_arn(99);
    waf.ok(
        "AssociateWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceArn": resource }),
    )
    .await;

    // How many matching requests each row has driven through the edge by the time the
    // dimension is read: the population tracks the traffic.
    for driven in 1..=3_i64 {
        let request = request_with("/prod/orders", "", &[(MARKER_HEADER, MARKER_VALUE)]);
        edge::decide(&*engine, &resource, &request).unwrap_or_else(|| {
            panic!("case request {driven}: the associated web acl must decide the request")
        });
        let (samples, population) = samples_of(&waf, &web_acl_arn, "count-marker").await;
        assert_eq!(
            (samples.len() as i64, population),
            (driven, driven),
            "case request {driven}: expected {driven} counted sample(s) over a \
             population of {driven}, got {samples:?} over {population}"
        );
    }

    // A window the traffic falls outside of holds none of it.
    let now = now_seconds();
    let elsewhere = waf
        .ok(
            "GetSampledRequests",
            json!({
                "WebAclArn": web_acl_arn,
                "RuleMetricName": "count-marker",
                "Scope": SCOPE,
                "MaxItems": 500,
                "TimeWindow": { "StartTime": now - 7_200, "EndTime": now - 3_600 },
            }),
        )
        .await;
    assert_eq!(
        elsewhere["PopulationSize"].as_i64(),
        Some(0),
        "case a window before the counted traffic: expected an empty population, got \
         {elsewhere}"
    );

    // And `MaxItems` caps the sample without changing the population it was drawn
    // from ("if PopulationSize is less than MaxItems, the sample includes every
    // request", `GetSampledRequests$PopulationSize`).
    let capped = waf
        .ok(
            "GetSampledRequests",
            json!({
                "WebAclArn": web_acl_arn,
                "RuleMetricName": "count-marker",
                "Scope": SCOPE,
                "MaxItems": 1,
                "TimeWindow": { "StartTime": now - 300, "EndTime": now + 300 },
            }),
        )
        .await;
    assert_eq!(
        (
            capped["SampledRequests"].as_array().map(Vec::len),
            capped["PopulationSize"].as_i64()
        ),
        (Some(1), Some(3)),
        "case MaxItems below the counted population: expected one sample of a \
         population of three, got {capped}"
    );
}

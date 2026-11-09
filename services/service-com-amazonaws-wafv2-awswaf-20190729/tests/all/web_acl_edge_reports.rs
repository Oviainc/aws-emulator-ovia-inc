// SPDX-License-Identifier: CC0-1.0

//! What a web ACL's DATA plane records, as the two reports that read it answer.
//!
//! The rule: `GetSampledRequests` draws its sample from the requests the web ACL
//! actually evaluated for the `RuleMetricName` dimension inside the reported
//! `TimeWindow`, capped at `MaxItems`, with `PopulationSize` the size of the population
//! it drew from; and `GetRateBasedStatementManagedKeys` reports the aggregation
//! instances whose count over the rule's `EvaluationWindowSec` is currently over its
//! `Limit`, in CIDR notation under the family each belongs to. Both are computed from
//! what the edge recorded, so a web ACL that evaluated nothing reports nothing and one
//! that evaluated requests reports exactly those.
//!
//! The requests are driven through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the SAME
//! shared engine the service is mounted on — this service models no data-plane
//! endpoint of its own, because the requests a web ACL decides arrive at the protected
//! resource's endpoint — and both reports are then read over the wire, so what the
//! tables below observe is what an SDK observes.
//!
//! The tables span the class rather than one recorded call: one row per way a sample
//! can be selected or left out (the dimension, each end of the window, the cap), and
//! one row per address family a managed key is rendered under.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};

use crate::web_acl_lifecycle::{base64_of, visibility, Waf, TEST_REGION};

/// A stub protected resource: an API Gateway REST API stage, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one. The stage itself belongs to
/// another service; what matters here is that the binding is keyed by this ARN.
fn stage_arn() -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/a1b2c3d4e5/stages/prod")
}

/// Seconds since the Unix epoch, now.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// One request at `path` from `client_ip`, as the edge inspects it.
fn request_from(client_ip: &str, path: &str) -> InspectedRequest {
    InspectedRequest {
        method: "GET".to_string(),
        uri_path: path.to_string(),
        query_string: String::new(),
        headers: vec![("user-agent".to_string(), "edge-reports".to_string())],
        body: Vec::new(),
        client_ip: client_ip.to_string(),
    }
}

/// The web ACL both reports below are read of: one blocking byte-match rule and one
/// rate-based rule, each with its own metric dimension.
///
/// `RateBasedStatement$Limit` is modelled `range(min 10)`, so the smallest limit a rule
/// can carry is the one used here.
const RATE_LIMIT: i64 = 10;

/// Create that web ACL and put it in front of [`stage_arn`]; answers its `(id, arn)`.
async fn web_acl_in_front(waf: &Waf) -> (String, String) {
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": "edge-reports",
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("edge-reports"),
                "Rules": [
                    {
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
                    },
                    {
                        "Name": "by-ip",
                        "Priority": 1,
                        "Statement": {
                            "RateBasedStatement": {
                                "Limit": RATE_LIMIT,
                                "AggregateKeyType": "IP",
                            }
                        },
                        "Action": { "Block": {} },
                        "VisibilityConfig": visibility("by-ip"),
                    },
                ],
            }),
        )
        .await;
    let summary = &created["Summary"];
    let id = summary["Id"].as_str().expect("Summary.Id").to_string();
    let arn = summary["ARN"].as_str().expect("Summary.ARN").to_string();
    waf.ok(
        "AssociateWebACL",
        json!({ "WebACLArn": arn, "ResourceArn": stage_arn() }),
    )
    .await;
    (id, arn)
}

/// `GetSampledRequests` for `metric` over `[start, end]`, capped at `max_items`.
async fn sampled(waf: &Waf, arn: &str, metric: &str, start: i64, end: i64, max_items: i64) -> Value {
    waf.ok(
        "GetSampledRequests",
        json!({
            "WebAclArn": arn,
            "RuleMetricName": metric,
            "Scope": "REGIONAL",
            "MaxItems": max_items,
            "TimeWindow": { "StartTime": start, "EndTime": end },
        }),
    )
    .await
}

#[tokio::test]
async fn a_sample_reports_the_requests_the_web_acl_evaluated_for_that_dimension() {
    let (waf, engine) = Waf::sharing_engine();
    let (_, arn) = web_acl_in_front(&waf).await;

    // Before anything is evaluated, every window is empty: the report is computed from
    // what the edge recorded, and it has recorded nothing.
    let now = now_seconds();
    let empty = sampled(&waf, &arn, "deny-orders", now - 60, now + 60, 50).await;
    assert_eq!(
        empty["PopulationSize"].as_i64(),
        Some(0),
        "case a web acl that has evaluated nothing: expected an empty population, got {empty}"
    );

    // Three requests the blocking rule decides, and one the default action does.
    for path in ["/prod/orders", "/prod/orders", "/prod/orders", "/prod/admin"] {
        let decision = edge::decide(&*engine, &stage_arn(), &request_from("198.51.100.5", path))
            .unwrap_or_else(|| panic!("case {path}: the associated web acl must decide the request"));
        let expected_allow = path.ends_with("/admin");
        assert_eq!(
            decision.action.is_allow(),
            expected_allow,
            "case {path}: expected allow={expected_allow}, got {:?}",
            decision.action
        );
    }

    /// One way a sample can be selected or left out.
    struct Case {
        case: &'static str,
        metric: &'static str,
        start: i64,
        end: i64,
        max_items: i64,
        population: i64,
        reported: usize,
    }

    let now = now_seconds();
    let cases = [
        Case {
            case: "the dimension of the rule that blocked, over a window holding the \
                   requests",
            metric: "deny-orders",
            start: now - 60,
            end: now + 60,
            max_items: 50,
            population: 3,
            reported: 3,
        },
        Case {
            case: "the web ACL's own dimension, which the default action decided under",
            metric: "edge-reports",
            start: now - 60,
            end: now + 60,
            max_items: 50,
            population: 1,
            reported: 1,
        },
        Case {
            case: "a dimension of a rule that decided nothing",
            metric: "by-ip",
            start: now - 60,
            end: now + 60,
            max_items: 50,
            population: 0,
            reported: 0,
        },
        Case {
            case: "a dimension no rule of this web ACL carries",
            metric: "absent",
            start: now - 60,
            end: now + 60,
            max_items: 50,
            population: 0,
            reported: 0,
        },
        Case {
            case: "a window that ends before the requests arrived",
            metric: "deny-orders",
            start: now - 3_600,
            end: now - 1_800,
            max_items: 50,
            population: 0,
            reported: 0,
        },
        Case {
            case: "MaxItems below the population: the sample is capped, the population \
                   is not",
            metric: "deny-orders",
            start: now - 60,
            end: now + 60,
            max_items: 1,
            population: 3,
            reported: 1,
        },
    ];

    for case in &cases {
        let answered = sampled(
            &waf,
            &arn,
            case.metric,
            case.start,
            case.end,
            case.max_items,
        )
        .await;
        assert_eq!(
            answered["PopulationSize"].as_i64(),
            Some(case.population),
            "case {:?}: expected PopulationSize {}, got {answered}",
            case.case,
            case.population
        );
        let samples = answered["SampledRequests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            samples.len(),
            case.reported,
            "case {:?}: expected {} sampled request(s), got {answered}",
            case.case,
            case.reported
        );
        for sample in &samples {
            assert_eq!(
                sample["Request"]["ClientIP"].as_str(),
                Some("198.51.100.5"),
                "case {:?}: a sample reports the request it was drawn from, got {sample}",
                case.case
            );
            let expected_action = if case.metric == "edge-reports" {
                "ALLOW"
            } else {
                "BLOCK"
            };
            assert_eq!(
                sample["Action"].as_str(),
                Some(expected_action),
                "case {:?}: expected the action the dimension decided with, got {sample}",
                case.case
            );
        }
    }

    // The reported window never leaves the sampling horizon: a start earlier than three
    // hours ago becomes that instant, and an end after the call instant becomes it.
    let clamped = sampled(&waf, &arn, "deny-orders", now - 86_400, now + 86_400, 50).await;
    let window = &clamped["TimeWindow"];
    let start = window["StartTime"].as_f64().unwrap_or_default() as i64;
    let end = window["EndTime"].as_f64().unwrap_or_default() as i64;
    assert!(
        start >= now - 3 * 3_600 - 5 && end <= now + 5,
        "case a window wider than the sampling horizon: expected it clamped into \
         [{}, {now}], got [{start}, {end}]",
        now - 3 * 3_600
    );
}

#[tokio::test]
async fn the_managed_keys_are_the_aggregation_instances_currently_over_the_limit() {
    let (waf, engine) = Waf::sharing_engine();
    let (web_acl_id, _) = web_acl_in_front(&waf).await;

    /// The keys the report lists for the rate-based rule, by family.
    async fn managed_keys(waf: &Waf, web_acl_id: &str) -> (Vec<String>, Vec<String>) {
        let answered = waf
            .ok(
                "GetRateBasedStatementManagedKeys",
                json!({
                    "Scope": "REGIONAL",
                    "WebACLName": "edge-reports",
                    "WebACLId": web_acl_id,
                    "RuleName": "by-ip",
                }),
            )
            .await;
        let listed = |member: &str| -> Vec<String> {
            answered[member]["Addresses"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|address| address.as_str().map(str::to_string))
                .collect()
        };
        (listed("ManagedKeysIPV4"), listed("ManagedKeysIPV6"))
    }

    // Nothing has been evaluated, so no instance is over the limit.
    let (ipv4, ipv6) = managed_keys(&waf, &web_acl_id).await;
    assert!(
        ipv4.is_empty() && ipv6.is_empty(),
        "case a rule that has counted nothing: expected no managed keys, got \
         {ipv4:?} / {ipv6:?}"
    );

    /// One caller and how many requests it sends, with the CIDR its key is reported as
    /// when it goes over the limit.
    struct Caller {
        case: &'static str,
        client_ip: &'static str,
        requests: i64,
        reported: Option<&'static str>,
    }

    let callers = [
        Caller {
            case: "an IPv4 caller that stays at the limit",
            client_ip: "198.51.100.5",
            requests: RATE_LIMIT,
            reported: None,
        },
        Caller {
            case: "an IPv4 caller that goes past the limit",
            client_ip: "203.0.113.7",
            requests: RATE_LIMIT + 1,
            reported: Some("203.0.113.7/32"),
        },
        Caller {
            case: "an IPv6 caller that goes past the limit",
            client_ip: "2001:db8::1",
            requests: RATE_LIMIT + 3,
            reported: Some("2001:db8::1/128"),
        },
    ];

    for caller in &callers {
        for _ in 0..caller.requests {
            // `/admin` so the byte-match rule above does not decide the request before
            // the rate-based rule counts it.
            let decided = edge::decide(
                &*engine,
                &stage_arn(),
                &request_from(caller.client_ip, "/prod/admin"),
            );
            assert!(
                decided.is_some(),
                "case {:?}: the associated web acl must decide every request",
                caller.case
            );
        }
    }

    let (ipv4, ipv6) = managed_keys(&waf, &web_acl_id).await;
    for caller in &callers {
        let listed: &Vec<String> = if caller.client_ip.contains(':') {
            &ipv6
        } else {
            &ipv4
        };
        match caller.reported {
            Some(expected) => assert!(
                listed.contains(&expected.to_string()),
                "case {:?}: expected {expected} among the keys over the limit, got \
                 {listed:?}",
                caller.case
            ),
            None => assert!(
                !listed
                    .iter()
                    .any(|address| address.starts_with(caller.client_ip)),
                "case {:?}: a caller at the limit is not over it, got {listed:?}",
                caller.case
            ),
        }
    }
}

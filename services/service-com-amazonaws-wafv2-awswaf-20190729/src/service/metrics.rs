// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch series a web ACL publishes about the requests its rules ACTED on.
//!
//! A `VisibilityConfig` carries two switches, and they are independent: one asks for a
//! sample of the matched requests to be kept (`SampledRequestsEnabled`, what
//! `GetSampledRequests` draws from), the other asks for the dimension's counters to be
//! sent to CloudWatch — "indicates whether the associated resource sends metrics to
//! Amazon CloudWatch" (`VisibilityConfig$CloudWatchMetricsEnabled`) — under the name
//! that configuration gives the dimension, "a name of the Amazon CloudWatch metric
//! dimension" (`VisibilityConfig$MetricName`). This module is the second switch: the
//! counters WAF sends, and the series a client reads them back as.
//!
//! WHAT IS COUNTED. Every request a dimension acted on is one observation of the
//! counter named for the action it applied — [`BLOCKED_REQUESTS_METRIC`] for a request a
//! terminating `Block` refused, [`COUNTED_REQUESTS_METRIC`] for one a `Count` rule
//! matched without terminating the evaluation, and the counter of each other action a
//! rule can apply ([`ALLOWED_REQUESTS_METRIC`], [`CAPTCHA_REQUESTS_METRIC`],
//! [`CHALLENGE_REQUESTS_METRIC`]). Each observation is [`ONE_REQUEST`], so the `Sum`
//! over any span is exactly how many requests that dimension acted on in it — which is
//! the whole of what these counters say, and for a `Count` rule (whose request goes on
//! to the origin) the only evidence the rule matched at all.
//!
//! WHICH DIMENSIONS. The same dimensions the sample population is kept per: every rule
//! that acted on the request, named by its own `MetricName`, and the web ACL's own
//! dimension for a request its `DefaultAction` decided. A dimension whose
//! `CloudWatchMetricsEnabled` is false sends nothing while still deciding the request,
//! because that flag is the caller's instruction about the metrics and not about the
//! enforcement.
//!
//! HOW A SERIES IS IDENTIFIED. [`NAMESPACE`], and the three dimensions together:
//! [`WEB_ACL_DIMENSION`] naming the web ACL the traffic was decided by,
//! [`RULE_DIMENSION`] naming the acting dimension's `MetricName`, and
//! [`REGION_DIMENSION`] naming the Region the series is reported for — the Region of the
//! web ACL's own ARN for a REGIONAL web ACL, and [`GLOBAL_REGION`] for a
//! CLOUDFRONT-scoped one, which is served from every edge location rather than from a
//! Region. All three are part of one series' identity, so a reader filtering on the web
//! ACL finds the per-rule series beneath it.
//!
//! The datapoints go into the CloudWatch emulator's OWN metric store — the same store
//! `ListMetrics`, `GetMetricStatistics` and the alarm evaluation read — reached through
//! the shared engine, so one metric has one home and the publisher and its readers
//! cannot drift. They never travel over `PutMetricData`, which refuses a publish into a
//! namespace the services own ("you should not specify a namespace that begins with
//! `AWS/`", `PutMetricDataInput$Namespace`).

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// The namespace WAF reports what its web ACLs did with web requests in.
pub const NAMESPACE: &str = "AWS/WAFV2";

/// The dimension naming the web ACL whose evaluation a datapoint is about.
pub const WEB_ACL_DIMENSION: &str = "WebACL";

/// The dimension naming the acting rule dimension's `VisibilityConfig$MetricName`.
pub const RULE_DIMENSION: &str = "Rule";

/// The dimension naming the Region the series is reported for.
pub const REGION_DIMENSION: &str = "Region";

/// The value [`REGION_DIMENSION`] takes for a web ACL that belongs to no Region: a
/// CLOUDFRONT-scoped web ACL is reached through one endpoint and applied at every edge
/// location (the `Scope` member's own doc), so what its series are reported for is the
/// whole of it rather than a Region.
pub const GLOBAL_REGION: &str = "Global";

/// How many requests a dimension let through to the origin (`RuleAction$Allow`).
pub const ALLOWED_REQUESTS_METRIC: &str = "AllowedRequests";

/// How many requests a dimension refused at the edge (`RuleAction$Block`).
pub const BLOCKED_REQUESTS_METRIC: &str = "BlockedRequests";

/// How many requests a dimension MATCHED without acting on them — `RuleAction$Count`
/// "enables WAF to count requests that match the rule, without otherwise affecting what
/// happens to the request", and a rule the web ACL took out of force is counted the same
/// way (`ExcludedRule`: "the rules in the referenced rule group whose actions are set to
/// Count").
pub const COUNTED_REQUESTS_METRIC: &str = "CountedRequests";

/// How many requests a dimension answered with a CAPTCHA puzzle
/// (`RuleAction$Captcha`).
pub const CAPTCHA_REQUESTS_METRIC: &str = "CaptchaRequests";

/// How many requests a dimension answered with a silent challenge
/// (`RuleAction$Challenge`).
pub const CHALLENGE_REQUESTS_METRIC: &str = "ChallengeRequests";

/// What ONE request contributes to the counter of the action a dimension applied to it:
/// a counter counts requests, so each one it acted on is a single observation of 1 and
/// the `Sum` over a span is how many requests that span held.
const ONE_REQUEST: f64 = 1.0;

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// Milliseconds in the second a datapoint's instant is held as: the evaluation stamps
/// the request in milliseconds, CloudWatch holds a datapoint's instant in seconds.
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// The field of an ARN that names its Region, in
/// `arn:<partition>:<service>:<region>:<account>:<resource>`.
const ARN_REGION_FIELD: usize = 3;

/// ONE rule dimension that acted on one request, and the action it applied.
///
/// The dimension is named by its `VisibilityConfig$MetricName` — the rule's own for a
/// rule that acted, the web ACL's for a request its `DefaultAction` decided — and the
/// action is the `ActionValue` that dimension applied, which is what decides the counter
/// the request lands in ([`action_metric`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActedDimension {
    /// The acting dimension's `VisibilityConfig$MetricName`.
    pub metric_name: String,
    /// The `ActionValue` that dimension applied to the request.
    pub action_value: String,
}

/// The counter one observation of `action_value` belongs to, or `None` for an action
/// value no counter of this namespace counts.
pub fn action_metric(action_value: &str) -> Option<&'static str> {
    match action_value {
        "ALLOW" => Some(ALLOWED_REQUESTS_METRIC),
        "BLOCK" => Some(BLOCKED_REQUESTS_METRIC),
        // Both values the evaluation reports for a rule that matched without acting:
        // the rule's own `Count`, and a rule the web ACL excluded, which WAF counts
        // instead of applying.
        "COUNT" | "EXCLUDED_AS_COUNT" => Some(COUNTED_REQUESTS_METRIC),
        "CAPTCHA" => Some(CAPTCHA_REQUESTS_METRIC),
        "CHALLENGE" => Some(CHALLENGE_REQUESTS_METRIC),
        _ => None,
    }
}

/// The value [`REGION_DIMENSION`] takes for a web ACL of `scope` whose ARN is
/// `web_acl_arn`.
///
/// A REGIONAL web ACL protects resources of one Region, and the Region it was created in
/// is the one its own ARN carries; a CLOUDFRONT-scoped one belongs to no Region, so its
/// series are reported for [`GLOBAL_REGION`].
pub fn reported_region(scope: &str, web_acl_arn: &str) -> String {
    if scope == crate::state::SCOPE_CLOUDFRONT {
        return GLOBAL_REGION.to_string();
    }
    web_acl_arn
        .split(':')
        .nth(ARN_REGION_FIELD)
        .filter(|region| !region.is_empty())
        .unwrap_or(GLOBAL_REGION)
        .to_string()
}

/// Count every dimension that acted on ONE request into that dimension's own series.
///
/// `at_millis` is the instant the request was evaluated at, which is the instant the
/// observation carries: a counter of requests is read back over the window the traffic
/// happened in.
///
/// Nothing is published when the CloudWatch store cannot be reached through this engine:
/// a datapoint has one home, and this path never makes a second one.
pub fn publish<Eng: Send + Sync + 'static>(
    engine: &Eng,
    web_acl_name: &str,
    region: &str,
    acted: &[ActedDimension],
    at_millis: i64,
) {
    if acted.is_empty() {
        return;
    }
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let timestamp_secs = at_millis as f64 / MILLIS_PER_SECOND;
    let mut store = state.lock();
    for dimension in acted {
        let Some(metric_name) = action_metric(&dimension.action_value) else {
            continue;
        };
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: [
                (WEB_ACL_DIMENSION.to_string(), web_acl_name.to_string()),
                (RULE_DIMENSION.to_string(), dimension.metric_name.clone()),
                (REGION_DIMENSION.to_string(), region.to_string()),
            ]
            .into_iter()
            .collect::<cloudwatch::state::DimensionSet>(),
        };
        let datum = cloudwatch::state::StoredDatum {
            timestamp_secs,
            sample_count: ONE_REQUEST,
            sum: ONE_REQUEST,
            minimum: ONE_REQUEST,
            maximum: ONE_REQUEST,
            // A counter of requests measures no quantity of its own, which CloudWatch
            // spells by leaving the unit off the datum: a read of it reports the value
            // the enumerated unit type declares for a measurement that has none.
            unit: None,
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The request itself was already
            // decided, so nothing is failed for it; the dropped observation is reported
            // where a developer can see it.
            ::tracing::warn!(
                web_acl = %web_acl_name,
                rule = %dimension.metric_name,
                metric = %metric_name,
                "the CloudWatch metric store is at capacity, so what this web acl did \
                 with this request was not counted"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    //! Which counter an action lands in, and which Region a web ACL's series are
    //! reported for. The published series themselves are pinned end to end over the wire
    //! in `tests/all/web_acl_request_metrics.rs`.

    use super::*;

    #[test]
    fn every_action_value_the_evaluation_reports_has_its_own_counter() {
        // One row per `ActionValue` a dimension can report
        // (`smithy_query(query="shape",shape="ActionValue")`), plus a value that names no
        // action at all.
        let cases: [(&str, Option<&str>); 7] = [
            ("ALLOW", Some(ALLOWED_REQUESTS_METRIC)),
            ("BLOCK", Some(BLOCKED_REQUESTS_METRIC)),
            ("COUNT", Some(COUNTED_REQUESTS_METRIC)),
            ("EXCLUDED_AS_COUNT", Some(COUNTED_REQUESTS_METRIC)),
            ("CAPTCHA", Some(CAPTCHA_REQUESTS_METRIC)),
            ("CHALLENGE", Some(CHALLENGE_REQUESTS_METRIC)),
            ("", None),
        ];
        for (action, expected) in cases {
            let actual = action_metric(action);
            assert_eq!(
                actual, expected,
                "case {action:?}: expected the counter {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_regional_web_acls_series_are_reported_for_its_own_region() {
        // Rows span the scopes a web ACL can live in, and the Regions a REGIONAL one's
        // ARN can name.
        let cases: [(&str, &str, &str); 4] = [
            (
                crate::state::SCOPE_REGIONAL,
                "arn:aws:wafv2:us-west-2:000000000000:regional/webacl/guard/1111",
                "us-west-2",
            ),
            (
                crate::state::SCOPE_REGIONAL,
                "arn:aws:wafv2:eu-central-1:000000000000:regional/webacl/guard/1111",
                "eu-central-1",
            ),
            (
                crate::state::SCOPE_CLOUDFRONT,
                "arn:aws:wafv2:us-east-1:000000000000:global/webacl/guard/1111",
                GLOBAL_REGION,
            ),
            // A web ACL whose ARN names no Region is reported for none.
            (crate::state::SCOPE_REGIONAL, "arn:aws:wafv2::", GLOBAL_REGION),
        ];
        for (scope, arn, expected) in cases {
            let actual = reported_region(scope, arn);
            assert_eq!(
                actual, expected,
                "case {scope:?} {arn:?}: expected the Region dimension {expected:?}, got \
                 {actual:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `GetSampledRequests`, end to end over the wire.
//!
//! The operation "gets detailed information about a specified number of requests--a
//! sample--that WAF randomly selects from among the first 5,000 requests that your
//! Amazon Web Services resource received during a time range that you choose", and
//! two rules are asserted here:
//!
//! 1. The reported `TimeWindow` is the range WAF SAMPLED over: the requested range
//!    intersected with the sampling horizon that ends at the call instant. "You can
//!    specify any time range in the previous three hours. If you specify a start time
//!    that's earlier than three hours ago, WAF sets it to three hours ago", and a
//!    range already inside the horizon comes back unchanged ("GetSampledRequests
//!    returns a time range, which is usually the time range that you specified").
//! 2. The sample, and the `PopulationSize` it was drawn from, report the requests
//!    this service inspected for the named rule dimension inside that window. This
//!    emulator serves WAF's control plane only — no web request is routed through a
//!    web ACL — so every window reports an empty sample over a population of zero,
//!    whatever dimension, window or `MaxItems` the call names.
//!
//! Both are asserted over TABLES spanning the class — the window rows differ in where
//! each end falls relative to the horizon and to the call instant, the others in
//! scope, metric-name form and `MaxItems` (each published bound and one past it) —
//! and the addressing rows pin which web ACL a report is about. The harness is the
//! one in [`crate::web_acl_lifecycle`]: every call goes through the router's front
//! door, so what these tests observe is what an SDK observes.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{
    create_ip_set, create_web_acl, rule, sqli_statement, Waf, TEST_REGION,
};

/// How far back a sampling window reaches, in seconds: "you can specify any time
/// range in the previous three hours", from the `TimeWindow` member's own doc
/// (`smithy_query(query="operation",operation="GetSampledRequests")` →
/// `GetSampledRequests$TimeWindow`). Written out here rather than imported, so the
/// table does not restate the code under test.
const SAMPLING_HORIZON_SECS: f64 = 3.0 * 60.0 * 60.0;

/// The region a CLOUDFRONT-scoped resource's ARN carries whatever the caller signed
/// for: "API and SDKs - For all calls, use the Region endpoint us-east-1" (`Scope`'s
/// own doc).
const CLOUDFRONT_ARN_REGION: &str = "us-east-1";

/// How far the clock this test reads may differ from the clock the handler read:
/// both are the system clock, so the difference is the time the call took. Generous
/// enough that a loaded machine does not fail a row, far smaller than any offset in
/// the table.
const CLOCK_SLACK_SECS: f64 = 60.0;

/// Seconds since the Unix epoch, truncated to a whole second so every offset in the
/// table is exactly representable on the wire.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as f64
}

/// The ARN this service mints for a web ACL, as
/// `arn:aws:wafv2:<region>:<account>:<global|regional>/webacl/<name>/<id>`.
fn web_acl_arn(scope: &str, name: &str, id: &str) -> String {
    let (region, prefix) = if scope == "CLOUDFRONT" {
        (CLOUDFRONT_ARN_REGION, "global")
    } else {
        (TEST_REGION, "regional")
    };
    format!(
        "arn:aws:wafv2:{region}:{}:{prefix}/webacl/{name}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `GetSampledRequests` request for `arn` in `scope`, over the window
/// `[now + start_offset, now + end_offset]`.
fn sampled_request(
    scope: &str,
    arn: &str,
    metric: &str,
    base: f64,
    start_offset: f64,
    end_offset: f64,
    max_items: i64,
) -> Value {
    json!({
        "WebAclArn": arn,
        "RuleMetricName": metric,
        "Scope": scope,
        "TimeWindow": {
            "StartTime": base + start_offset,
            "EndTime": base + end_offset,
        },
        "MaxItems": max_items,
    })
}

/// One end of the reported window, in seconds since the epoch.
fn window_end(body: &Value, member: &str) -> f64 {
    body["TimeWindow"][member]
        .as_f64()
        .unwrap_or_else(|| panic!("TimeWindow.{member} must be an epoch-seconds number, got {body}"))
}

/// Every answer reports an empty sample over a population of zero: no web request has
/// been inspected for any web ACL in this emulator.
fn assert_empty_sample(body: &Value, case: &str) {
    assert_eq!(
        body["SampledRequests"],
        json!([]),
        "case {case:?}: expected an empty sample, got {body}"
    );
    assert_eq!(
        body["PopulationSize"].as_i64(),
        Some(0),
        "case {case:?}: expected PopulationSize 0, got {body}"
    );
}

/// What a row expects of the window the answer reports.
#[derive(Debug, Clone, Copy)]
enum Reported {
    /// The requested range already lies inside the horizon, so both ends come back
    /// exactly as requested.
    AsRequested,
    /// The start is older than the horizon, so it comes back as the horizon's start;
    /// the end comes back as requested.
    StartAtHorizon,
    /// The whole requested range is older than the horizon, so both ends come back at
    /// the horizon's start.
    CollapsedToHorizon,
    /// The end is later than the call instant, so it comes back as the call instant;
    /// the start comes back as requested.
    EndAtCallInstant,
}

/// The reported `TimeWindow` is the requested range intersected with the three-hour
/// sampling horizon ending at the call instant.
///
/// One row per way a requested range can fall relative to that horizon — wholly
/// inside, a minute inside its start, a minute / an hour / a week before it, wholly
/// before it, zero-length, and ending after the call instant — run in both scopes,
/// against a web ACL with rules and one without. A row inside the horizon must come
/// back exactly as requested; a row outside it must come back pulled to the bound it
/// crossed, and never outside the horizon.
#[tokio::test]
async fn the_reported_window_is_the_requested_range_inside_the_sampling_horizon() {
    let rows: &[(&str, f64, f64, Reported)] = &[
        (
            "wholly inside the horizon",
            -600.0,
            -60.0,
            Reported::AsRequested,
        ),
        (
            // The horizon's own start moves with the clock, so a row AT it, or within
            // the second `now_secs` truncates away, would be one the call's own
            // duration decides; the rows a minute either side of it are what pin the
            // bound.
            "a minute inside the horizon's start",
            -(SAMPLING_HORIZON_SECS - 60.0),
            -1.0,
            Reported::AsRequested,
        ),
        (
            "zero-length, inside the horizon",
            -600.0,
            -600.0,
            Reported::AsRequested,
        ),
        (
            "a minute before the horizon's start",
            -(SAMPLING_HORIZON_SECS + 60.0),
            -60.0,
            Reported::StartAtHorizon,
        ),
        (
            "an hour before the horizon's start",
            -(SAMPLING_HORIZON_SECS + 3600.0),
            -600.0,
            Reported::StartAtHorizon,
        ),
        (
            "a week before the horizon's start",
            -(7.0 * 86_400.0),
            -1.0,
            Reported::StartAtHorizon,
        ),
        (
            "wholly before the horizon",
            -(SAMPLING_HORIZON_SECS + 7200.0),
            -(SAMPLING_HORIZON_SECS + 3600.0),
            Reported::CollapsedToHorizon,
        ),
        (
            "ending ten minutes after the call",
            -600.0,
            600.0,
            Reported::EndAtCallInstant,
        ),
        (
            "ending a day after the call",
            -(SAMPLING_HORIZON_SECS / 2.0),
            86_400.0,
            Reported::EndAtCallInstant,
        ),
    ];

    let waf = Waf::new();
    let (_regional_id, regional_arn, _regional_lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "sampled-regional",
        json!([rule("sampled-rule", sqli_statement())]),
    )
    .await;
    let (_cloudfront_id, cloudfront_arn, _cloudfront_lock) =
        create_web_acl(&waf, "CLOUDFRONT", "sampled-cloudfront", json!([])).await;

    for (scope, arn) in [
        ("REGIONAL", &regional_arn),
        ("CLOUDFRONT", &cloudfront_arn),
    ] {
        for (row, start_offset, end_offset, reported) in rows {
            let case = format!("{scope} {row}");
            let base = now_secs();
            let body = waf
                .ok(
                    "GetSampledRequests",
                    sampled_request(scope, arn, "sampled-rule", base, *start_offset, *end_offset, 500),
                )
                .await;
            let start = window_end(&body, "StartTime");
            let end = window_end(&body, "EndTime");
            let requested_start = base + start_offset;
            let requested_end = base + end_offset;
            let horizon_start = base - SAMPLING_HORIZON_SECS;

            // The invariants every row shares: a range, inside the horizon, ending no
            // later than the call.
            assert!(
                start <= end,
                "case {case:?}: the reported window must be a range, got [{start}, {end}]"
            );
            assert!(
                start >= horizon_start - CLOCK_SLACK_SECS,
                "case {case:?}: the reported start must lie inside the horizon \
                 (from {horizon_start}), got {start}"
            );
            assert!(
                end <= base + CLOCK_SLACK_SECS,
                "case {case:?}: the reported end must not be after the call instant \
                 ({base}), got {end}"
            );

            match reported {
                Reported::AsRequested => {
                    assert_eq!(
                        (start, end),
                        (requested_start, requested_end),
                        "case {case:?}: a range inside the horizon must be reported as \
                         requested ([{requested_start}, {requested_end}]), got \
                         [{start}, {end}]"
                    );
                }
                Reported::StartAtHorizon => {
                    assert!(
                        (start - horizon_start).abs() <= CLOCK_SLACK_SECS,
                        "case {case:?}: a start before the horizon must be reported at \
                         the horizon's start ({horizon_start}), got {start}"
                    );
                    assert!(
                        start > requested_start,
                        "case {case:?}: the requested start ({requested_start}) must have \
                         been pulled forward, got {start}"
                    );
                    assert_eq!(
                        end, requested_end,
                        "case {case:?}: the end lies inside the horizon and must be \
                         reported as requested ({requested_end}), got {end}"
                    );
                }
                Reported::CollapsedToHorizon => {
                    assert!(
                        (start - horizon_start).abs() <= CLOCK_SLACK_SECS,
                        "case {case:?}: a range before the horizon must be reported at \
                         the horizon's start ({horizon_start}), got {start}"
                    );
                    assert_eq!(
                        end, start,
                        "case {case:?}: a range before the horizon holds no instant WAF \
                         sampled, so both ends must meet at {start}, got {end}"
                    );
                }
                Reported::EndAtCallInstant => {
                    assert_eq!(
                        start, requested_start,
                        "case {case:?}: the start lies inside the horizon and must be \
                         reported as requested ({requested_start}), got {start}"
                    );
                    assert!(
                        (end - base).abs() <= CLOCK_SLACK_SECS && end < requested_end,
                        "case {case:?}: an end after the call instant ({base}) must be \
                         reported at it, got {end}"
                    );
                }
            }

            assert_empty_sample(&body, &case);
        }
    }
}

/// The sample is empty and its population zero for every dimension and every sample
/// size: nothing in this emulator inspects a web request, so no window holds one.
///
/// One row per form `RuleMetricName` takes (`MetricName` is
/// `@pattern(^[\w#:\.\-/]+$)`, and the name is "the metric name assigned to the Rule
/// or RuleGroup dimension": a rule of the web ACL, the web ACL's own metric, a name
/// no dimension carries, and the `<vendor>#<rule group>#<rule>` form a managed group's
/// rule reports) crossed with `MaxItems` at each published bound (`ListMaxItems` is
/// `@range(min 1, max 500)`).
#[tokio::test]
async fn every_dimension_and_sample_size_reports_an_empty_sample() {
    let metrics = [
        "sampled-rule",
        "sampled-metrics",
        "ALL",
        "DefaultAction",
        "no-such-dimension",
        "AWS#AWSManagedRulesCommonRuleSet#SizeRestrictions_BODY",
    ];
    let sizes = [1_i64, 2, 499, 500];

    let waf = Waf::new();
    let (_id, arn, _lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "sampled-metrics",
        json!([rule("sampled-rule", sqli_statement())]),
    )
    .await;

    for metric in metrics {
        for size in sizes {
            let case = format!("metric {metric:?} with MaxItems {size}");
            let base = now_secs();
            let body = waf
                .ok(
                    "GetSampledRequests",
                    sampled_request("REGIONAL", &arn, metric, base, -1800.0, 0.0, size),
                )
                .await;
            assert_empty_sample(&body, &case);
            assert_eq!(
                window_end(&body, "StartTime"),
                base - 1800.0,
                "case {case:?}: the reported window must not depend on the dimension \
                 or the sample size, got {body}"
            );
        }
    }

    // One past each published bound of `MaxItems`: the request validator refuses it
    // before the operation runs, so no sample is reported at all.
    for size in [0_i64, 501] {
        let case = format!("MaxItems {size}");
        let (status, body) = waf
            .call(
                "GetSampledRequests",
                sampled_request("REGIONAL", &arn, "ALL", now_secs(), -1800.0, 0.0, size),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got status {status} and body {body}"
        );
        assert_eq!(
            body["__type"].as_str().unwrap_or_default(),
            "ValidationException",
            "case {case:?}: a member-constraint failure is the request validator's own \
             refusal, got {body}"
        );
    }
}

/// A report is about ONE stored web ACL: the `WebAclArn` must name a web ACL this
/// service holds in the requested `Scope`.
///
/// One row per way an address can name no such web ACL — an ARN of another resource
/// kind, a web ACL ARN that was never created, the stored ARN read in the other
/// scope, and the ARN of a web ACL that was deleted — which is
/// `WAFNonexistentItemException`; and one per way an ARN can be malformed, which is
/// `WAFInvalidParameterException` ("your request references an ARN that is
/// malformed", that error's own doc).
#[tokio::test]
async fn a_sample_is_refused_for_an_address_that_names_no_web_acl() {
    let waf = Waf::new();
    let (regional_id, regional_arn, regional_lock) =
        create_web_acl(&waf, "REGIONAL", "sampled-addressed", json!([])).await;
    let (_cloudfront_id, cloudfront_arn, _cloudfront_lock) =
        create_web_acl(&waf, "CLOUDFRONT", "sampled-addressed-cf", json!([])).await;
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, "REGIONAL", "sampled-addressed-ips").await;
    let absent_id = "11111111-2222-4333-8444-555555555555";

    // A web ACL that existed and was deleted names nothing afterwards: the report
    // follows stored state, not the shape of the ARN.
    let (deleted_id, deleted_arn, deleted_lock) =
        create_web_acl(&waf, "REGIONAL", "sampled-deleted", json!([])).await;
    waf.ok(
        "DeleteWebACL",
        json!({
            "Name": "sampled-deleted",
            "Scope": "REGIONAL",
            "Id": deleted_id,
            "LockToken": deleted_lock,
        }),
    )
    .await;

    let nonexistent: Vec<(&str, &str, String)> = vec![
        ("the ARN of an IP set", "REGIONAL", ip_set_arn),
        (
            "a web ACL ARN that was never created",
            "REGIONAL",
            web_acl_arn("REGIONAL", "absent", absent_id),
        ),
        (
            "the stored ARN read in the other scope",
            "CLOUDFRONT",
            regional_arn.clone(),
        ),
        (
            "the other scope's ARN read as REGIONAL",
            "REGIONAL",
            cloudfront_arn,
        ),
        ("the ARN of a deleted web ACL", "REGIONAL", deleted_arn),
    ];
    for (case, scope, arn) in nonexistent {
        let base = now_secs();
        waf.refused(
            "GetSampledRequests",
            sampled_request(scope, &arn, "ALL", base, -1800.0, 0.0, 10),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }

    let malformed: Vec<(&str, String)> = vec![
        (
            "not an ARN at all",
            "webacl/absent/1111111111111111".to_string(),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::a-bucket-named-like-this".to_string(),
        ),
        (
            "an unknown scope segment",
            format!(
                "arn:aws:wafv2:us-west-2:{}:worldwide/webacl/absent/1111",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "too few resource segments",
            format!(
                "arn:aws:wafv2:us-west-2:{}:regional/webacl/absent",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];
    for (case, arn) in malformed {
        let base = now_secs();
        let body = waf
            .refused(
                "GetSampledRequests",
                sampled_request("REGIONAL", &arn, "ALL", base, -1800.0, 0.0, 10),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_eq!(
            body["Field"].as_str().unwrap_or_default(),
            "RESOURCE_ARN",
            "case {case:?}: expected Field RESOURCE_ARN, got {body}"
        );
    }

    // The stored web ACL, read with the scope it was created in, is still reportable
    // after every refusal above: none of them changed what a later caller sees.
    let base = now_secs();
    let body = waf
        .ok(
            "GetSampledRequests",
            sampled_request("REGIONAL", &regional_arn, "ALL", base, -1800.0, 0.0, 10),
        )
        .await;
    assert_empty_sample(&body, "the stored web ACL");
    assert_eq!(
        window_end(&body, "EndTime"),
        base,
        "the stored web ACL must still be reportable, got {body}"
    );
    // The report left the web ACL exactly as it was: same lock token, same id.
    let read_back = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "sampled-addressed", "Scope": "REGIONAL", "Id": &regional_id }),
        )
        .await;
    assert_eq!(
        read_back["LockToken"].as_str().unwrap_or_default(),
        regional_lock,
        "a report must not change the web ACL's lock token, got {read_back}"
    );
}

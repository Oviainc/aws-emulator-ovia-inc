// SPDX-License-Identifier: CC0-1.0

//! What a web ACL REPORTS to CloudWatch about the requests its rules acted on, read
//! back over the wire.
//!
//! RULE: **a rule dimension whose `VisibilityConfig$CloudWatchMetricsEnabled` is true
//! counts every request it acted on into the `AWS/WAFV2` series named for that action —
//! `BlockedRequests` for a request a terminating `Block` refused, `CountedRequests` for
//! one a `Count` rule matched without terminating the evaluation, and the counter of
//! each other action a rule can apply — dimensioned by the web ACL, by that dimension's
//! own `VisibilityConfig$MetricName` and by the Region the series is reported for, all
//! three together, with one observation of 1 per request and no unit of measure, so the
//! `Sum` over the window is exactly how many requests that dimension acted on; a
//! dimension whose flag is false reports nothing while still deciding the request, and
//! the web ACL's own dimension holds the requests its `DefaultAction` decided.**
//!
//! Only a suite with CloudWatch mounted beside this service can state that: the series
//! the data plane publishes live in the CloudWatch store (there is one home for a
//! metric), so what a client sees is what `ListMetrics` and `GetMetricStatistics`
//! answer. The sibling `counted_rule_samples` suite pins the OTHER half of a
//! `VisibilityConfig` — the sample population `GetSampledRequests` draws from — which is
//! switched independently.
//!
//! The table spans what the rule generalises over rather than one recorded call: one row
//! per action a rule can apply (`Block`, `Count`, `Captcha`, `Challenge`, `Allow`), a
//! terminating and a non-terminating rule with metrics turned OFF, a different number of
//! requests per row so no row's count can stand in for another's, requests that match no
//! rule at all so the web ACL's own dimension is exercised, and — because the Region is
//! part of a series' identity — a second web ACL created in another Region.
//!
//! The requests are driven through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the SAME
//! shared engine both services are mounted on, because this service models no data-plane
//! endpoint of its own: the requests a web ACL decides arrive at the protected resource's
//! endpoint.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};
use service_com_amazonaws_wafv2_awswaf_20190729::service::metrics;
use smithy_server_runtime::MultiServiceRouter;

use crate::web_acl_lifecycle::{base64_of, TEST_REGION};

/// The `Scope` every web ACL here lives in: an API Gateway stage is a REGIONAL protected
/// resource.
const SCOPE: &str = "REGIONAL";

/// A second Region, so the `Region` dimension is shown to be part of a series' identity
/// rather than a constant.
const OTHER_REGION: &str = "eu-central-1";

/// CloudWatch's `X-Amz-Target` prefix and its signing name, which is what routes a call
/// to it.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The period the series are read at, in seconds.
const PERIOD: i64 = 60;

/// Every counter this namespace holds, from the publisher's own constants: a row states
/// which ONE of them its requests land in, and every other must stay empty for that
/// dimension.
const EVERY_COUNTER: [&str; 5] = [
    metrics::ALLOWED_REQUESTS_METRIC,
    metrics::BLOCKED_REQUESTS_METRIC,
    metrics::COUNTED_REQUESTS_METRIC,
    metrics::CAPTCHA_REQUESTS_METRIC,
    metrics::CHALLENGE_REQUESTS_METRIC,
];

/// One emulator carrying this service and CloudWatch on ONE engine, with the engine
/// itself in hand: the data plane publishes into the CloudWatch store, so both have to be
/// mounted on the same engine for a client to read back what the web ACL reported.
struct Front {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Front {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise the services, not SigV4: the `Authorization` header below
        // is present so a request ROUTES and names a region, and is never verified.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(engine.clone()),
        );
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                engine.clone(),
            ),
        );
        Self { router, engine }
    }

    /// One wafv2 call (`awsJson1_1`), credential-scoped for `region`, which must answer
    /// 200.
    async fn waf(&self, region: &str, operation: &str, body: Value) -> Value {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("https://wafv2.example.com/")
            .header("host", "wafv2.example.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AWSWAF_20190729.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLEWAFV2/20260101/{region}/wafv2/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "{operation} must succeed, got status {status} and body {parsed}"
        );
        parsed
    }

    /// One CloudWatch call (`awsJson1_0`), which must answer 200.
    async fn cloudwatch(&self, case: &str, operation: &str, body: &Value) -> Value {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "host",
                format!("{CLOUDWATCH_SIGNING_NAME}.{TEST_REGION}.amazonaws.com"),
            )
            .header("content-type", "application/x-amz-json-1.0")
            .header(
                "x-amz-target",
                format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{TEST_REGION}/\
                     {CLOUDWATCH_SIGNING_NAME}/aws4_request, SignedHeaders=host, \
                     Signature=0000"
                ),
            )
            .body(::bytes::Bytes::from(body.to_string()))
            .expect("the CloudWatch request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
        );
        parsed
    }

    /// Read one series over the whole window and fold what it holds.
    async fn held(&self, case: &str, series: &Series<'_>, at: f64) -> Held {
        let answered = self
            .cloudwatch(
                case,
                "GetMetricStatistics",
                &json!({
                    "Namespace": metrics::NAMESPACE,
                    "MetricName": series.metric,
                    "Dimensions": series.dimensions(),
                    "StartTime": at - 2.0 * PERIOD as f64,
                    "EndTime": at + 2.0 * PERIOD as f64,
                    "Period": PERIOD,
                    "Statistics": ["Sum", "SampleCount"],
                }),
            )
            .await;
        let mut folded = Held::default();
        for datapoint in answered["Datapoints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            folded.datapoints += 1;
            folded.sum += datapoint["Sum"].as_f64().unwrap_or_default();
            folded.samples += datapoint["SampleCount"].as_f64().unwrap_or_default();
            if let Some(unit) = datapoint["Unit"].as_str() {
                if !folded.units.iter().any(|held| held == unit) {
                    folded.units.push(unit.to_string());
                }
            }
        }
        folded
    }
}

/// One series of this namespace, as a reader names it.
struct Series<'a> {
    web_acl: &'a str,
    rule_metric: &'a str,
    region: &'a str,
    metric: &'a str,
}

impl Series<'_> {
    /// The dimension set the series carries, as a request names it.
    fn dimensions(&self) -> Value {
        json!([
            {"Name": metrics::WEB_ACL_DIMENSION, "Value": self.web_acl},
            {"Name": metrics::RULE_DIMENSION, "Value": self.rule_metric},
            {"Name": metrics::REGION_DIMENSION, "Value": self.region},
        ])
    }
}

/// What one series holds over the whole window.
#[derive(Debug, Default)]
struct Held {
    datapoints: usize,
    samples: f64,
    sum: f64,
    units: Vec<String>,
}

/// Wall-clock now as epoch seconds, the clock a datapoint's instant is stamped on.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// A stub protected resource: an API Gateway REST API stage in `region`, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one.
fn stage_arn(region: &str, api: &str) -> String {
    format!("arn:aws:apigateway:{region}::/restapis/{api}/stages/prod")
}

/// A `VisibilityConfig` naming `metric`, with the CloudWatch switch set to
/// `metrics_enabled` and sampling always on — so a row with metrics off is shown to
/// report nothing to CloudWatch while still keeping its sample.
fn visibility(metric: &str, metrics_enabled: bool) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": metrics_enabled,
        "MetricName": metric,
    })
}

/// A `ByteMatchStatement` matching a URI path that ENDS WITH `suffix`.
fn ends_with(suffix: &str) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": base64_of(suffix),
            "FieldToMatch": { "UriPath": {} },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": "ENDS_WITH",
        }
    })
}

/// One request as the edge inspects it, addressed to `path`.
fn request_for(path: &str) -> InspectedRequest {
    InspectedRequest {
        method: "GET".to_string(),
        uri_path: path.to_string(),
        query_string: String::new(),
        headers: vec![("user-agent".to_string(), "waf-metrics".to_string())],
        body: Vec::new(),
        client_ip: "198.51.100.5".to_string(),
    }
}

/// One rule of the web ACL, the traffic driven at it, and what its own series must hold.
struct Acting {
    case: &'static str,
    /// The path suffix the rule matches, which is also what names its metric dimension.
    suffix: &'static str,
    /// The rule's `Action`.
    action: Value,
    /// `VisibilityConfig$CloudWatchMetricsEnabled` for this rule.
    metrics_enabled: bool,
    /// How many requests this row sends at the rule — a different count per row, so no
    /// row's total can stand in for another's.
    requests: usize,
    /// The counter the rule's requests must land in, or `None` for a rule that reports
    /// nothing.
    counter: Option<&'static str>,
    /// Whether the rule's action TERMINATES the evaluation, which decides whether its
    /// requests also reach the `DefaultAction`.
    terminates: bool,
}

impl Acting {
    /// This rule's `VisibilityConfig$MetricName`, which is the `Rule` dimension of its
    /// series.
    fn metric_name(&self) -> String {
        format!("{}-rule", self.suffix.trim_start_matches('/'))
    }
}

/// One row per action a rule can apply, plus a terminating and a non-terminating rule
/// with the CloudWatch switch turned off.
fn acting_rules() -> Vec<Acting> {
    vec![
        Acting {
            case: "a terminating Block rule that reports metrics",
            suffix: "/blocked",
            action: json!({ "Block": {} }),
            metrics_enabled: true,
            requests: 7,
            counter: Some(metrics::BLOCKED_REQUESTS_METRIC),
            terminates: true,
        },
        Acting {
            case: "a Count rule, which matches without terminating",
            suffix: "/counted",
            action: json!({ "Count": {} }),
            metrics_enabled: true,
            requests: 3,
            counter: Some(metrics::COUNTED_REQUESTS_METRIC),
            terminates: false,
        },
        Acting {
            case: "a terminating Block rule with metrics turned OFF",
            suffix: "/quiet",
            action: json!({ "Block": {} }),
            metrics_enabled: false,
            requests: 5,
            counter: None,
            terminates: true,
        },
        Acting {
            case: "a Count rule with metrics turned OFF",
            suffix: "/hushed",
            action: json!({ "Count": {} }),
            metrics_enabled: false,
            requests: 8,
            counter: None,
            terminates: false,
        },
        Acting {
            case: "a terminating Captcha rule",
            suffix: "/puzzle",
            action: json!({ "Captcha": {} }),
            metrics_enabled: true,
            requests: 2,
            counter: Some(metrics::CAPTCHA_REQUESTS_METRIC),
            terminates: true,
        },
        Acting {
            case: "a terminating Challenge rule",
            suffix: "/riddle",
            action: json!({ "Challenge": {} }),
            metrics_enabled: true,
            requests: 4,
            counter: Some(metrics::CHALLENGE_REQUESTS_METRIC),
            terminates: true,
        },
        Acting {
            case: "a terminating Allow rule",
            suffix: "/welcome",
            action: json!({ "Allow": {} }),
            metrics_enabled: true,
            requests: 6,
            counter: Some(metrics::ALLOWED_REQUESTS_METRIC),
            terminates: true,
        },
    ]
}

/// How many requests match no rule at all, so the `DefaultAction` decides them.
const UNMATCHED_REQUESTS: usize = 9;

/// Create the web ACL `rules` describes, in `region`, and associate it with a stage of
/// its own. Answers `(name, arn, resource arn)`.
async fn web_acl_in_front(
    front: &Front,
    region: &str,
    name: &str,
    api: &str,
    rules: Value,
) -> (String, String, String) {
    let created = front
        .waf(
            region,
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(name, true),
                "Rules": rules,
            }),
        )
        .await;
    let arn = created["Summary"]["ARN"]
        .as_str()
        .expect("the created web ACL reports its ARN")
        .to_string();
    let resource = stage_arn(region, api);
    front
        .waf(
            region,
            "AssociateWebACL",
            json!({ "WebACLArn": arn, "ResourceArn": resource }),
        )
        .await;
    (name.to_string(), arn, resource)
}

#[tokio::test]
async fn every_dimension_that_acted_counts_its_own_requests_under_its_own_metric_name() {
    let front = Front::new();
    let rows = acting_rules();
    let rules: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(priority, row)| {
            json!({
                "Name": format!("{}-name", row.suffix.trim_start_matches('/')),
                "Priority": priority as i64,
                "Statement": ends_with(row.suffix),
                "Action": row.action.clone(),
                "VisibilityConfig": visibility(&row.metric_name(), row.metrics_enabled),
            })
        })
        .collect();
    let (web_acl, _arn, resource) = web_acl_in_front(
        &front,
        TEST_REGION,
        "metrics-guard",
        "a1b2c3d4e5",
        Value::Array(rules),
    )
    .await;

    let at = now_secs();
    for row in &rows {
        for sent in 1..=row.requests {
            let decided = edge::decide(
                &*front.engine,
                &resource,
                &request_for(&format!("/prod{}", row.suffix)),
            );
            assert!(
                decided.is_some(),
                "case {:?}: the associated web acl must decide request {sent} of {}",
                row.case,
                row.requests
            );
        }
    }
    for sent in 1..=UNMATCHED_REQUESTS {
        let decided = edge::decide(&*front.engine, &resource, &request_for("/prod/open"));
        assert!(
            decided.is_some(),
            "case a request no rule matches: the associated web acl must decide request \
             {sent} of {UNMATCHED_REQUESTS}"
        );
    }

    // Each rule's own dimension holds exactly the requests IT acted on, in exactly the
    // counter named for the action it applied, and nothing in any other counter.
    for row in &rows {
        let rule_metric = row.metric_name();
        for counter in EVERY_COUNTER {
            let expected = match row.counter {
                Some(own) if own == counter => row.requests as f64,
                _ => 0.0,
            };
            let series = Series {
                web_acl: &web_acl,
                rule_metric: &rule_metric,
                region: TEST_REGION,
                metric: counter,
            };
            let held = front.held(row.case, &series, at).await;
            assert_eq!(
                held.sum, expected,
                "case {:?}: expected the {counter} Sum under Rule={rule_metric:?} to be \
                 {expected}, got {held:?}",
                row.case
            );
            if expected == 0.0 {
                assert_eq!(
                    held.datapoints, 0,
                    "case {:?}: the {counter} series under Rule={rule_metric:?} must hold \
                     no datapoint at all, got {held:?}",
                    row.case
                );
                continue;
            }
            assert_eq!(
                held.samples, expected,
                "case {:?}: each request is one observation, so the {counter} SampleCount \
                 under Rule={rule_metric:?} must be {expected}, got {held:?}",
                row.case
            );
            assert_eq!(
                held.units,
                vec!["None".to_string()],
                "case {:?}: a counter of requests carries no unit of measure, got {held:?}",
                row.case
            );
        }
    }

    // The web ACL's own dimension holds the requests its `DefaultAction` decided: the
    // ones no rule matched, plus the ones a non-terminating rule counted and let through.
    let reached_default: usize = UNMATCHED_REQUESTS
        + rows
            .iter()
            .filter(|row| !row.terminates)
            .map(|row| row.requests)
            .sum::<usize>();
    for counter in EVERY_COUNTER {
        let expected = if counter == metrics::ALLOWED_REQUESTS_METRIC {
            reached_default as f64
        } else {
            0.0
        };
        let series = Series {
            web_acl: &web_acl,
            rule_metric: &web_acl,
            region: TEST_REGION,
            metric: counter,
        };
        let held = front.held("the web ACL's own dimension", &series, at).await;
        assert_eq!(
            held.sum, expected,
            "case the web ACL's own dimension: expected the {counter} Sum to be {expected} \
             (the requests the DefaultAction decided), got {held:?}"
        );
    }

    // A dimension of this web ACL nothing ever configured holds nothing, so a reader is
    // not answered from some other dimension's series.
    let absent = Series {
        web_acl: &web_acl,
        rule_metric: "no-such-rule",
        region: TEST_REGION,
        metric: metrics::BLOCKED_REQUESTS_METRIC,
    };
    let held = front.held("a rule dimension nothing configured", &absent, at).await;
    assert_eq!(
        held.datapoints, 0,
        "case a rule dimension nothing configured: expected no datapoint, got {held:?}"
    );
}

#[tokio::test]
async fn a_series_is_identified_by_the_web_acl_the_rule_and_the_region_together() {
    let front = Front::new();

    // Before any request there is nothing to report: these series exist because traffic
    // was decided, not because a web ACL was created.
    let before = front
        .cloudwatch(
            "before any request",
            "ListMetrics",
            &json!({ "Namespace": metrics::NAMESPACE }),
        )
        .await;
    assert_eq!(
        before["Metrics"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "a web ACL that has decided nothing reports no series, got {before}"
    );

    // Two web ACLs of the same shape, created in different Regions: the Region is part of
    // a series' identity, so each one's traffic is counted into its own series.
    let blocking = json!([{
        "Name": "deny-blocked",
        "Priority": 0,
        "Statement": ends_with("/blocked"),
        "Action": { "Block": {} },
        "VisibilityConfig": visibility("blocked-rule", true),
    }]);
    let regions: [(&str, &str, &str, usize); 2] = [
        (TEST_REGION, "home-guard", "b1b2c3d4e5", 7),
        (OTHER_REGION, "away-guard", "c1b2c3d4e5", 3),
    ];
    let at = now_secs();
    let mut fronts: Vec<(String, &str, usize)> = Vec::new();
    for (region, name, api, requests) in regions {
        let (web_acl, _arn, resource) =
            web_acl_in_front(&front, region, name, api, blocking.clone()).await;
        for sent in 1..=requests {
            let decided = edge::decide(&*front.engine, &resource, &request_for("/prod/blocked"));
            assert!(
                decided.is_some(),
                "case {region}: the associated web acl must decide request {sent} of \
                 {requests}"
            );
        }
        fronts.push((web_acl, region, requests));
    }

    for (web_acl, region, requests) in &fronts {
        let own = Series {
            web_acl,
            rule_metric: "blocked-rule",
            region,
            metric: metrics::BLOCKED_REQUESTS_METRIC,
        };
        let held = front.held(region, &own, at).await;
        assert_eq!(
            held.sum, *requests as f64,
            "case {region}: expected the Sum of this Region's series to be {requests}, got \
             {held:?}"
        );
        // The SAME web ACL and rule read for the other Region's value: the Region is part
        // of the identity, so nothing of this web ACL's traffic is there.
        let other_region = if *region == TEST_REGION {
            OTHER_REGION
        } else {
            TEST_REGION
        };
        let elsewhere = Series {
            web_acl,
            rule_metric: "blocked-rule",
            region: other_region,
            metric: metrics::BLOCKED_REQUESTS_METRIC,
        };
        let held = front.held(region, &elsewhere, at).await;
        assert_eq!(
            held.datapoints, 0,
            "case {region}: this web ACL's series must not be reported for \
             {other_region}, got {held:?}"
        );
    }

    // `ListMetrics` filtered on ONE web ACL reports that web ACL's series, each carrying
    // the web ACL, the rule and the Region together.
    let (home_acl, _, home_requests) = &fronts[0];
    let listed = front
        .cloudwatch(
            "listed for one web ACL",
            "ListMetrics",
            &json!({
                "Namespace": metrics::NAMESPACE,
                "Dimensions": [{ "Name": metrics::WEB_ACL_DIMENSION, "Value": home_acl }],
            }),
        )
        .await;
    let reported = listed["Metrics"].as_array().cloned().unwrap_or_default();
    assert!(
        !reported.is_empty(),
        "case listed for one web ACL: expected at least one series, got {listed}"
    );
    for metric in &reported {
        let names: Vec<&str> = metric["Dimensions"]
            .as_array()
            .map(|dimensions| {
                dimensions
                    .iter()
                    .filter_map(|dimension| dimension["Name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        for required in [
            metrics::WEB_ACL_DIMENSION,
            metrics::RULE_DIMENSION,
            metrics::REGION_DIMENSION,
        ] {
            assert!(
                names.contains(&required),
                "case listed for one web ACL: every series must carry the {required} \
                 dimension, got {metric}"
            );
        }
        assert_eq!(
            metric["Dimensions"]
                .as_array()
                .and_then(|dimensions| dimensions.iter().find(|dimension| {
                    dimension["Name"].as_str() == Some(metrics::WEB_ACL_DIMENSION)
                }))
                .and_then(|dimension| dimension["Value"].as_str()),
            Some(home_acl.as_str()),
            "case listed for one web ACL: the filter must report only this web ACL's \
             series, got {metric}"
        );
    }
    // The other web ACL's series are reported for it and not for this one, which is what
    // the `WebACL` dimension being part of the identity means.
    let other_listed = front
        .cloudwatch(
            "listed for the other web ACL",
            "ListMetrics",
            &json!({
                "Namespace": metrics::NAMESPACE,
                "Dimensions": [{ "Name": metrics::WEB_ACL_DIMENSION, "Value": fronts[1].0 }],
            }),
        )
        .await;
    assert_eq!(
        other_listed["Metrics"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case listed for the other web ACL: expected its one blocked-requests series, got \
         {other_listed}"
    );
    assert!(
        *home_requests != fronts[1].2,
        "the rows must drive different counts, so one web ACL's total cannot stand in for \
         the other's"
    );
}

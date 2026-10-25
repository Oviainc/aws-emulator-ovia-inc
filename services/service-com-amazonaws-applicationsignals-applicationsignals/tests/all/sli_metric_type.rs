// SPDX-License-Identifier: CC0-1.0

//! `SliMetric.MetricType` / `RequestBasedSliMetric.MetricType` is reported only for an SLO
//! whose metric Application Signals itself selected.
//!
//! The rule under test, from `smithy_query(shape="ServiceLevelIndicatorMetric",
//! member="MetricType")` (identical on `RequestBasedServiceLevelIndicatorMetric`): "If the
//! SLO monitors either the `LATENCY` or `AVAILABILITY` metric that Application Signals
//! collects, this field displays which of those metrics is used." So a returned indicator
//! reports a metric type exactly when the SLI config SELECTED a standard metric — it named
//! the Application Signals entity (`KeyAttributes`) and specified no metric of its own —
//! and leaves the optional member ABSENT otherwise, whichever operation serialises the SLO
//! (create, read or update).
//!
//! A config specifies a metric of its own when it carries CloudWatch metric or metric-math
//! queries (`MetricDataQueries`, or a request-based config's `TotalRequestCountMetric` /
//! `MonitoredRequestCountMetric`), a `MetricName` — "the name of the CloudWatch metric to
//! use for the SLO, when using a custom metric rather than Application Signals standard
//! metrics" — or a `MetricSource`, "the metric source for SLOs on resources other than
//! Application Signals services".

use serde_json::{json, Value};

use super::slo_lifecycle::{call, create, name_path_segment, router};

/// One `SliMetricConfig` (or `RequestBasedSliMetricConfig`) and the `MetricType` the stored
/// indicator must report for it.
struct Case {
    label: &'static str,
    metric_config: Value,
    expected: Option<&'static str>,
}

/// The two values the enum admits, from
/// `smithy_query(shape="ServiceLevelIndicatorMetricType")` -> `["LATENCY", "AVAILABILITY"]`.
/// Both appear below, so the entry the issue did not cite (`LATENCY`) is covered too.
const METRIC_TYPES: [&str; 2] = ["LATENCY", "AVAILABILITY"];

/// The Application Signals entity a standard-metric SLO is related to: the `Type`, `Name`
/// and `Environment` attributes the `KeyAttributes` doc requires.
///
/// `pub(crate)` so the other suites in this target relate a config to an entity through the
/// same helper rather than a second copy of it.
pub(crate) fn key_attributes(service: &str) -> Value {
    json!({ "Type": "Service", "Name": service, "Environment": "prod" })
}

/// One CloudWatch metric query, the explicit metric a config can name for itself.
fn metric_data_query(id: &str, metric_name: &str) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": { "Namespace": "AWS/ApplicationELB", "MetricName": metric_name },
            "Period": 60,
            "Stat": "Sum"
        },
        "ReturnData": true
    })
}

/// The `MetricType` of the period-based indicator in a returned `Slo`, as a string, or
/// `None` when the member is absent.
fn sli_metric_type(slo: &Value) -> Option<&str> {
    slo["Sli"]["SliMetric"]["MetricType"].as_str()
}

/// The `MetricType` of the request-based indicator in a returned `Slo`.
fn request_based_metric_type(slo: &Value) -> Option<&str> {
    slo["RequestBasedSli"]["RequestBasedSliMetric"]["MetricType"].as_str()
}

/// The period-based rows: `MetricType` survives only where the config selected a standard
/// metric, and the create, the read-back and an update all agree.
#[tokio::test]
async fn period_based_sli_reports_metric_type_only_for_a_standard_metric() {
    let cases = vec![
        Case {
            // Names the service and nothing else: Application Signals picks the metric.
            label: "service-standard-metric",
            metric_config: json!({
                "KeyAttributes": key_attributes("orders"),
                "MetricType": METRIC_TYPES[1]
            }),
            expected: Some(METRIC_TYPES[1]),
        },
        Case {
            // The same selection narrowed to one operation, with the other enum value.
            label: "operation-standard-metric",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "OperationName": "POST /checkout",
                "MetricType": METRIC_TYPES[0],
                "PeriodSeconds": 60
            }),
            expected: Some(METRIC_TYPES[0]),
        },
        Case {
            // A dependency of the named service still monitors a collected standard metric.
            label: "dependency-standard-metric",
            metric_config: json!({
                "KeyAttributes": key_attributes("orders"),
                "DependencyConfig": {
                    "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "payments" },
                    "DependencyOperationName": "Charge"
                },
                "MetricType": METRIC_TYPES[1]
            }),
            expected: Some(METRIC_TYPES[1]),
        },
        Case {
            // An empty query list names no metric, so the selection still stands.
            label: "empty-metric-data-queries",
            metric_config: json!({
                "KeyAttributes": key_attributes("orders"),
                "MetricType": METRIC_TYPES[0],
                "MetricDataQueries": []
            }),
            expected: Some(METRIC_TYPES[0]),
        },
        Case {
            // One explicit CloudWatch query: that metric, not a standard one, is monitored.
            label: "one-metric-data-query",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "OperationName": "GET /cart",
                "MetricType": METRIC_TYPES[1],
                "MetricDataQueries": [metric_data_query("m1", "RequestCount")]
            }),
            expected: None,
        },
        Case {
            // Several queries combined by a metric-math expression: same conclusion.
            label: "metric-math-expression",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "MetricType": METRIC_TYPES[0],
                "MetricDataQueries": [
                    metric_data_query("m1", "RequestCount"),
                    metric_data_query("m2", "HTTPCode_Target_5XX_Count"),
                    { "Id": "e1", "Expression": "m2 / m1 * 100", "Label": "fault rate",
                      "ReturnData": true }
                ]
            }),
            expected: None,
        },
        Case {
            // `MetricName` is the model's own marker for a custom metric.
            label: "custom-metric-name",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "MetricType": METRIC_TYPES[0],
                "MetricName": "Latency",
                "Statistic": "p99",
                "PeriodSeconds": 300
            }),
            expected: None,
        },
        Case {
            // A metric source outside Application Signals collects no standard metric.
            label: "metric-source-outside-application-signals",
            metric_config: json!({
                "KeyAttributes": key_attributes("web"),
                "MetricType": METRIC_TYPES[1],
                "MetricSource": {
                    "MetricSourceKeyAttributes": { "Type": "AWS::RUM::AppMonitor", "Name": "web" }
                }
            }),
            expected: None,
        },
        Case {
            // No entity named, so there is no service whose standard metrics could be used.
            label: "metric-type-without-key-attributes",
            metric_config: json!({ "MetricType": METRIC_TYPES[1] }),
            expected: None,
        },
        Case {
            // Nothing requested, nothing reported.
            label: "no-metric-type-requested",
            metric_config: json!({ "KeyAttributes": key_attributes("orders") }),
            expected: None,
        },
    ];

    let router = router();
    for case in &cases {
        let name = format!("period-{}", case.label);
        let (status, created) = create(
            &router,
            json!({ "Name": name, "SliConfig": { "SliMetricConfig": case.metric_config.clone() } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateServiceLevelObjective, got {status} with {created}",
            case.label
        );
        assert_eq!(
            sli_metric_type(&created["Slo"]),
            case.expected,
            "case {:?}: expected Slo.Sli.SliMetric.MetricType {:?} on create, got {:?}",
            case.label,
            case.expected,
            created["Slo"]["Sli"]["SliMetric"]["MetricType"]
        );

        // The record that was PERSISTED carries the same answer, so the member is not just
        // suppressed on the create response.
        let (status, fetched) = call(
            &router,
            "GET",
            &format!("/slo/{}", name_path_segment(&name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetServiceLevelObjective, got {status} with {fetched}",
            case.label
        );
        assert_eq!(
            sli_metric_type(&fetched["Slo"]),
            case.expected,
            "case {:?}: expected Slo.Sli.SliMetric.MetricType {:?} on read-back, got {:?}",
            case.label,
            case.expected,
            fetched["Slo"]["Sli"]["SliMetric"]["MetricType"]
        );

        // And an update that re-supplies the same config answers the same, as does the read
        // after it.
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/slo/{}", name_path_segment(&name)),
            Some(json!({ "SliConfig": { "SliMetricConfig": case.metric_config.clone() } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with {updated}",
            case.label
        );
        assert_eq!(
            sli_metric_type(&updated["Slo"]),
            case.expected,
            "case {:?}: expected Slo.Sli.SliMetric.MetricType {:?} on update, got {:?}",
            case.label,
            case.expected,
            updated["Slo"]["Sli"]["SliMetric"]["MetricType"]
        );
    }
}

/// The request-based rows. The member and its doc are the same, and so is the rule; what
/// differs is that this config names its own metric through the request-count queries.
#[tokio::test]
async fn request_based_sli_reports_metric_type_only_for_a_standard_metric() {
    let cases = vec![
        Case {
            label: "service-standard-metric",
            metric_config: json!({
                "KeyAttributes": key_attributes("orders"),
                "MetricType": METRIC_TYPES[1]
            }),
            expected: Some(METRIC_TYPES[1]),
        },
        Case {
            label: "operation-standard-metric",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "OperationName": "POST /checkout",
                "MetricType": METRIC_TYPES[0]
            }),
            expected: Some(METRIC_TYPES[0]),
        },
        Case {
            label: "dependency-standard-metric",
            metric_config: json!({
                "KeyAttributes": key_attributes("orders"),
                "DependencyConfig": {
                    "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "payments" },
                    "DependencyOperationName": "Charge"
                },
                "MetricType": METRIC_TYPES[1]
            }),
            expected: Some(METRIC_TYPES[1]),
        },
        Case {
            // Both request-count metrics spelled out: the SLO monitors those queries.
            label: "total-and-good-count-queries",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "OperationName": "GET /cart",
                "MetricType": METRIC_TYPES[1],
                "TotalRequestCountMetric": [metric_data_query("t1", "RequestCount")],
                "MonitoredRequestCountMetric": {
                    "GoodCountMetric": [metric_data_query("g1", "HTTPCode_Target_2XX_Count")]
                }
            }),
            expected: None,
        },
        Case {
            // The bad-count union member is the same kind of explicit metric.
            label: "bad-count-query-only",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "MetricType": METRIC_TYPES[0],
                "MonitoredRequestCountMetric": {
                    "BadCountMetric": [metric_data_query("b1", "HTTPCode_Target_5XX_Count")]
                }
            }),
            expected: None,
        },
        Case {
            label: "custom-metric-name",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "MetricType": METRIC_TYPES[0],
                "MetricName": "Fault"
            }),
            expected: None,
        },
        Case {
            label: "metric-source-outside-application-signals",
            metric_config: json!({
                "KeyAttributes": key_attributes("canary"),
                "MetricType": METRIC_TYPES[1],
                "MetricSource": {
                    "MetricSourceKeyAttributes": { "Type": "AWS::Synthetics::Canary", "Name": "probe" }
                }
            }),
            expected: None,
        },
        Case {
            label: "metric-type-without-key-attributes",
            metric_config: json!({
                "MetricType": METRIC_TYPES[1],
                "TotalRequestCountMetric": [metric_data_query("t1", "RequestCount")]
            }),
            expected: None,
        },
        Case {
            label: "no-metric-type-requested",
            metric_config: json!({ "KeyAttributes": key_attributes("orders") }),
            expected: None,
        },
    ];

    let router = router();
    for case in &cases {
        let name = format!("request-{}", case.label);
        let (status, created) = create(
            &router,
            json!({
                "Name": name,
                "RequestBasedSliConfig": { "RequestBasedSliMetricConfig": case.metric_config.clone() }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateServiceLevelObjective, got {status} with {created}",
            case.label
        );
        assert_eq!(
            request_based_metric_type(&created["Slo"]),
            case.expected,
            "case {:?}: expected Slo.RequestBasedSli.RequestBasedSliMetric.MetricType {:?} on create, got {:?}",
            case.label,
            case.expected,
            created["Slo"]["RequestBasedSli"]["RequestBasedSliMetric"]["MetricType"]
        );

        let (status, fetched) = call(
            &router,
            "GET",
            &format!("/slo/{}", name_path_segment(&name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetServiceLevelObjective, got {status} with {fetched}",
            case.label
        );
        assert_eq!(
            request_based_metric_type(&fetched["Slo"]),
            case.expected,
            "case {:?}: expected Slo.RequestBasedSli.RequestBasedSliMetric.MetricType {:?} on read-back, got {:?}",
            case.label,
            case.expected,
            fetched["Slo"]["RequestBasedSli"]["RequestBasedSliMetric"]["MetricType"]
        );

        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/slo/{}", name_path_segment(&name)),
            Some(json!({
                "RequestBasedSliConfig": { "RequestBasedSliMetricConfig": case.metric_config.clone() }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with {updated}",
            case.label
        );
        assert_eq!(
            request_based_metric_type(&updated["Slo"]),
            case.expected,
            "case {:?}: expected Slo.RequestBasedSli.RequestBasedSliMetric.MetricType {:?} on update, got {:?}",
            case.label,
            case.expected,
            updated["Slo"]["RequestBasedSli"]["RequestBasedSliMetric"]["MetricType"]
        );
    }
}

/// Switching a stored SLO from a standard-metric config to one that names its own metric
/// DROPS the member, and switching back restores it: the answer follows the config that is
/// stored now, not the one the record was created with.
#[tokio::test]
async fn an_update_that_replaces_the_config_re_decides_the_member() {
    let router = router();
    let standard = json!({
        "SliMetricConfig": {
            "KeyAttributes": key_attributes("checkout"),
            "OperationName": "POST /checkout",
            "MetricType": METRIC_TYPES[0]
        }
    });
    let explicit = json!({
        "SliMetricConfig": {
            "KeyAttributes": key_attributes("checkout"),
            "OperationName": "POST /checkout",
            "MetricType": METRIC_TYPES[0],
            "MetricDataQueries": [metric_data_query("m1", "TargetResponseTime")]
        }
    });

    let (status, created) = create(
        &router,
        json!({ "Name": "switching-slo", "SliConfig": standard.clone() }),
    )
    .await;
    assert_eq!(status, 200, "expected the setup create to succeed, got {status} with {created}");
    assert_eq!(
        sli_metric_type(&created["Slo"]),
        Some(METRIC_TYPES[0]),
        "expected the standard-metric create to report {:?}, got {:?}",
        METRIC_TYPES[0],
        created["Slo"]["Sli"]["SliMetric"]["MetricType"]
    );

    for (label, config, expected) in [
        ("to an explicit metric", explicit, None),
        ("back to the standard metric", standard, Some(METRIC_TYPES[0])),
    ] {
        let (status, updated) = call(
            &router,
            "PATCH",
            "/slo/switching-slo",
            Some(json!({ "SliConfig": config })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with {updated}"
        );
        assert_eq!(
            sli_metric_type(&updated["Slo"]),
            expected,
            "case {label:?}: expected Slo.Sli.SliMetric.MetricType {expected:?}, got {:?}",
            updated["Slo"]["Sli"]["SliMetric"]["MetricType"]
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `MetricSourceType` is derived from the source an SLI NAMES, over every member it can
//! name one in.
//!
//! The rule under test: an SLO's `MetricSourceType` — "the SLI metric source type for this
//! SLO", whose enum admits exactly `ServiceOperation`, `CloudWatchMetric`,
//! `ServiceDependency`, `AppMonitor`, `Canary` and `Service`
//! (`smithy_query(query="shape",shape="MetricSourceType")`) — follows which source members the
//! indicator carries, most specific first: a `MetricSource` (by the last segment of its `Type`
//! key attribute), then a dependency, then an operation, then the service entity; and an
//! indicator that names NO Application Signals entity and no external source, but spells
//! CloudWatch metric queries of its own, is reading plain CloudWatch metrics. "Of its own"
//! is answered over EVERY query member the indicator's kind states them in: a period-based
//! indicator's `MetricDataQueries`, and BOTH of a request-based one's `TotalRequestCountMetric`
//! ("the metric that you want to use as the \"total requests\" number") and
//! `MonitoredRequestCountMetric` ("the metric that you want to use as the \"good request\" or
//! \"bad request\" value"), either of which the model lets a config define its indicator
//! through alone (`smithy_query(query="shape",
//! shape="RequestBasedServiceLevelIndicatorMetricConfig")` leaves both optional). An empty
//! query list names no metric.
//!
//! The same derived value is what the `MetricSourceTypes` selector of
//! `ListServiceLevelObjectives` narrows by, so each row is asserted both as the value a create,
//! a read and an update report and as the set that selector picks out.
//!
//! The harness is `slo_lifecycle`'s, so this suite speaks to the same mounted router as every
//! other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, name_path_segment, router};

/// Every value the modelled `MetricSourceType` enum admits
/// (`smithy_query(query="shape",shape="MetricSourceType")`), so the selector is driven over
/// the whole catalogue rather than the kinds the rows happen to produce.
const METRIC_SOURCE_TYPES: [&str; 6] = [
    "ServiceOperation",
    "CloudWatchMetric",
    "ServiceDependency",
    "AppMonitor",
    "Canary",
    "Service",
];

/// One CloudWatch metric query: the metric an indicator names for itself.
fn query(id: &str, metric_name: &str) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": { "Namespace": "AWS/ApiGateway", "MetricName": metric_name },
            "Period": 60,
            "Stat": "Sum"
        }
    })
}

/// The Application Signals service entity an indicator can relate its metric to.
fn key_attributes(service: &str) -> Value {
    json!({ "Type": "Service", "Name": service, "Environment": "prod" })
}

/// One row: the SLI config an objective is created with, and the `MetricSourceType` the
/// stored record must report for it (`None` when the indicator names no source at all).
struct Case {
    /// Also the objective's name, so a failure names the row.
    label: &'static str,
    /// Which of the two mutually exclusive config members carries `metric_config`.
    config_member: &'static str,
    metric_config: Value,
    expected: Option<&'static str>,
}

/// The rows the rule is asserted over.
///
/// They differ along what the rule generalises over: which source member the indicator names
/// (none, a service, an operation of one, a dependency of one, an external metric source of
/// each kind the enum has a value for), WHICH query member spells the indicator's own metric
/// (a period-based `MetricDataQueries`, a request-based `TotalRequestCountMetric`, each
/// variant of `MonitoredRequestCountMetric`, both request-count members at once), how many
/// queries it holds (none, one, two), and which kind of indicator carries it.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The form the classification was already pinned on.
            label: "rb-total-count-queries",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({ "TotalRequestCountMetric": [query("total", "Count")] }),
            expected: Some("CloudWatchMetric"),
        },
        Case {
            // The same kind of indicator, defined through the OTHER query member only.
            label: "rb-good-count-queries",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "MonitoredRequestCountMetric": { "GoodCountMetric": [query("good", "Count")] }
            }),
            expected: Some("CloudWatchMetric"),
        },
        Case {
            // The union's other variant: bad requests counted instead of good ones.
            label: "rb-bad-count-queries",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "MonitoredRequestCountMetric": { "BadCountMetric": [query("bad", "5XXError")] }
            }),
            expected: Some("CloudWatchMetric"),
        },
        Case {
            label: "rb-total-and-good-count-queries",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "TotalRequestCountMetric": [query("total", "Count")],
                "MonitoredRequestCountMetric": { "GoodCountMetric": [query("good", "2XXCount")] }
            }),
            expected: Some("CloudWatchMetric"),
        },
        Case {
            // Present but EMPTY query lists name no metric, so there is no source to report.
            label: "rb-empty-request-count-queries",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "TotalRequestCountMetric": [],
                "MonitoredRequestCountMetric": { "GoodCountMetric": [] }
            }),
            expected: None,
        },
        Case {
            // An indicator that names nothing at all names no source either.
            label: "rb-no-source-member",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({}),
            expected: None,
        },
        Case {
            // An entity IS named, so the entity decides and the queries do not.
            label: "rb-good-count-queries-on-a-service",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "KeyAttributes": key_attributes("orders"),
                "MonitoredRequestCountMetric": { "GoodCountMetric": [query("good", "Count")] }
            }),
            expected: Some("Service"),
        },
        Case {
            label: "rb-good-count-queries-on-an-operation",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "OperationName": "POST /checkout",
                "MonitoredRequestCountMetric": { "GoodCountMetric": [query("good", "Count")] }
            }),
            expected: Some("ServiceOperation"),
        },
        Case {
            label: "rb-bad-count-queries-on-a-dependency",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "KeyAttributes": key_attributes("orders"),
                "OperationName": "POST /checkout",
                "DependencyConfig": {
                    "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "ledger" },
                    "DependencyOperationName": "Charge"
                },
                "MonitoredRequestCountMetric": { "BadCountMetric": [query("bad", "Faults")] }
            }),
            expected: Some("ServiceDependency"),
        },
        Case {
            // An external metric source outranks the indicator's own queries, and its kind is
            // the last segment of its `Type` key attribute.
            label: "rb-good-count-queries-on-an-app-monitor",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "MetricSource": {
                    "MetricSourceKeyAttributes": { "Type": "AWS::RUM::AppMonitor", "Name": "web" }
                },
                "MonitoredRequestCountMetric": { "GoodCountMetric": [query("good", "Count")] }
            }),
            expected: Some("AppMonitor"),
        },
        Case {
            label: "rb-good-count-queries-on-a-canary",
            config_member: "RequestBasedSliConfig",
            metric_config: json!({
                "MetricSource": {
                    "MetricSourceKeyAttributes": {
                        "Type": "AWS::Synthetics::Canary",
                        "Name": "ping"
                    }
                },
                "MonitoredRequestCountMetric": { "GoodCountMetric": [query("good", "Count")] }
            }),
            expected: Some("Canary"),
        },
        Case {
            // The parallel period-based indicator: one query member, same conclusions.
            label: "pb-metric-data-queries",
            config_member: "SliConfig",
            metric_config: json!({ "MetricDataQueries": [query("errors", "5XXError")] }),
            expected: Some("CloudWatchMetric"),
        },
        Case {
            label: "pb-empty-metric-data-queries",
            config_member: "SliConfig",
            metric_config: json!({ "MetricDataQueries": [] }),
            expected: None,
        },
        Case {
            label: "pb-metric-data-queries-on-an-operation",
            config_member: "SliConfig",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "OperationName": "GET /cart",
                "MetricDataQueries": [query("errors", "5XXError")]
            }),
            expected: Some("ServiceOperation"),
        },
    ]
}

/// The create request body one row describes.
fn create_body(case: &Case) -> Value {
    let mut body = json!({ "Name": case.label });
    body[case.config_member] = inner_config(case);
    body
}

/// The SLI config member of one row, with the metric config under the key its kind uses.
fn inner_config(case: &Case) -> Value {
    let metric_member = if case.config_member == "SliConfig" {
        "SliMetricConfig"
    } else {
        "RequestBasedSliMetricConfig"
    };
    json!({ metric_member: case.metric_config.clone() })
}

/// Store every row, asserting each create answers the expected `MetricSourceType`.
async fn seed(router: &MultiServiceRouter) {
    for case in cases() {
        let (status, created) = create(router, create_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateServiceLevelObjective, got {status} with {created}",
            case.label
        );
        assert_eq!(
            created["Slo"]["MetricSourceType"].as_str(),
            case.expected,
            "case {:?}: expected MetricSourceType {:?} on create, got {:?}",
            case.label,
            case.expected,
            created["Slo"]["MetricSourceType"]
        );
    }
}

/// The objective names one listing reports, in order.
fn names(page: &Value) -> Vec<String> {
    page["SloSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One listing's summaries keyed by name, so a row's own summary can be read out of a page.
fn summaries_by_name(page: &Value) -> std::collections::BTreeMap<String, Value> {
    page["SloSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    (
                        summary["Name"].as_str().unwrap_or_default().to_string(),
                        summary.clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A create, the read-back and an update that re-supplies the same config all report the
/// `MetricSourceType` the indicator's source members decide.
#[tokio::test]
async fn metric_source_type_follows_the_source_the_indicator_names() {
    let router = router();
    seed(&router).await;

    for case in cases() {
        let path = format!("/slo/{}", name_path_segment(case.label));

        let (status, fetched) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetServiceLevelObjective, got {status} with {fetched}",
            case.label
        );
        assert_eq!(
            fetched["Slo"]["MetricSourceType"].as_str(),
            case.expected,
            "case {:?}: expected MetricSourceType {:?} on read-back, got {:?}",
            case.label,
            case.expected,
            fetched["Slo"]["MetricSourceType"]
        );

        let (status, updated) = call(
            &router,
            "PATCH",
            &path,
            Some(json!({ case.config_member: inner_config(&case) })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with {updated}",
            case.label
        );
        assert_eq!(
            updated["Slo"]["MetricSourceType"].as_str(),
            case.expected,
            "case {:?}: expected MetricSourceType {:?} on update, got {:?}",
            case.label,
            case.expected,
            updated["Slo"]["MetricSourceType"]
        );
    }
}

/// The summary a listing reports carries the same derived value as the record itself.
#[tokio::test]
async fn every_summary_reports_the_derived_metric_source_type() {
    let router = router();
    seed(&router).await;

    let (status, page) = call(&router, "POST", "/slos", Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from an unfiltered ListServiceLevelObjectives, got {status} with {page}"
    );
    let summaries = summaries_by_name(&page);
    for case in cases() {
        let summary = summaries.get(case.label).unwrap_or(&Value::Null);
        assert_eq!(
            summary["MetricSourceType"].as_str(),
            case.expected,
            "case {:?}: expected the summary's MetricSourceType {:?}, got {summary}",
            case.label,
            case.expected
        );
    }
}

/// The `MetricSourceTypes` selector reports exactly the objectives whose derived kind is one
/// of those it names — every value of the modelled enum, one at a time and in pairs — and an
/// objective that states no source kind is reported by no such listing.
#[tokio::test]
async fn the_metric_source_types_selector_reports_every_objective_of_the_kinds_it_names() {
    let router = router();
    seed(&router).await;

    // Each single kind, then every adjacent pair, so a selector naming more than one kind is
    // the union of what each names.
    let mut selections: Vec<Vec<&str>> = METRIC_SOURCE_TYPES.iter().map(|kind| vec![*kind]).collect();
    for pair in METRIC_SOURCE_TYPES.windows(2) {
        selections.push(pair.to_vec());
    }

    for kinds in selections {
        let mut expected: Vec<String> = cases()
            .into_iter()
            .filter(|case| {
                case.expected
                    .is_some_and(|kind| kinds.contains(&kind))
            })
            .map(|case| case.label.to_string())
            .collect();
        // The listing reports summaries in SLO-name order (the store is ordered by name), so
        // the expected set is compared in that order.
        expected.sort();

        let (status, page) = call(
            &router,
            "POST",
            "/slos",
            Some(json!({ "MetricSourceTypes": kinds })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {kinds:?}: expected HTTP 200 from ListServiceLevelObjectives, got {status} with {page}"
        );
        assert_eq!(
            names(&page),
            expected,
            "case {kinds:?}: expected {expected:?}, got {:?}",
            names(&page)
        );
    }
}

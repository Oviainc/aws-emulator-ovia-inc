// SPDX-License-Identifier: CC0-1.0

//! The metric an SLI config describes surfaces in the indicator's query list.
//!
//! The rule under test: an SLI metric config describes the metric the SLO evaluates either
//! as explicit `MetricDataQueries` or as that metric's parts — `MetricName`, `Statistic`
//! and `PeriodSeconds`, which are exactly the `Metric`, `Stat` and `Period` of a
//! `MetricStat` — and since the returned `ServiceLevelIndicatorMetric` declares no member
//! of its own for any of those parts
//! (`smithy_query(query="shape", shape="ServiceLevelIndicatorMetric")`), its `@required`
//! `MetricDataQueries` is where they are reported, for whichever operation serialises the
//! SLO (create, read or update).
//!
//! Where the config states no statistic, nothing in the request names the `@required`
//! `MetricStat.Stat` and the required list is reported empty rather than carrying an
//! invented one; the same holds for the request-based indicator's request-count members,
//! whose config declares neither a statistic nor a period at all.

use serde_json::{json, Value};

use super::sli_metric_type::key_attributes;
use super::slo_lifecycle::{call, create, name_path_segment, router};

/// The period-based indicator's query list in a returned `Slo`.
fn sli_queries(slo: &Value) -> &Value {
    &slo["Sli"]["SliMetric"]["MetricDataQueries"]
}

/// Create the SLO, read it back and update it with the same config, and hand back the
/// `Slo` each of the three operations answered with.
///
/// Every assertion below is made against all three, because the rule is about the record
/// that was STORED and not about one response: the same two helpers build the indicator for
/// a create and for an update.
async fn create_read_and_update(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
    config: &Value,
) -> Vec<(&'static str, Value)> {
    let (status, created) = create(router, json!({ "Name": name, "SliConfig": config })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateServiceLevelObjective, got {status} with \
         {created}"
    );
    let (status, fetched) = call(
        router,
        "GET",
        &format!("/slo/{}", name_path_segment(name)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetServiceLevelObjective, got {status} with \
         {fetched}"
    );
    let (status, updated) = call(
        router,
        "PATCH",
        &format!("/slo/{}", name_path_segment(name)),
        Some(json!({ "SliConfig": config })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with \
         {updated}"
    );
    vec![
        ("create", created["Slo"].clone()),
        ("read", fetched["Slo"].clone()),
        ("update", updated["Slo"].clone()),
    ]
}

/// A config that names the metric's parts is reported as the ONE query those parts
/// describe, carrying the requested statistic and evaluation period.
///
/// The rows differ along what the rule generalises over: the statistic's form (a
/// percentile, a named statistic, an extended percentile), the period (each published bound
/// of the modelled `range 60..900`, a value inside it, and the member omitted), and whether
/// the config names a `MetricName` of its own.
#[tokio::test]
async fn the_metric_a_config_names_is_materialised_into_one_query() {
    /// One config and the `MetricStat` the indicator's single query must carry for it.
    struct Case {
        label: &'static str,
        metric_config: Value,
        /// `MetricStat.Stat`.
        stat: &'static str,
        /// `MetricStat.Period`.
        period: i64,
        /// `MetricStat.Metric.MetricName`, absent when the config names none.
        metric_name: Option<&'static str>,
    }

    // `Statistic` is "any CloudWatch statistic or extended statistic"
    // (`smithy_query(shape="ServiceLevelIndicatorMetricConfig", member="Statistic")`), whose
    // modelled form is `length 1..20`, `pattern ^[a-zA-Z0-9.]+$`. `PeriodSeconds` is
    // `range 60..900`, so 60 and 900 are its published bounds.
    let cases = vec![
        Case {
            label: "percentile-statistic-at-the-period-minimum",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "MetricType": "LATENCY",
                "Statistic": "p99",
                "PeriodSeconds": 60
            }),
            stat: "p99",
            period: 60,
            metric_name: None,
        },
        Case {
            label: "named-statistic-at-the-period-maximum",
            metric_config: json!({
                "KeyAttributes": key_attributes("orders"),
                "Statistic": "Average",
                "PeriodSeconds": 900
            }),
            stat: "Average",
            period: 900,
            metric_name: None,
        },
        Case {
            label: "custom-metric-name-inside-the-period-range",
            metric_config: json!({
                "KeyAttributes": key_attributes("checkout"),
                "OperationName": "POST /checkout",
                "MetricName": "RenderTime",
                "Statistic": "Sum",
                "PeriodSeconds": 300
            }),
            stat: "Sum",
            period: 300,
            metric_name: Some("RenderTime"),
        },
        Case {
            label: "custom-metric-name-with-no-period-requested",
            metric_config: json!({
                "MetricName": "Duration",
                "Statistic": "p50.5"
            }),
            stat: "p50.5",
            // Omitted: the shortest period an SLO evaluation may use, which is the modelled
            // `SLIPeriodSeconds` minimum.
            period: 60,
            metric_name: Some("Duration"),
        },
        Case {
            label: "statistic-at-its-longest-modelled-form",
            metric_config: json!({
                "MetricName": "Latency",
                // 20 characters, the modelled `length` maximum of `Statistic`.
                "Statistic": "TM99.99999999999999",
                "PeriodSeconds": 120
            }),
            stat: "TM99.99999999999999",
            period: 120,
            metric_name: Some("Latency"),
        },
        Case {
            label: "an-empty-query-list-beside-the-named-parts",
            metric_config: json!({
                "KeyAttributes": key_attributes("payments"),
                "MetricDataQueries": [],
                "Statistic": "Maximum",
                "PeriodSeconds": 180
            }),
            stat: "Maximum",
            period: 180,
            metric_name: None,
        },
    ];

    let router = router();
    for case in &cases {
        let name = format!("materialised-{}", case.label);
        let config = json!({ "SliMetricConfig": case.metric_config.clone() });
        for (operation, slo) in create_read_and_update(&router, case.label, &name, &config).await {
            let case_name = format!("{} on {operation}", case.label);
            let queries = sli_queries(&slo).as_array().cloned().unwrap_or_default();
            assert_eq!(
                queries.len(),
                1,
                "case {case_name:?}: expected exactly one materialised query, got {:?}",
                sli_queries(&slo)
            );
            let query = &queries[0];
            // `MetricDataQuery.Id` is required, and its doc fixes the form: "the valid
            // characters are letters, numbers, and underscore. The first character must be a
            // lowercase letter."
            let id = query["Id"].as_str().unwrap_or_default();
            assert!(
                id.starts_with(|first: char| first.is_ascii_lowercase())
                    && id
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_'),
                "case {case_name:?}: expected a query Id of letters, numbers and underscores \
                 starting with a lowercase letter, got {:?}",
                query["Id"]
            );
            let stat = &query["MetricStat"];
            assert_eq!(
                stat["Stat"].as_str(),
                Some(case.stat),
                "case {case_name:?}: expected MetricStat.Stat {:?}, got {:?}",
                case.stat,
                stat["Stat"]
            );
            assert_eq!(
                stat["Period"].as_i64(),
                Some(case.period),
                "case {case_name:?}: expected MetricStat.Period {}, got {:?}",
                case.period,
                stat["Period"]
            );
            assert_eq!(
                stat["Metric"]["MetricName"].as_str(),
                case.metric_name,
                "case {case_name:?}: expected MetricStat.Metric.MetricName {:?}, got {:?}",
                case.metric_name,
                stat["Metric"]["MetricName"]
            );
            // Nothing in the request names the metric's namespace or its dimension set, so
            // neither is reported.
            for member in ["Namespace", "Dimensions"] {
                assert!(
                    stat["Metric"][member].is_null(),
                    "case {case_name:?}: expected no MetricStat.Metric.{member}, got {:?}",
                    stat["Metric"][member]
                );
            }
            assert!(
                query["Expression"].is_null(),
                "case {case_name:?}: a single metric is no math expression, got Expression {:?}",
                query["Expression"]
            );
        }
    }
}

/// An update that supplies a NEW config re-materialises the query, so the statistic and
/// period a client reads back are always the ones last requested.
#[tokio::test]
async fn an_update_reports_the_newly_requested_statistic_and_period() {
    let router = router();
    let name = "re-materialised-after-update";
    let (status, created) = create(
        &router,
        json!({
            "Name": name,
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": key_attributes("checkout"),
                    "MetricType": "LATENCY",
                    "Statistic": "p99",
                    "PeriodSeconds": 60
                }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create\": expected HTTP 200, got {status} with {created}"
    );
    assert_eq!(
        (
            sli_queries(&created["Slo"])[0]["MetricStat"]["Stat"].as_str(),
            sli_queries(&created["Slo"])[0]["MetricStat"]["Period"].as_i64()
        ),
        (Some("p99"), Some(60)),
        "case \"create\": expected the created query to carry p99 over 60 seconds, got {:?}",
        sli_queries(&created["Slo"])
    );

    let (status, updated) = call(
        &router,
        "PATCH",
        &format!("/slo/{}", name_path_segment(name)),
        Some(json!({
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": key_attributes("checkout"),
                    "MetricType": "LATENCY",
                    "Statistic": "p90",
                    "PeriodSeconds": 600
                }
            }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"update\": expected HTTP 200, got {status} with {updated}"
    );
    let (status, fetched) = call(
        &router,
        "GET",
        &format!("/slo/{}", name_path_segment(name)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"read-back\": expected HTTP 200, got {status} with {fetched}"
    );
    for (operation, slo) in [("update", &updated["Slo"]), ("read-back", &fetched["Slo"])] {
        assert_eq!(
            (
                sli_queries(slo)[0]["MetricStat"]["Stat"].as_str(),
                sli_queries(slo)[0]["MetricStat"]["Period"].as_i64()
            ),
            (Some("p90"), Some(600)),
            "case {operation:?}: expected the updated query to carry p90 over 600 seconds, got \
             {:?}",
            sli_queries(slo)
        );
    }
}

/// A config that spells its queries out is reported with exactly those queries: the parts
/// it also names change nothing, because each query already carries its own `Period` and
/// `Stat`.
///
/// The rows differ along the COUNT and KIND of query the config supplies (one metric,
/// several metrics combined by a metric-math expression) and whether the config also names
/// the metric's parts.
#[tokio::test]
async fn a_config_that_spells_out_its_queries_is_reported_verbatim() {
    let one_query = json!([{
        "Id": "m1",
        "MetricStat": {
            "Metric": {
                "Namespace": "AWS/Lambda",
                "MetricName": "Duration",
                "Dimensions": [{ "Name": "FunctionName", "Value": "checkout" }]
            },
            "Period": 60,
            "Stat": "p99",
            "Unit": "Milliseconds"
        },
        "ReturnData": true
    }]);
    let math_queries = json!([
        {
            "Id": "m1",
            "MetricStat": {
                "Metric": { "Namespace": "AWS/ApplicationELB", "MetricName": "RequestCount" },
                "Period": 300,
                "Stat": "Sum"
            },
            "ReturnData": false
        },
        {
            "Id": "m2",
            "MetricStat": {
                "Metric": {
                    "Namespace": "AWS/ApplicationELB",
                    "MetricName": "HTTPCode_Target_5XX_Count"
                },
                "Period": 300,
                "Stat": "Sum"
            },
            "ReturnData": false
        },
        { "Id": "e1", "Expression": "m2 / m1 * 100", "Label": "fault rate", "ReturnData": true }
    ]);

    let cases = vec![
        ("one-explicit-query", one_query.clone(), None),
        ("a-metric-math-expression", math_queries, None),
        (
            "explicit-queries-beside-the-named-parts",
            one_query,
            Some(json!({ "MetricName": "Ignored", "Statistic": "Average", "PeriodSeconds": 900 })),
        ),
    ];

    let router = router();
    for (label, queries, also_named) in &cases {
        let mut metric_config = json!({
            "KeyAttributes": key_attributes("checkout"),
            "MetricDataQueries": queries.clone()
        });
        if let Some(extra) = also_named {
            for (member, value) in extra.as_object().expect("the row's extra members") {
                metric_config[member] = value.clone();
            }
        }
        let name = format!("verbatim-{label}");
        let config = json!({ "SliMetricConfig": metric_config });
        for (operation, slo) in create_read_and_update(&router, label, &name, &config).await {
            assert_eq!(
                sli_queries(&slo),
                queries,
                "case {:?}: expected the config's own queries {queries:?} on {operation}, got {:?}",
                label,
                sli_queries(&slo)
            );
        }
    }
}

/// A config that names NO statistic reports an EMPTY query list: `MetricStat.Stat` is
/// required with no default, nothing in such a request names one, and the query the real
/// service builds for a standard metric comes from its own catalogue (a namespace, a
/// dimension set and a query id) rather than from the request.
///
/// The rows differ along which members the config does name.
#[tokio::test]
async fn a_config_that_names_no_statistic_reports_an_empty_query_list() {
    let cases = vec![
        (
            "a-standard-metric-type",
            json!({ "KeyAttributes": key_attributes("orders"), "MetricType": "AVAILABILITY" }),
        ),
        (
            "a-metric-name-alone",
            json!({ "KeyAttributes": key_attributes("checkout"), "MetricName": "Latency" }),
        ),
        (
            "a-period-alone",
            json!({ "KeyAttributes": key_attributes("checkout"), "PeriodSeconds": 300 }),
        ),
        (
            "a-metric-source-outside-application-signals",
            json!({
                "MetricSource": {
                    "MetricSourceKeyAttributes": { "Type": "AWS::RUM::AppMonitor", "Name": "web" }
                }
            }),
        ),
        ("nothing-at-all", json!({})),
    ];

    let router = router();
    for (label, metric_config) in &cases {
        let name = format!("no-statistic-{label}");
        let config = json!({ "SliMetricConfig": metric_config.clone() });
        for (operation, slo) in create_read_and_update(&router, label, &name, &config).await {
            assert_eq!(
                sli_queries(&slo),
                &json!([]),
                "case {label:?}: expected an empty MetricDataQueries on {operation}, got {:?}",
                sli_queries(&slo)
            );
        }
    }
}

/// A `PeriodSeconds` outside the modelled `range 60..900` is refused with HTTP 400 rather
/// than materialised into a query, one step past each published bound.
#[tokio::test]
async fn a_period_outside_the_modelled_range_is_refused() {
    let router = router();
    for period in [59, 901] {
        let (status, error) = create(
            &router,
            json!({
                "Name": format!("period-{period}"),
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": key_attributes("checkout"),
                        "Statistic": "p99",
                        "PeriodSeconds": period
                    }
                }
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case PeriodSeconds={period}: expected HTTP 400 outside the modelled range 60..900, \
             got {status} with {error}"
        );
    }
}

/// The request-based indicator reports the request-count queries its config supplied, and
/// keeps the union variant the config chose.
///
/// The rows differ along which variant the config names (`GoodCountMetric`,
/// `BadCountMetric`) and whether it names the monitored count at all — that config declares
/// neither a statistic nor a period, so a config supplying neither list describes no query
/// to report and the required members stay empty.
#[tokio::test]
async fn the_request_based_indicator_reports_the_request_count_queries_it_was_given() {
    fn count_query(id: &str, metric_name: &str) -> Value {
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

    let cases = vec![
        (
            "good-count-queries",
            json!({
                "KeyAttributes": key_attributes("checkout"),
                "TotalRequestCountMetric": [count_query("t1", "RequestCount")],
                "MonitoredRequestCountMetric": {
                    "GoodCountMetric": [count_query("g1", "HTTPCode_Target_2XX_Count")]
                }
            }),
        ),
        (
            "bad-count-queries",
            json!({
                "KeyAttributes": key_attributes("orders"),
                "TotalRequestCountMetric": [count_query("t1", "RequestCount")],
                "MonitoredRequestCountMetric": {
                    "BadCountMetric": [count_query("b1", "HTTPCode_Target_5XX_Count")]
                }
            }),
        ),
        (
            "no-request-count-queries",
            json!({ "KeyAttributes": key_attributes("payments"), "MetricType": "AVAILABILITY" }),
        ),
    ];

    let router = router();
    for (label, metric_config) in &cases {
        let name = format!("request-based-{label}");
        let (status, created) = create(
            &router,
            json!({
                "Name": name,
                "RequestBasedSliConfig": { "RequestBasedSliMetricConfig": metric_config.clone() }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with {created}"
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
            "case {label:?}: expected HTTP 200 reading the SLO back, got {status} with {fetched}"
        );
        for (operation, slo) in [("create", &created["Slo"]), ("read", &fetched["Slo"])] {
            let metric = &slo["RequestBasedSli"]["RequestBasedSliMetric"];
            let expected_total = metric_config["TotalRequestCountMetric"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                metric["TotalRequestCountMetric"],
                Value::Array(expected_total.clone()),
                "case {label:?}: expected TotalRequestCountMetric {expected_total:?} on \
                 {operation}, got {:?}",
                metric["TotalRequestCountMetric"]
            );
            let expected_monitored = if metric_config["MonitoredRequestCountMetric"].is_null() {
                // The member is required on the indicator and the config named neither
                // variant, so the good-count list is reported empty.
                json!({ "GoodCountMetric": [] })
            } else {
                metric_config["MonitoredRequestCountMetric"].clone()
            };
            assert_eq!(
                metric["MonitoredRequestCountMetric"], expected_monitored,
                "case {label:?}: expected MonitoredRequestCountMetric {expected_monitored:?} on \
                 {operation}, got {:?}",
                metric["MonitoredRequestCountMetric"]
            );
        }
    }
}

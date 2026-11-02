// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetMetricData`: every result is computed from the datapoints
//! `PutMetricData` stored, folded into the query's `Period` windows by its `Stat`, inside
//! the requested range and ordered as `ScanBy` asks.
//!
//! Every suite here drives the service over the wire it speaks (`awsJson1_0`, `POST /` with
//! `X-Amz-Target`), so a response that is not computed from the shared store cannot pass.
//!
//! The tables span the classes the rules generalise over — which statistic is asked for,
//! which period the windows are, which dimension set and unit the query names, which order
//! and page size the datapoints come back in — so any one recorded request is a single row.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, ok};

/// One minute, in the epoch seconds every published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series in this suite is published under.
const NAMESPACE: &str = "MyApp/MetricData";

/// A minute-aligned instant ten minutes in the past: recent enough that the published
/// datapoints sit inside CloudWatch's own retention, and aligned so that
/// `GetMetricDataInput$StartTime`'s documented rounding ("Start time less than 15 days ago -
/// Round down to the nearest whole minute") leaves it exactly where the request put it.
fn base_time() -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now / MINUTE).floor() * MINUTE - 10.0 * MINUTE
}

/// Publish `(offset in seconds from `base`, value)` into one series of [`NAMESPACE`].
async fn publish(
    router: &smithy_server_runtime::MultiServiceRouter,
    metric: &str,
    dimensions: &Value,
    unit: &str,
    base: f64,
    points: &[(f64, f64)],
) {
    let data: Vec<Value> = points
        .iter()
        .map(|(offset, value)| {
            json!({
                "MetricName": metric,
                "Dimensions": dimensions,
                "Value": value,
                "Unit": unit,
                "Timestamp": base + offset,
            })
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": data}),
    )
    .await;
}

/// One `MetricStat` query over the `Service=api` series of `metric`.
fn metric_stat_query(id: &str, metric: &str, period: i64, stat: &str) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": {
                "Namespace": NAMESPACE,
                "MetricName": metric,
                "Dimensions": [{"Name": "Service", "Value": "api"}],
            },
            "Period": period,
            "Stat": stat,
        },
    })
}

/// The `(timestamp, value)` pairs one result carries, in the order the response listed them.
fn pairs(result: &Value) -> Vec<(f64, f64)> {
    let timestamps: Vec<f64> = result
        .get("Timestamps")
        .and_then(Value::as_array)
        .map(|listed| listed.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let values: Vec<f64> = result
        .get("Values")
        .and_then(Value::as_array)
        .map(|listed| listed.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    assert_eq!(
        timestamps.len(),
        values.len(),
        "a result's Timestamps and Values stay in step, got {result}"
    );
    timestamps.into_iter().zip(values).collect()
}

/// The results of one response, in the order it listed them.
fn results(body: &Value) -> Vec<Value> {
    body.get("MetricDataResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("GetMetricData returns MetricDataResults, got {body}"))
}

/// The single result of a one-query response.
fn single_result(body: &Value) -> Value {
    let listed = results(body);
    assert_eq!(
        listed.len(),
        1,
        "a one-query request answers one result, got {body}"
    );
    listed[0].clone()
}

/// A string member of a result.
fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the window table: the case, the query's `Period`, how far past the base the
/// range ends (in minutes), and the `(window offset, Sum)` pairs the response must carry.
type WindowCase = (&'static str, i64, f64, Vec<(f64, f64)>);

/// One row of the ordering table: the case, the `ScanBy` the request names, and the order
/// the datapoints must come back in.
type OrderCase<'a> = (&'static str, Option<&'static str>, &'a Vec<(f64, f64)>);

/// Every statistic is folded out of the SAME stored datapoints: the rows differ along which
/// statistic the query asks for, and the two windows differ along which datapoints fall in
/// them.
#[tokio::test]
async fn each_statistic_is_folded_from_the_stored_datapoints_of_its_window() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        "Latency",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Milliseconds",
        base,
        // Two 60-second windows: 10 and 30 in the first, 50 and 70 in the second.
        &[(0.0, 10.0), (30.0, 30.0), (60.0, 50.0), (90.0, 70.0)],
    )
    .await;

    // (the Stat asked for, the value of the first window, the value of the second)
    // `MetricStat$Stat`: "It can include any CloudWatch statistic or extended statistic."
    let rows: [(&str, f64, f64); 6] = [
        ("Sum", 40.0, 120.0),
        ("Average", 20.0, 60.0),
        ("Minimum", 10.0, 50.0),
        ("Maximum", 30.0, 70.0),
        ("SampleCount", 2.0, 2.0),
        // A percentile by nearest rank over the two published values of each window.
        ("p50", 10.0, 50.0),
    ];
    for (stat, first, second) in rows {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [metric_stat_query("m1", "Latency", 60, stat)],
                "StartTime": base,
                "EndTime": base + 2.0 * MINUTE,
                "ScanBy": "TimestampAscending",
            }),
        )
        .await;
        let result = single_result(&body);
        assert_eq!(
            pairs(&result),
            vec![(base, first), (base + MINUTE, second)],
            "case stat {stat}: expected the two windows folded by {stat}, got {result}"
        );
        assert_eq!(
            text(&result, "Id"),
            "m1",
            "case stat {stat}: a result is reported under its query's Id, got {result}"
        );
        assert_eq!(
            text(&result, "StatusCode"),
            "Complete",
            "case stat {stat}: every datapoint of the range was returned, got {result}"
        );
        assert_eq!(
            text(&result, "Label"),
            "Latency",
            "case stat {stat}: an omitted Label defaults to the metric, got {result}"
        );
    }
}

/// The window width is the query's `Period`, and only the datapoints inside
/// `[StartTime, EndTime)` are folded: the rows differ along the period and along where the
/// range's bounds fall.
#[tokio::test]
async fn the_period_sizes_the_windows_and_the_range_bounds_them() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        "Requests",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Count",
        base,
        &[
            (-MINUTE, 1.0),
            (0.0, 2.0),
            (60.0, 4.0),
            (120.0, 8.0),
            (180.0, 16.0),
        ],
    )
    .await;

    // (case, period, range end offset in minutes, the (window offset, Sum) pairs expected)
    let rows: [WindowCase; 4] = [
        (
            "one window per minute",
            60,
            3.0,
            vec![(0.0, 2.0), (60.0, 4.0), (120.0, 8.0)],
        ),
        (
            "a three-minute period folds the same datapoints into one window",
            180,
            3.0,
            vec![(0.0, 14.0)],
        ),
        (
            "a five-minute period reaches past the data and still reports one window",
            300,
            5.0,
            vec![(0.0, 30.0)],
        ),
        (
            "the range's end is exclusive, so the datapoint on it is left out",
            60,
            1.0,
            vec![(0.0, 2.0)],
        ),
    ];
    for (case, period, end_minutes, expected) in rows {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [metric_stat_query("m1", "Requests", period, "Sum")],
                "StartTime": base,
                "EndTime": base + end_minutes * MINUTE,
                "ScanBy": "TimestampAscending",
            }),
        )
        .await;
        let result = single_result(&body);
        let expected: Vec<(f64, f64)> = expected
            .into_iter()
            .map(|(offset, value)| (base + offset, value))
            .collect();
        assert_eq!(
            pairs(&result),
            expected,
            "case {case:?}: expected {expected:?}, got {result}"
        );
    }
    // The datapoint a minute BEFORE the range is never folded in, whatever period is used:
    // the sums above never carry its value of 1.
}

/// A query answers exactly the series its `MetricStat` identifies: the rows differ along the
/// dimension set, the metric name, the namespace and the unit filter — each naming another
/// series, or none at all.
#[tokio::test]
async fn a_query_answers_only_the_series_it_identifies() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        "Latency",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Milliseconds",
        base,
        &[(0.0, 10.0)],
    )
    .await;
    publish(
        &router,
        "Latency",
        &json!([{"Name": "Service", "Value": "worker"}]),
        "Milliseconds",
        base,
        &[(0.0, 100.0)],
    )
    .await;
    publish(
        &router,
        "Latency",
        &json!([]),
        "Milliseconds",
        base,
        &[(0.0, 1_000.0)],
    )
    .await;

    // (case, the Metric the query names, the Unit it filters on, the values expected)
    let rows: [(&str, Value, Option<&str>, Vec<f64>); 6] = [
        (
            "the dimensioned series",
            json!({"Namespace": NAMESPACE, "MetricName": "Latency", "Dimensions": [{"Name": "Service", "Value": "api"}]}),
            None,
            vec![10.0],
        ),
        (
            "another dimension value is another series",
            json!({"Namespace": NAMESPACE, "MetricName": "Latency", "Dimensions": [{"Name": "Service", "Value": "worker"}]}),
            None,
            vec![100.0],
        ),
        (
            "no dimensions at all is a series of its own",
            json!({"Namespace": NAMESPACE, "MetricName": "Latency"}),
            None,
            vec![1_000.0],
        ),
        (
            "a dimension nothing published names no series",
            json!({"Namespace": NAMESPACE, "MetricName": "Latency", "Dimensions": [{"Name": "Service", "Value": "absent"}]}),
            None,
            vec![],
        ),
        (
            "the published unit lets the data through",
            json!({"Namespace": NAMESPACE, "MetricName": "Latency", "Dimensions": [{"Name": "Service", "Value": "api"}]}),
            Some("Milliseconds"),
            vec![10.0],
        ),
        (
            "another unit matches nothing: CloudWatch performs no unit conversions",
            json!({"Namespace": NAMESPACE, "MetricName": "Latency", "Dimensions": [{"Name": "Service", "Value": "api"}]}),
            Some("Seconds"),
            vec![],
        ),
    ];
    for (case, metric, unit, expected) in rows {
        let mut stat = json!({"Metric": metric, "Period": 60, "Stat": "Sum"});
        if let Some(unit) = unit {
            stat["Unit"] = json!(unit);
        }
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [{"Id": "m1", "MetricStat": stat}],
                "StartTime": base,
                "EndTime": base + MINUTE,
            }),
        )
        .await;
        let result = single_result(&body);
        let values: Vec<f64> = pairs(&result).into_iter().map(|(_, value)| value).collect();
        assert_eq!(
            values, expected,
            "case {case:?}: expected {expected:?}, got {result}"
        );
        assert_eq!(
            text(&result, "StatusCode"),
            "Complete",
            "case {case:?}: a series with no datapoint in the range is still complete, got {result}"
        );
    }

    // A series nothing published at all answers a result with no datapoint rather than a
    // missing-resource refusal, and an explicit Label is reported as given.
    let body = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [{
                "Id": "absent",
                "Label": "nothing here",
                "MetricStat": {
                    "Metric": {"Namespace": "MyApp/Nothing", "MetricName": "Never"},
                    "Period": 60,
                    "Stat": "Sum",
                },
            }],
            "StartTime": base,
            "EndTime": base + MINUTE,
        }),
    )
    .await;
    let result = single_result(&body);
    assert!(
        pairs(&result).is_empty(),
        "case a metric nothing published: expected no datapoint, got {result}"
    );
    assert_eq!(
        text(&result, "Label"),
        "nothing here",
        "case a metric nothing published: an explicit Label is reported as given, got {result}"
    );
}

/// `ScanBy` orders the datapoints, `MaxDatapoints` bounds how many one page carries, and the
/// `NextToken` of a cut page returns the rest exactly once.
#[tokio::test]
async fn scan_by_orders_the_datapoints_and_max_datapoints_pages_them() {
    let router = build_router();
    let base = base_time();
    let published: Vec<(f64, f64)> = (0..4)
        .map(|window| (f64::from(window) * MINUTE, f64::from(window) + 1.0))
        .collect();
    publish(
        &router,
        "Paged",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Count",
        base,
        &published,
    )
    .await;

    let ascending: Vec<(f64, f64)> = published
        .iter()
        .map(|(offset, value)| (base + offset, *value))
        .collect();
    let mut descending = ascending.clone();
    descending.reverse();

    // (case, the ScanBy the request names, the order the datapoints come back in)
    let orders: [OrderCase<'_>; 3] = [
        ("TimestampAscending", Some("TimestampAscending"), &ascending),
        (
            "TimestampDescending",
            Some("TimestampDescending"),
            &descending,
        ),
        // "If you omit this parameter, the default of TimestampDescending is used."
        ("omitted", None, &descending),
    ];
    for (case, scan_by, expected) in orders {
        let mut request = json!({
            "MetricDataQueries": [metric_stat_query("m1", "Paged", 60, "Sum")],
            "StartTime": base,
            "EndTime": base + 4.0 * MINUTE,
        });
        if let Some(scan_by) = scan_by {
            request["ScanBy"] = json!(scan_by);
        }
        let body = ok(&router, "GetMetricData", &request).await;
        let result = single_result(&body);
        assert_eq!(
            &pairs(&result),
            expected,
            "case ScanBy {case}: expected {expected:?}, got {result}"
        );
        assert!(
            body.get("NextToken").and_then(Value::as_str).is_none(),
            "case ScanBy {case}: a page that returned everything mints no NextToken, got {body}"
        );
    }

    // (case, the page size, how many pages the four datapoints take)
    for (case, page_size, pages) in [("one at a time", 1, 4), ("two at a time", 2, 2)] {
        let mut collected: Vec<(f64, f64)> = Vec::new();
        let mut token: Option<String> = None;
        let mut seen_pages = 0;
        loop {
            let mut request = json!({
                "MetricDataQueries": [metric_stat_query("m1", "Paged", 60, "Sum")],
                "StartTime": base,
                "EndTime": base + 4.0 * MINUTE,
                "ScanBy": "TimestampAscending",
                "MaxDatapoints": page_size,
            });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let body = ok(&router, "GetMetricData", &request).await;
            let result = single_result(&body);
            let page = pairs(&result);
            assert!(
                page.len() <= page_size,
                "case {case}: a page carries at most {page_size} datapoints, got {result}"
            );
            collected.extend(page);
            seen_pages += 1;
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let expected_status = if token.is_some() {
                "PartialData"
            } else {
                "Complete"
            };
            assert_eq!(
                text(&result, "StatusCode"),
                expected_status,
                "case {case}: a cut page reports PartialData and the last one Complete, got {result}"
            );
            if token.is_none() {
                break;
            }
            assert!(
                seen_pages <= 8,
                "case {case}: paging must terminate, got {seen_pages} pages"
            );
        }
        assert_eq!(
            seen_pages, pages,
            "case {case}: expected {pages} pages, took {seen_pages}"
        );
        assert_eq!(
            collected, ascending,
            "case {case}: paging returns every datapoint exactly once, got {collected:?}"
        );
    }
}

/// Several queries in one call answer side by side, each under its own `Id`, and a query put
/// there only to feed a calculation (`ReturnData` false) contributes no result.
#[tokio::test]
async fn every_query_answers_under_its_own_id_unless_it_asks_not_to_be_returned() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        "Errors",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Count",
        base,
        &[(0.0, 2.0)],
    )
    .await;
    publish(
        &router,
        "Invocations",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Count",
        base,
        &[(0.0, 8.0)],
    )
    .await;

    let body = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [
                metric_stat_query("errors", "Errors", 60, "Sum"),
                metric_stat_query("invocations", "Invocations", 60, "Sum"),
            ],
            "StartTime": base,
            "EndTime": base + MINUTE,
        }),
    )
    .await;
    let listed = results(&body);
    let answered: Vec<(String, Vec<f64>)> = listed
        .iter()
        .map(|result| {
            (
                text(result, "Id"),
                pairs(result).into_iter().map(|(_, value)| value).collect(),
            )
        })
        .collect();
    assert_eq!(
        answered,
        vec![
            ("errors".to_string(), vec![2.0]),
            ("invocations".to_string(), vec![8.0]),
        ],
        "case two queries: each answers under its own Id, got {body}"
    );

    // "If you are performing this call just to do math expressions and do not also need the
    // raw data returned, you can specify false."
    let mut hidden = metric_stat_query("invocations", "Invocations", 60, "Sum");
    hidden["ReturnData"] = json!(false);
    let body = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [metric_stat_query("errors", "Errors", 60, "Sum"), hidden],
            "StartTime": base,
            "EndTime": base + MINUTE,
        }),
    )
    .await;
    let ids: Vec<String> = results(&body)
        .iter()
        .map(|result| text(result, "Id"))
        .collect();
    assert_eq!(
        ids,
        vec!["errors".to_string()],
        "case ReturnData false: that query contributes no result, got {body}"
    );

    // A query whose data is not returned spends none of the `MaxDatapoints` budget either:
    // the budget bounds what a caller receives. The hidden query here carries three
    // datapoints, so a budget of two would be exhausted by it if it counted.
    publish(
        &router,
        "Hidden",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Count",
        base,
        &[(0.0, 1.0), (60.0, 1.0), (120.0, 1.0)],
    )
    .await;
    publish(
        &router,
        "Errors",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Count",
        base,
        &[(60.0, 3.0)],
    )
    .await;
    let mut hidden = metric_stat_query("hidden", "Hidden", 60, "Sum");
    hidden["ReturnData"] = json!(false);
    let body = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [hidden, metric_stat_query("errors", "Errors", 60, "Sum")],
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
            "ScanBy": "TimestampAscending",
            "MaxDatapoints": 2,
        }),
    )
    .await;
    let result = single_result(&body);
    assert_eq!(
        pairs(&result),
        vec![(base, 2.0), (base + MINUTE, 3.0)],
        "case a hidden query beside a returned one: the budget is spent on the returned data only, got {body}"
    );
    assert!(
        body.get("NextToken").and_then(Value::as_str).is_none(),
        "case a hidden query beside a returned one: nothing was cut short, so no NextToken, got {body}"
    );
    assert_eq!(
        text(&result, "StatusCode"),
        "Complete",
        "case a hidden query beside a returned one: the returned series is complete, got {result}"
    );
}

/// The reads report only: two identical calls answer identically, and nothing a later caller
/// reads changes because the data was read.
#[tokio::test]
async fn reading_the_data_changes_nothing_a_later_caller_sees() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        "Latency",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Milliseconds",
        base,
        &[(0.0, 10.0), (30.0, 30.0)],
    )
    .await;

    let request = json!({
        "MetricDataQueries": [metric_stat_query("m1", "Latency", 60, "Sum")],
        "StartTime": base,
        "EndTime": base + MINUTE,
    });
    let before = ok(&router, "ListMetrics", &json!({"Namespace": NAMESPACE})).await;
    let first = ok(&router, "GetMetricData", &request).await;
    let second = ok(&router, "GetMetricData", &request).await;
    let after = ok(&router, "ListMetrics", &json!({"Namespace": NAMESPACE})).await;

    assert_eq!(
        pairs(&single_result(&first)),
        vec![(base, 40.0)],
        "case the first call: expected the window's sum, got {first}"
    );
    assert_eq!(
        second, first,
        "case the second call: a report is repeatable, got {second} after {first}"
    );
    assert_eq!(
        after, before,
        "case the metrics after the read: reading the data must not change them, got {after} after {before}"
    );
}

/// A bad request is refused with the parameter fault it names, and nothing is reported for
/// it: the rows differ along which member is at fault.
#[tokio::test]
async fn a_request_outside_the_contract_is_refused() {
    let router = build_router();
    let base = base_time();
    let query = metric_stat_query("m1", "Latency", 60, "Sum");

    // (case, the request, the error code expected)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "no query at all",
            json!({"MetricDataQueries": [], "StartTime": base, "EndTime": base + MINUTE}),
            "InvalidParameterValueException",
        ),
        (
            // The ids of a query array are the SHAPE of the request, so an array that uses one
            // id twice is refused under the request-validation class rather than as a fault
            // about a parameter value (`crate::metric_query_ids` tables that rule).
            "two queries sharing one Id",
            json!({
                "MetricDataQueries": [query.clone(), query.clone()],
                "StartTime": base,
                "EndTime": base + MINUTE,
            }),
            "ValidationException",
        ),
        (
            "EndTime equal to StartTime",
            json!({"MetricDataQueries": [query.clone()], "StartTime": base, "EndTime": base}),
            "InvalidParameterValueException",
        ),
        (
            "EndTime before StartTime",
            json!({
                "MetricDataQueries": [query.clone()],
                "StartTime": base,
                "EndTime": base - MINUTE,
            }),
            "InvalidParameterValueException",
        ),
        (
            // `ScanBy` is a modelled enum, so the boundary validator refuses a value outside
            // it before the handler runs — under the request-validation class, the one
            // every validation refusal this service makes carries
            // (`crate::metric_stream_scope` tables
            // that rule across the operations modelling such a member).
            "a ScanBy outside the enum",
            json!({
                "MetricDataQueries": [query.clone()],
                "StartTime": base,
                "EndTime": base + MINUTE,
                "ScanBy": "ValueDescending",
            }),
            "ValidationException",
        ),
        (
            "MaxDatapoints below one",
            json!({
                "MetricDataQueries": [query.clone()],
                "StartTime": base,
                "EndTime": base + MINUTE,
                "MaxDatapoints": 0,
            }),
            "InvalidParameterValueException",
        ),
        (
            "a Period that is neither a documented sub-minute value nor a multiple of 60",
            json!({
                "MetricDataQueries": [metric_stat_query("m1", "Latency", 45, "Sum")],
                "StartTime": base,
                "EndTime": base + MINUTE,
            }),
            "InvalidParameterValueException",
        ),
        (
            "a NextToken this operation never minted",
            json!({
                "MetricDataQueries": [query.clone()],
                "StartTime": base,
                "EndTime": base + MINUTE,
                "NextToken": "not-a-token",
            }),
            "InvalidNextToken",
        ),
        (
            "a MetricStat naming no MetricName",
            json!({
                "MetricDataQueries": [{
                    "Id": "m1",
                    "MetricStat": {
                        "Metric": {"Namespace": NAMESPACE},
                        "Period": 60,
                        "Stat": "Sum",
                    },
                }],
                "StartTime": base,
                "EndTime": base + MINUTE,
            }),
            "InvalidParameterCombinationException",
        ),
    ];
    for (case, request, expected) in rows {
        let (status, body) = call(&router, "GetMetricData", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    // Every documented period is accepted: the sub-minute divisions of a minute and the
    // multiples of 60. Source: `MetricStat$Period` — "the period can be 1, 5, 10, 20, 30,
    // 60, or any multiple of 60".
    for period in [1, 5, 10, 20, 30, 60, 120, 300, 3_600] {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [metric_stat_query("m1", "Latency", period, "Sum")],
                "StartTime": base,
                "EndTime": base + 2.0 * MINUTE,
            }),
        )
        .await;
        assert_eq!(
            text(&single_result(&body), "StatusCode"),
            "Complete",
            "case period {period}: a documented period is accepted, got {body}"
        );
    }
}

/// `StatusCode` reports the PAGE a result came back on: a page that hands back a `NextToken`
/// reports every result as `PartialData`, and only the page that exhausts the request's
/// datapoints reports `Complete`.
///
/// "PartialData means that an incomplete set of data points were returned. You can use the
/// NextToken value that was returned and repeat your request to get more data points."
/// (`MetricDataResult$StatusCode`) — so a client that stops paginating when it sees
/// `Complete`, which is what the member documents it to mean, cannot truncate the series.
#[tokio::test]
async fn every_result_of_a_cut_page_is_partial_until_the_last_page() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        "Paged",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Count",
        base,
        &[(0.0, 1.0), (60.0, 2.0), (120.0, 3.0), (180.0, 4.0)],
    )
    .await;

    let mut token: Option<String> = None;
    let mut pages: Vec<(Vec<String>, bool)> = Vec::new();
    for _ in 0..8 {
        let mut request = json!({
            "MetricDataQueries": [
                metric_stat_query("m1", "Paged", 60, "Sum"),
                metric_stat_query("m2", "Paged", 60, "Maximum"),
            ],
            "StartTime": base,
            "EndTime": base + 4.0 * MINUTE,
            "ScanBy": "TimestampAscending",
            "MaxDatapoints": 3,
        });
        if let Some(carried) = token.as_ref() {
            request["NextToken"] = json!(carried);
        }
        let page = ok(&router, "GetMetricData", &request).await;
        let statuses: Vec<String> = results(&page)
            .iter()
            .map(|result| text(result, "StatusCode"))
            .collect();
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        pages.push((statuses, token.is_some()));
        if token.is_none() {
            break;
        }
    }

    assert!(
        pages.len() > 1,
        "case eight datapoints at MaxDatapoints=3: expected several pages, got {pages:?}"
    );
    for (index, (statuses, more)) in pages.iter().enumerate() {
        let expected = if *more { "PartialData" } else { "Complete" };
        assert!(
            !statuses.is_empty() && statuses.iter().all(|status| status == expected),
            "case page {index} (more to come: {more}): expected every result {expected}, got \
             {statuses:?}"
        );
    }
    assert!(
        pages.last().is_some_and(|(_, more)| !more),
        "case the last page: expected no NextToken, got {pages:?}"
    );
}

/// A `NextToken` is a position within ONE enumeration: replayed against a request that asks
/// for something else, it is refused rather than silently skipping or repeating datapoints.
#[tokio::test]
async fn a_next_token_is_bound_to_the_request_it_was_minted_for() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        "Bound",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Count",
        base,
        &[(0.0, 1.0), (60.0, 2.0), (120.0, 4.0)],
    )
    .await;

    let first_page = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [metric_stat_query("m1", "Bound", 60, "Sum")],
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
            "ScanBy": "TimestampAscending",
            "MaxDatapoints": 1,
        }),
    )
    .await;
    let token = first_page
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a cut page mints a NextToken, got {first_page}"))
        .to_string();

    // (case, what the continuation changes about the request)
    let changed: [(&str, Value); 4] = [
        (
            "another statistic",
            json!({
                "MetricDataQueries": [metric_stat_query("m1", "Bound", 60, "Average")],
                "StartTime": base,
                "EndTime": base + 3.0 * MINUTE,
                "ScanBy": "TimestampAscending",
                "MaxDatapoints": 1,
                "NextToken": token,
            }),
        ),
        (
            "another range",
            json!({
                "MetricDataQueries": [metric_stat_query("m1", "Bound", 60, "Sum")],
                "StartTime": base,
                "EndTime": base + 2.0 * MINUTE,
                "ScanBy": "TimestampAscending",
                "MaxDatapoints": 1,
                "NextToken": token,
            }),
        ),
        (
            "another order",
            json!({
                "MetricDataQueries": [metric_stat_query("m1", "Bound", 60, "Sum")],
                "StartTime": base,
                "EndTime": base + 3.0 * MINUTE,
                "ScanBy": "TimestampDescending",
                "MaxDatapoints": 1,
                "NextToken": token,
            }),
        ),
        (
            "another query beside it",
            json!({
                "MetricDataQueries": [
                    metric_stat_query("m1", "Bound", 60, "Sum"),
                    metric_stat_query("m2", "Bound", 60, "Sum"),
                ],
                "StartTime": base,
                "EndTime": base + 3.0 * MINUTE,
                "ScanBy": "TimestampAscending",
                "MaxDatapoints": 1,
                "NextToken": token,
            }),
        ),
    ];
    for (case, request) in changed {
        let (status, body) = call(&router, "GetMetricData", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNextToken",
            "case {case:?}: expected InvalidNextToken, got {body}"
        );
        // The refusal carries an EMPTY message, as the recorded one does: neither the token
        // nor why it was rejected is echoed back.
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(""),
            "case {case:?}: expected an empty message, got {body}"
        );
    }

    // The same request the token was minted for continues where it left off.
    let second_page = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [metric_stat_query("m1", "Bound", 60, "Sum")],
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
            "ScanBy": "TimestampAscending",
            "MaxDatapoints": 1,
            "NextToken": token,
        }),
    )
    .await;
    assert_eq!(
        pairs(&single_result(&second_page)),
        vec![(base + MINUTE, 2.0)],
        "case the request the token belongs to: expected the second datapoint, got {second_page}"
    );
}

/// Every statistic CloudWatch defines is COMPUTED from the window's own distribution rather
/// than refused — the interquartile mean and the trimmed / Winsorized / trimmed-count /
/// percentile-rank families, in both their bare and their spanned form — while a `Stat`
/// outside the statistic grammar altogether is still a bad parameter value.
///
/// Source for the families and their definitions: the "CloudWatch statistics definitions" page
/// the model's `MetricStreamStatisticsConfiguration$AdditionalStatistics` points at, which this
/// crate's statistic grammar (`crate::state::is_cloudwatch_statistic`) is written from. The
/// rows span the families, both forms and both kinds of bound (a percentile of the
/// distribution and a plain metric value), and the window's two published values make each
/// definition's arithmetic checkable by hand.
#[tokio::test]
async fn every_defined_statistic_is_computed_from_the_windows_distribution() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        "Latency",
        &json!([{"Name": "Service", "Value": "api"}]),
        "Milliseconds",
        base,
        &[(0.0, 10.0), (30.0, 30.0)],
    )
    .await;

    // (the statistic, the value its definition gives over the window's values 10 and 30)
    let cases: Vec<(&str, f64)> = vec![
        // "Interquartile mean. The trimmed mean of the interquartile range" — the middle half.
        ("IQM", 20.0),
        // "The mean of all values that are between two specified boundaries": the lowest 10%
        // and the highest 10% of the distribution's weight are removed first.
        ("TM(10%:90%)", 20.0),
        // The bare form trims from the top only, 2% of the weight of the larger value.
        ("tm98", (10.0 + 30.0 * 0.96) / 1.96),
        // "Values outside of the boundaries are replaced with the values of the boundaries,
        // then the mean is computed" — so every observation still counts.
        ("wm90", 20.0),
        ("WM(0%:100%)", 20.0),
        // "The number of data points in the chosen range", and the sum of their values.
        ("TC(0%:100%)", 2.0),
        ("ts(0%:100%)", 40.0),
        // A bound without a `%` is a metric VALUE rather than a percentile.
        ("TM(20:)", 30.0),
        ("TS(:20)", 10.0),
        // "Percentile rank. The percentage of values that fall within the specified range."
        ("PR(:300)", 100.0),
        ("PR(:20)", 50.0),
    ];
    for (stat, expected) in cases {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [metric_stat_query("m1", "Latency", 60, stat)],
                "StartTime": base,
                "EndTime": base + MINUTE,
            }),
        )
        .await;
        let result = single_result(&body);
        let computed = pairs(&result);
        assert_eq!(
            computed.len(),
            1,
            "case stat {stat}: expected the window's one value, got {result}"
        );
        assert!(
            (computed[0].1 - expected).abs() < 1e-9,
            "case stat {stat}: expected {expected}, got {:?}",
            computed[0].1
        );
    }

    // A name no statistic family produces is refused as the bad value it is.
    for stat in ["Median", "p101", "tm110", "TM(:)", "PR(300)", "average!"] {
        let (status, body) = call(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [metric_stat_query("m1", "Latency", 60, stat)],
                "StartTime": base,
                "EndTime": base + MINUTE,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case stat {stat}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case stat {stat}: expected InvalidParameterValueException, got {body}"
        );
    }

    // The statistics folded out of the stored aggregates still answer their windows, so the
    // rows above are the grammar and not a blanket acceptance.
    for stat in ["Sum", "Average", "Minimum", "Maximum", "SampleCount", "p90"] {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [metric_stat_query("m1", "Latency", 60, stat)],
                "StartTime": base,
                "EndTime": base + MINUTE,
            }),
        )
        .await;
        let result = single_result(&body);
        assert_eq!(
            pairs(&result).len(),
            1,
            "case stat {stat}: expected the window's value, got {result}"
        );
    }
}

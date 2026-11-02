// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the metric math an `Expression` query carries: every value is
//! computed from the datapoints `PutMetricData` stored, over the queries of the same request.
//!
//! The suite drives the service over the wire it speaks (`awsJson1_0`, `POST /` with
//! `X-Amz-Target`), so a result that is not computed from the shared store cannot pass.
//!
//! The tables span the classes the rules generalise over — which operator or function the
//! expression applies, whether it reads one series or an array of them, whether a point's
//! value is a number at all, and how a search names the series it discovers — so any one
//! recorded expression is a single row.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, ok, wait_for_evaluation};

/// One minute, in the epoch seconds every published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series in this suite is published under.
const NAMESPACE: &str = "MyApp/MetricMath";

/// How close two computed values have to be to count as the same number.
const TOLERANCE: f64 = 1e-9;

/// A minute-aligned instant ten minutes in the past: inside CloudWatch's own retention, and
/// aligned so that `GetMetricDataInput$StartTime`'s documented rounding ("Start time less than
/// 15 days ago - Round down to the nearest whole minute") leaves it where the request put it.
fn base_time() -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now / MINUTE).floor() * MINUTE - 10.0 * MINUTE
}

/// Publish `(offset in minutes from `base`, value)` into one series of [`NAMESPACE`].
async fn publish(
    router: &smithy_server_runtime::MultiServiceRouter,
    metric: &str,
    dimensions: &Value,
    base: f64,
    values: &[f64],
) {
    let data: Vec<Value> = values
        .iter()
        .enumerate()
        .map(|(minute, value)| {
            #[allow(clippy::cast_precision_loss)]
            let offset = minute as f64 * MINUTE;
            json!({
                "MetricName": metric,
                "Dimensions": dimensions,
                "Value": value,
                "Unit": "Count",
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

/// The two series every expression below reads: `Requests` on two hosts, and `Errors` on one.
async fn publish_fixtures(router: &smithy_server_runtime::MultiServiceRouter, base: f64) {
    publish(
        router,
        "Requests",
        &json!([{"Name": "Service", "Value": "api"}, {"Name": "Host", "Value": "a"}]),
        base,
        &[10.0, 20.0, 30.0],
    )
    .await;
    publish(
        router,
        "Requests",
        &json!([{"Name": "Service", "Value": "api"}, {"Name": "Host", "Value": "b"}]),
        base,
        &[1.0, 2.0, 3.0],
    )
    .await;
    publish(
        router,
        "Errors",
        &json!([{"Name": "Service", "Value": "api"}, {"Name": "Host", "Value": "a"}]),
        base,
        &[1.0, 0.0, 4.0],
    )
    .await;
}

/// The `Requests` series of host `a` under the id `m1`, and the `Errors` series of the same
/// host under `m2`, neither returned as data of its own.
fn source_queries() -> Vec<Value> {
    vec![
        retrieving_query("m1", "Requests", "a"),
        retrieving_query("m2", "Errors", "a"),
    ]
}

/// One `MetricStat` query the expressions refer to, whose own data is not returned.
fn retrieving_query(id: &str, metric: &str, host: &str) -> Value {
    json!({
        "Id": id,
        "ReturnData": false,
        "MetricStat": {
            "Metric": {
                "Namespace": NAMESPACE,
                "MetricName": metric,
                "Dimensions": [
                    {"Name": "Service", "Value": "api"},
                    {"Name": "Host", "Value": host},
                ],
            },
            "Period": 60,
            "Stat": "Sum",
        },
    })
}

/// Read `expression` over the fixtures, oldest datapoint first.
async fn evaluate(
    router: &smithy_server_runtime::MultiServiceRouter,
    base: f64,
    expression: &str,
) -> Value {
    let mut queries = source_queries();
    queries.push(json!({"Id": "e1", "Expression": expression, "Period": 60}));
    ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": queries,
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await
}

/// The `(timestamp, value)` pairs one result carries, in the order the response listed them.
fn pairs(result: &Value) -> Vec<(f64, f64)> {
    let timestamps: Vec<f64> = numbers(result, "Timestamps");
    let values: Vec<f64> = numbers(result, "Values");
    assert_eq!(
        timestamps.len(),
        values.len(),
        "a result's Timestamps and Values stay in step, got {result}"
    );
    timestamps.into_iter().zip(values).collect()
}

/// One numeric list member of a result.
fn numbers(result: &Value, member: &str) -> Vec<f64> {
    result
        .get(member)
        .and_then(Value::as_array)
        .map(|listed| listed.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// The results of one response, in the order it listed them.
fn results(body: &Value) -> Vec<Value> {
    body.get("MetricDataResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("GetMetricData returns MetricDataResults, got {body}"))
}

/// The one result of a response that returns data for a single query.
fn single_result(body: &Value) -> Value {
    let listed = results(body);
    assert_eq!(
        listed.len(),
        1,
        "one returned query answers one result, got {body}"
    );
    listed[0].clone()
}

/// The values of a response's single result.
fn single_values(body: &Value) -> Vec<f64> {
    pairs(&single_result(body))
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

/// Assert two value lists are the same numbers, naming the case and what was computed.
fn assert_values(case: &str, computed: &[f64], expected: &[f64]) {
    assert_eq!(
        computed.len(),
        expected.len(),
        "case {case:?}: expected {expected:?}, got {computed:?}"
    );
    for (left, right) in computed.iter().zip(expected) {
        assert!(
            (left - right).abs() < TOLERANCE,
            "case {case:?}: expected {expected:?}, got {computed:?}"
        );
    }
}

/// Every operator and every function is applied to the SAME two stored series, so each row
/// differs only in what the expression asks for — the operator, the arity, whether the
/// operand is one series or an array of them, and whether the value folds to a number.
#[tokio::test]
async fn an_expression_is_computed_over_the_queries_it_refers_to() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    // m1 is [10, 20, 30] and m2 is [1, 0, 4], one point per minute.
    let cases: Vec<(&str, Vec<f64>)> = vec![
        ("m1", vec![10.0, 20.0, 30.0]),
        ("m1 + m2", vec![11.0, 20.0, 34.0]),
        ("m1 - m2", vec![9.0, 20.0, 26.0]),
        ("m1 * 2", vec![20.0, 40.0, 60.0]),
        ("2 * m1", vec![20.0, 40.0, 60.0]),
        ("m1 / 10", vec![1.0, 2.0, 3.0]),
        ("m2 ^ 2", vec![1.0, 0.0, 16.0]),
        ("-m1", vec![-10.0, -20.0, -30.0]),
        ("(m1 + m2) * 2", vec![22.0, 40.0, 68.0]),
        // A comparison is 1 where it holds and 0 where it does not, which is what makes it an
        // operand of IF and of arithmetic.
        ("m1 > 15", vec![0.0, 1.0, 1.0]),
        ("m1 >= 20 AND m2 > 0", vec![0.0, 0.0, 1.0]),
        ("IF(m1 > 15, m1, 0)", vec![0.0, 20.0, 30.0]),
        ("IF(m2, m1, -1)", vec![10.0, -1.0, 30.0]),
        // The one-point functions: each point on its own.
        ("ABS(-m1)", vec![10.0, 20.0, 30.0]),
        ("FLOOR(m1 / 7)", vec![1.0, 2.0, 4.0]),
        ("CEIL(m1 / 7)", vec![2.0, 3.0, 5.0]),
        ("SQRT(m2)", vec![1.0, 0.0, 2.0]),
        (
            "LOG10(m1)",
            vec![1.0, 20.0_f64.log10(), 30.0_f64.log10()],
        ),
        ("LOG(m1)", vec![10.0_f64.ln(), 20.0_f64.ln(), 30.0_f64.ln()]),
        // The functions whose value at a point depends on the point before it, so the first
        // point of the series has none.
        ("DIFF(m1)", vec![10.0, 10.0]),
        ("RATE(m1)", vec![10.0 / MINUTE, 10.0 / MINUTE]),
        ("DIFF_TIME(m1)", vec![MINUTE, MINUTE]),
        ("RUNNING_SUM(m1)", vec![10.0, 30.0, 60.0]),
        // An array of series folds to the series of that statistic at each time stamp.
        ("SUM([m1,m2])", vec![11.0, 20.0, 34.0]),
        ("AVG([m1,m2])", vec![5.5, 10.0, 17.0]),
        ("MIN([m1,m2])", vec![1.0, 0.0, 4.0]),
        ("MAX([m1,m2])", vec![10.0, 20.0, 30.0]),
        ("DATAPOINT_COUNT([m1,m2])", vec![2.0, 2.0, 2.0]),
        // ONE series folds to a single number, which another expression reads as a term.
        ("TIME_SERIES(SUM(m1))", vec![60.0, 60.0, 60.0]),
        ("TIME_SERIES(AVG(m1))", vec![20.0, 20.0, 20.0]),
        ("TIME_SERIES(MIN(m2))", vec![0.0, 0.0, 0.0]),
        ("TIME_SERIES(MAX(m2))", vec![4.0, 4.0, 4.0]),
        ("m1 / SUM(m1) * 100", vec![100.0 / 6.0, 100.0 / 3.0, 50.0]),
        ("TIME_SERIES(METRIC_COUNT([m1,m2]))", vec![2.0, 2.0, 2.0]),
        ("TIME_SERIES(DATAPOINT_COUNT(m1))", vec![3.0, 3.0, 3.0]),
        ("TIME_SERIES(PERIOD(m1))", vec![MINUTE, MINUTE, MINUTE]),
        ("STDDEV([m1,m2])", vec![4.5, 10.0, 13.0]),
        // A constant series carries the number at every period of the range.
        ("TIME_SERIES(7)", vec![7.0, 7.0, 7.0]),
        // Everything the request retrieves, summed per time stamp.
        ("SUM(METRICS())", vec![11.0, 20.0, 34.0]),
        ("SUM(METRICS('Errors'))", vec![1.0, 0.0, 4.0]),
    ];

    for (expression, expected) in cases {
        let body = evaluate(&router, base, expression).await;
        assert_values(expression, &single_values(&body), &expected);
    }
}

/// A point whose arithmetic produced no number is dropped, and the result says so — the other
/// points of the same series are still answered.
#[tokio::test]
async fn a_point_the_arithmetic_cannot_compute_is_dropped_and_reported() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    // m2's middle point is 0, so the middle quotient is not a number at all; the two points
    // either side of it are. Every row carries the SAME message: the sentence CloudWatch
    // reports a dropped point with is fixed text, interpolating nothing from the request.
    const DROPPED: &str =
        "One or more data-points have been dropped due to non-numeric values (NaN, -Infinite, \
         +Infinite)";
    for (expression, expected) in [
        ("m1 / m2", vec![10.0, 7.5]),
        ("LOG(m2)", vec![0.0, 4.0_f64.ln()]),
        ("SQRT(-m1)", vec![]),
    ] {
        let body = evaluate(&router, base, expression).await;
        let result = single_result(&body);
        assert_values(expression, &single_values(&body), &expected);
        let messages = result
            .get("Messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            messages,
            vec![json!({"Code": "ArithmeticError", "Value": DROPPED})],
            "case {expression:?}: expected exactly the arithmetic-error message, got {result}"
        );
    }

    // A series whose every point IS a number carries no such message.
    let body = evaluate(&router, base, "m1 + m2").await;
    let result = single_result(&body);
    assert_eq!(
        result.get("Messages"),
        Some(&json!([])),
        "case \"m1 + m2\": expected no message, got {result}"
    );
}

/// A search discovers its series in the store server-side: a schema matches the exact set of
/// dimension NAMES, while a dimension term matches a series carrying more dimensions than the
/// query named, and either feeds the same aggregation.
#[tokio::test]
async fn a_search_expression_discovers_series_and_feeds_an_aggregation() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    // `Requests` is published on two hosts, [10, 20, 30] and [1, 2, 3].
    let cases: Vec<(&str, Vec<f64>)> = vec![
        // The schema form: this namespace, and a series dimensioned by exactly Service+Host.
        (
            "SUM(SEARCH('{MyApp/MetricMath,Service,Host} MetricName=\"Requests\"', 'Sum', 60))",
            vec![11.0, 22.0, 33.0],
        ),
        (
            "AVG(SEARCH('{MyApp/MetricMath,Service,Host} MetricName=\"Requests\"', 'Sum', 60))",
            vec![5.5, 11.0, 16.5],
        ),
        // A PARTIAL dimension match: the query names one of the two dimensions the series
        // carry, and matches them anyway.
        (
            "SUM(SEARCH('Namespace=\"MyApp/MetricMath\" MetricName=\"Requests\" Service=\"api\"', 'Sum', 60))",
            vec![11.0, 22.0, 33.0],
        ),
        (
            "AVG(SEARCH('MetricName=\"Requests\" Host=\"b\"', 'Sum', 60))",
            vec![1.0, 2.0, 3.0],
        ),
        // A free term matches anywhere in the series' identity.
        (
            "SUM(SEARCH('Errors', 'Sum', 60))",
            vec![1.0, 0.0, 4.0],
        ),
        // `NOT` excludes what its term matches, and `OR` widens the match.
        (
            "SUM(SEARCH('MetricName=\"Requests\" NOT Host=\"b\"', 'Sum', 60))",
            vec![10.0, 20.0, 30.0],
        ),
        (
            "SUM(SEARCH('Namespace=\"MyApp/MetricMath\" NOT MetricName=\"Errors\"', 'Sum', 60))",
            vec![11.0, 22.0, 33.0],
        ),
        (
            "SUM(SEARCH('MetricName=\"Errors\" OR Host=\"b\"', 'Sum', 60))",
            vec![2.0, 2.0, 7.0],
        ),
        // Every series of the namespace, whatever its metric name.
        (
            "SUM(SEARCH('Namespace=\"MyApp/MetricMath\"', 'Sum', 60))",
            vec![12.0, 22.0, 37.0],
        ),
        // The statistic the search folds by is the search's own.
        (
            "MAX(SEARCH('MetricName=\"Requests\"', 'Maximum', 60))",
            vec![10.0, 20.0, 30.0],
        ),
    ];
    for (expression, expected) in cases {
        let body = evaluate(&router, base, expression).await;
        assert_values(expression, &single_values(&body), &expected);
    }

    // A bare search reports EVERY series it matched, each as its own result under the query's
    // id — which is why an alarm cannot watch one.
    let body = evaluate(
        &router,
        base,
        "SEARCH('{MyApp/MetricMath,Service,Host} MetricName=\"Requests\"', 'Sum', 60)",
    )
    .await;
    let listed = results(&body);
    assert_eq!(
        listed.len(),
        2,
        "case a bare search: expected one result per matched series, got {body}"
    );
    for result in &listed {
        assert_eq!(
            result.get("Id").and_then(Value::as_str),
            Some("e1"),
            "case a bare search: every result is reported under the query's id, got {result}"
        );
    }
    let mut labels: Vec<String> = listed
        .iter()
        .map(|result| {
            result
                .get("Label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    labels.sort();
    assert_eq!(
        labels,
        vec!["Requests a api".to_string(), "Requests b api".to_string()],
        "case a bare search: each series is labelled by its own dimensions, got {body}"
    );

    // A schema fixes the EXACT set of dimension names, so a schema naming only one of the two
    // the series carry matches nothing — and the query is still answered, with no points.
    let body = evaluate(
        &router,
        base,
        "SEARCH('{MyApp/MetricMath,Service} MetricName=\"Requests\"', 'Sum', 60)",
    )
    .await;
    assert_values("a schema no series matches", &single_values(&body), &[]);
}

/// An expression this service cannot evaluate is refused in CloudWatch's own words, and
/// nothing about the request is answered with a series nothing computed.
#[tokio::test]
async fn an_expression_the_service_cannot_evaluate_is_refused() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    // Each row is semantically or syntactically not an expression this service evaluates: an
    // id no query defines, a function nothing computes, a call whose operands are not what it
    // takes, and text that is not an expression at all.
    for expression in [
        "m9 + 1",
        "SUM(m9)",
        "SUM(m1",
        "m1 +",
        "SUM()",
        "SUM(m1, m2)",
        "NOPE(m1)",
        "SUM(5)",
        "'text' + 1",
        "IF(m1 > 1, m1)",
        "[m1,m2] + [m1,m2]",
        "m1 @ m2",
    ] {
        let mut queries = source_queries();
        queries.push(json!({"Id": "e1", "Expression": expression}));
        let (status, body) = call(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": queries,
                "StartTime": base,
                "EndTime": base + 3.0 * MINUTE,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {expression:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {expression:?}: expected the request-validation class, got {body}"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.starts_with("Error in expression 'e1': "),
            "case {expression:?}: expected the refusal to name the query, got {body}"
        );
    }

    // An expression that refers to itself is refused rather than followed.
    let (status, body) = call(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [{"Id": "e1", "Expression": "e1 + 1"}],
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a self-referring expression: expected HTTP 400, got {status} with {body}"
    );

    // An expression whose value is ONE number is a term another expression reads, not a
    // result: asking for it as data is refused, while feeding it to one that does return a
    // series is answered.
    for expression in ["SUM(m1)", "AVG(m1) + 1", "METRIC_COUNT([m1,m2])", "PERIOD(m1)"] {
        let mut queries = source_queries();
        queries.push(json!({"Id": "e1", "Expression": expression, "Period": 60}));
        let (status, body) = call(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": queries,
                "StartTime": base,
                "EndTime": base + 3.0 * MINUTE,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {expression:?} returned as data: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {expression:?} returned as data: expected the request-validation class, got \
             {body}"
        );

        let mut queries = source_queries();
        queries.push(json!({"Id": "e1", "Expression": expression, "ReturnData": false, "Period": 60}));
        queries.push(json!({"Id": "e2", "Expression": "TIME_SERIES(e1)", "Period": 60}));
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": queries,
                "StartTime": base,
                "EndTime": base + 3.0 * MINUTE,
                "ScanBy": "TimestampAscending",
            }),
        )
        .await;
        assert_eq!(
            single_values(&body).len(),
            3,
            "case {expression:?} as a term: expected the number at every period, got {body}"
        );
    }

    // And a query that names both an expression and a metric, or neither, is refused for the
    // SHAPE of the request it is — the request-validation class, which
    // `crate::metric_query_ids` tables for the ids of the same array.
    for (query, expected) in [
        (
            json!({"Id": "m1", "Expression": "1", "MetricStat": {
                "Metric": {"Namespace": NAMESPACE, "MetricName": "Requests"},
                "Period": 60,
                "Stat": "Sum",
            }}),
            "ValidationException",
        ),
        (json!({"Id": "m1"}), "ValidationException"),
    ] {
        let (status, body) = call(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [query],
                "StartTime": base,
                "EndTime": base + 3.0 * MINUTE,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {query}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {query}: expected {expected}, got {body}"
        );
    }
}

/// An expression may refer to another expression's result, and the chain is resolved whatever
/// order the queries are listed in.
#[tokio::test]
async fn an_expression_may_refer_to_another_expressions_result() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    let body = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [
                // The dependent expression is listed BEFORE the one it refers to.
                {"Id": "e2", "Expression": "e1 * 10", "Period": 60},
                {"Id": "e1", "Expression": "m1 + m2", "ReturnData": false, "Period": 60},
                retrieving_query("m1", "Requests", "a"),
                retrieving_query("m2", "Errors", "a"),
            ],
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    assert_values(
        "an expression over another expression",
        &single_values(&body),
        &[110.0, 200.0, 340.0],
    );
}

/// An anomaly detection band is the range an anomaly DETECTOR's model expects the metric in,
/// so a metric no detector was created for has no band: the two bounds are still reported,
/// under their own labels and with no points.
#[tokio::test]
async fn an_anomaly_detection_band_reports_its_two_bounds() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    for width in ["", ", 2", ", 3"] {
        let expression = format!("ANOMALY_DETECTION_BAND(m1{width})");
        let body = evaluate(&router, base, &expression).await;
        let listed = results(&body);
        assert_eq!(
            listed.len(),
            2,
            "case {expression:?}: expected the two bounds of the band, got {body}"
        );
        let mut labels: Vec<String> = listed
            .iter()
            .map(|result| {
                result
                    .get("Label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        labels.sort();
        assert_eq!(
            labels,
            vec!["Requests High".to_string(), "Requests Low".to_string()],
            "case {expression:?}: expected the Low/High pair of the metric, got {body}"
        );
        for result in &listed {
            assert_values(&expression, &pairs(result).iter().map(|(_, v)| *v).collect::<Vec<f64>>(), &[]);
        }
    }

    // A band width that is not a width at all is refused rather than answered.
    let mut queries = source_queries();
    queries.push(json!({"Id": "e1", "Expression": "ANOMALY_DETECTION_BAND(m1, 0)"}));
    let (status, body) = call(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": queries,
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a zero band width: expected HTTP 400, got {status} with {body}"
    );

    // The band's points come from the model an anomaly DETECTOR trained, so the bounds above
    // carry none: no detector was created for the metric. A detector for THIS metric and
    // statistic is what gives the band its points.
    let detector = json!({
        "SingleMetricAnomalyDetector": {
            "Namespace": NAMESPACE,
            "MetricName": "Requests",
            "Dimensions": [
                {"Name": "Service", "Value": "api"},
                {"Name": "Host", "Value": "a"},
            ],
            "Stat": "Sum",
        },
    });
    ok(&router, "PutAnomalyDetector", &detector).await;

    // A model still being trained says nothing about the metric yet, so the band is still
    // reported with no points.
    let pending = evaluate(&router, base, "ANOMALY_DETECTION_BAND(m1, 2)").await;
    for result in &results(&pending) {
        assert_values(
            "a model still being trained",
            &pairs(result).iter().map(|(_, value)| *value).collect::<Vec<f64>>(),
            &[],
        );
    }

    // Once the training the write started comes due, the band is drawn from what the metric's
    // own history says is normal: the mean of the values the model was trained on, and the
    // requested number of their deviations either side of it.
    wait_for_evaluation();
    let trained = [10.0_f64, 20.0, 30.0];
    #[allow(clippy::cast_precision_loss)]
    let count = trained.len() as f64;
    let mean = trained.iter().sum::<f64>() / count;
    let deviation = (trained.iter().map(|value| (value - mean) * (value - mean)).sum::<f64>()
        / count)
        .sqrt();
    // Rows differ along the band width, which is counted in those deviations: a wider band
    // reaches further either side of the same expectation.
    for (case, width) in [("the default width", 2.0_f64), ("a width of 3", 3.0)] {
        let expression = if (width - 2.0).abs() < TOLERANCE {
            "ANOMALY_DETECTION_BAND(m1)".to_string()
        } else {
            format!("ANOMALY_DETECTION_BAND(m1, {width})")
        };
        let body = evaluate(&router, base, &expression).await;
        let listed = results(&body);
        for result in &listed {
            let label = result
                .get("Label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let bound = if label.ends_with("High") {
                mean + width * deviation
            } else {
                mean - width * deviation
            };
            assert_values(
                &format!("{case}: {label}"),
                &pairs(result).iter().map(|(_, value)| *value).collect::<Vec<f64>>(),
                &[bound, bound, bound],
            );
        }
    }

    // A band over a series no detector was created for still has no points: the model is the
    // DETECTOR's, and the other host's series has none.
    let mut queries = source_queries();
    queries.push(retrieving_query("m3", "Requests", "b"));
    queries.push(json!({
        "Id": "e1",
        "Expression": "ANOMALY_DETECTION_BAND(m3, 2)",
        "Period": 60,
    }));
    let body = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": queries,
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    for result in &results(&body) {
        assert_values(
            "another host's series",
            &pairs(result).iter().map(|(_, value)| *value).collect::<Vec<f64>>(),
            &[],
        );
    }
}

/// An expression that asks for more than this service evaluates is REFUSED rather than
/// recursed into or allocated for — its length, its nesting and how many series its value may
/// hold are all bounded — while an expression that nests only a little is still evaluated.
#[tokio::test]
async fn an_oversized_expression_is_refused_rather_than_evaluated() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    // Each row is an expression built to cross one of the bounds: the nesting of a
    // parenthesised group, of a prefix chain, of a right-associative chain and of an array
    // literal, the number of series an array may hold, and the expression's own length.
    let cases: Vec<(&str, String)> = vec![
        (
            "a deeply parenthesised group",
            format!("{}m1{}", "(".repeat(200), ")".repeat(200)),
        ),
        ("a long prefix chain", format!("{}m1", "-".repeat(200))),
        (
            "a long right-associative chain",
            format!("1{}", "^1".repeat(200)),
        ),
        (
            "a deeply nested array",
            format!("SUM({}m1{})", "[".repeat(100), "]".repeat(100)),
        ),
        (
            "more series than an array may hold",
            format!("SUM([{}m1])", "m1,".repeat(600)),
        ),
        (
            "an expression longer than the member allows",
            format!("m1{}", "+m1".repeat(800)),
        ),
    ];
    for (case, expression) in cases {
        let mut queries = source_queries();
        queries.push(json!({"Id": "e1", "Expression": expression, "Period": 60}));
        let (status, body) = call(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": queries,
                "StartTime": base,
                "EndTime": base + 3.0 * MINUTE,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        // Both refusals — this service's own and the one the MODEL states
        // (`MetricExpression` is at most 2048 characters, refused by the boundary validator
        // before the handler runs) — are the one request-validation class.
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected a 400 refusal of the request-validation class, got {body}"
        );
    }

    // The same shapes inside the bounds are still evaluated, so the rows above are the bounds
    // and not a blanket refusal of a nested expression.
    for (case, expression, expected) in [
        ("a parenthesised group", "((m1 + m2)) * 2", vec![22.0, 40.0, 68.0]),
        ("a short prefix chain", "--m1", vec![10.0, 20.0, 30.0]),
        ("a short array", "SUM([m1,m1,m2])", vec![21.0, 40.0, 64.0]),
    ] {
        let body = evaluate(&router, base, expression).await;
        assert_values(case, &single_values(&body), &expected);
    }
}
#[tokio::test]
async fn an_array_of_series_is_ordered_and_cut_by_the_array_functions() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    // `Requests` is published on host a ([10, 20, 30]) and host b ([1, 2, 3]).
    let search = "SEARCH('MetricName=\"Requests\"', 'Sum', 60)";
    let cases: Vec<(String, Vec<f64>)> = vec![
        // The leading series by one statistic, descending and ascending.
        (
            format!("FIRST(SORT({search}, SUM, DESC))"),
            vec![10.0, 20.0, 30.0],
        ),
        (
            format!("FIRST(SORT({search}, MAX, ASC))"),
            vec![1.0, 2.0, 3.0],
        ),
        // The count cuts the array down before the aggregation reads it.
        (
            format!("SUM(SORT({search}, AVG, DESC, 1))"),
            vec![10.0, 20.0, 30.0],
        ),
        // A slice of the ordered array.
        (
            format!("SUM(SLICE(SORT({search}, SUM, DESC), 1, 2))"),
            vec![1.0, 2.0, 3.0],
        ),
        (
            format!("TIME_SERIES(METRIC_COUNT({search}))"),
            vec![2.0, 2.0, 2.0],
        ),
        (
            format!("TIME_SERIES(METRIC_COUNT(SORT({search}, SUM, DESC, 1)))"),
            vec![1.0, 1.0, 1.0],
        ),
    ];
    for (expression, expected) in cases {
        let body = evaluate(&router, base, &expression).await;
        assert_values(&expression, &single_values(&body), &expected);
    }
}

/// `ReturnData` decides which queries are REPORTED, not which are computed: the expression
/// still reads the series the suppressed queries retrieved.
#[tokio::test]
async fn a_suppressed_query_is_still_read_by_the_expression() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;

    let body = evaluate(&router, base, "m1 + m2").await;
    let listed = results(&body);
    assert_eq!(
        listed.len(),
        1,
        "case two suppressed queries: only the expression is reported, got {body}"
    );
    assert_eq!(
        listed[0].get("Id").and_then(Value::as_str),
        Some("e1"),
        "case two suppressed queries: the reported result is the expression's, got {body}"
    );
    assert_eq!(
        listed[0].get("Label").and_then(Value::as_str),
        Some("e1"),
        "case an expression without a Label: the default label is the query's own Id, got {body}"
    );

    // The same request with the metrics returned reports all three, in request order.
    let body = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [
                {"Id": "m1", "MetricStat": {
                    "Metric": {
                        "Namespace": NAMESPACE,
                        "MetricName": "Requests",
                        "Dimensions": [
                            {"Name": "Service", "Value": "api"},
                            {"Name": "Host", "Value": "a"},
                        ],
                    },
                    "Period": 60,
                    "Stat": "Sum",
                }},
                {"Id": "e1", "Expression": "m1 * 2", "Label": "twice", "Period": 60},
            ],
            "StartTime": base,
            "EndTime": base + 3.0 * MINUTE,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    let listed = results(&body);
    assert_eq!(
        listed
            .iter()
            .map(|result| result.get("Id").and_then(Value::as_str).unwrap_or_default())
            .collect::<Vec<&str>>(),
        vec!["m1", "e1"],
        "case both returned: the results are in request order, got {body}"
    );
    assert_eq!(
        listed[1].get("Label").and_then(Value::as_str),
        Some("twice"),
        "case a labelled expression: the caller's label is reported, got {body}"
    );
}

/// `FILL` gives the window's empty periods a value, which is what makes a series with holes
/// comparable with one without.
#[tokio::test]
async fn fill_gives_an_empty_period_a_value() {
    let router = build_router();
    let base = base_time();
    publish_fixtures(&router, base).await;
    // A fourth minute is published for `Errors` only, so `Requests` has a hole where the
    // expression reads both.
    publish(
        &router,
        "Errors",
        &json!([{"Name": "Service", "Value": "api"}, {"Name": "Host", "Value": "a"}]),
        base + 3.0 * MINUTE,
        &[5.0],
    )
    .await;

    for (expression, expected) in [
        ("FILL(m1, 0)", vec![10.0, 20.0, 30.0, 0.0]),
        ("FILL(m1, REPEAT)", vec![10.0, 20.0, 30.0, 30.0]),
        ("FILL(m1, 0) + m2", vec![11.0, 20.0, 34.0, 5.0]),
    ] {
        let mut queries = source_queries();
        queries.push(json!({"Id": "e1", "Expression": expression, "Period": 60}));
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": queries,
                "StartTime": base,
                "EndTime": base + 4.0 * MINUTE,
                "ScanBy": "TimestampAscending",
            }),
        )
        .await;
        assert_values(expression, &single_values(&body), &expected);
    }
}

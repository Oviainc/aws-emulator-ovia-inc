// SPDX-License-Identifier: CC0-1.0

//! The identity and the arithmetic of the metric READ path, end to end over the wire: what a
//! datapoint is identified by, what an expression computes from two of them, and what a result
//! is LABELLED with.
//!
//! Four rules, each asserted over a table whose rows differ along what the rule generalises
//! over:
//!
//! 1. **A binary operator over two series yields a value at every instant EITHER series has a
//!    value at, taking an operand with no datapoint there as 0** — so a summed or rate
//!    expression keeps the periods in which one series is idle, a sum over a metric nothing has
//!    ever written is the other operand's own series, and a PRODUCT over such a period is zero
//!    rather than the present operand's value or no datapoint at all. A period where that
//!    leaves the operator no finite number (a quotient by the absent side) carries no
//!    datapoint. The rows differ in the operator, in which operand is the absent one and in
//!    how the two series overlap.
//! 2. **A `MetricStat` that names no `Unit` resolves to ONE unit's series — the alphabetically
//!    first unit name the range holds — and never to the sum across units**, because a
//!    datapoint's unit is part of its read identity ("If you specify a unit, the operation
//!    returns only data that was collected with that unit specified.",
//!    `smithy_query(query="doc",shape="GetMetricStatisticsInput",member="Unit")`). The rows
//!    differ in how many units the instant holds and in which of them is first.
//! 3. **A dynamic label is resolved by the SERVER from the data the result returns**: the
//!    `${…}` tokens of `MetricDataQuery$Label` are replaced with the statistics, the instants,
//!    the count and the metric properties of the series being reported, in the zone
//!    `LabelOptions$Timezone` names. The rows are one token each, including tokens no report of
//!    this behaviour cites.
//! 4. **A window NO requested statistic is defined over is not a datapoint**: a percentile over
//!    an aggregate that kept no distribution has no value ("CloudWatch needs raw data points to
//!    calculate percentile statistics. If you publish data using a statistic set instead, you
//!    can only retrieve percentile statistics for this data if one of the following conditions
//!    is true: The SampleCount value of the statistic set is 1. The Min and the Max values of
//!    the statistic set are equal.",
//!    `smithy_query(query="doc",shape="GetMetricStatistics")`), so the read answers with no
//!    datapoint for that period rather than with an entry carrying only its instant and unit.
//!    The rows differ in the publication form, which decides whether the distribution survives.
//! 5. **An absent list member and an empty one read back the same way**: the protocols this
//!    service is served over cannot express the difference — on the query protocol neither
//!    carries a `<member>.member.N` key — so `DescribeAlarms` reports the `Dimensions` of an
//!    alarm written with none exactly as it reports the alarm written with an empty list. The
//!    rows differ in which list member is left out and in the form the other members take.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, ok};

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/DataPlane";

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// A minute-aligned instant ten minutes in the past: recent enough to sit inside every
/// retention tier, and aligned so the documented start-time rounding ("Start time less than 15
/// days ago - Round down to the nearest whole minute", `GetMetricDataInput$StartTime`) leaves a
/// request's start where it was put.
fn base_time() -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now / MINUTE).floor() * MINUTE - 10.0 * MINUTE
}

/// Publish one datum of `metric` at `at`, with `extra` merged over it.
async fn publish(router: &MultiServiceRouter, metric: &str, at: f64, extra: Value) {
    let mut datum = json!({"MetricName": metric, "Timestamp": at});
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        datum[member] = value;
    }
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": [datum]}),
    )
    .await;
}

/// One `MetricDataQuery` retrieving `metric` by `stat`, with its data not returned.
fn metric_query(id: &str, metric: &str, stat: &str) -> Value {
    json!({
        "Id": id,
        "ReturnData": false,
        "MetricStat": {
            "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
            "Period": 60,
            "Stat": stat,
        },
    })
}

/// The `(timestamps, values)` of the result `id`, oldest first.
async fn read_series(
    router: &MultiServiceRouter,
    queries: &[Value],
    base: f64,
    id: &str,
) -> (Vec<f64>, Vec<f64>) {
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": queries,
            "StartTime": base,
            "EndTime": base + 5.0 * MINUTE,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    let result = body["MetricDataResults"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|result| result.get("Id").and_then(Value::as_str) == Some(id))
        .cloned()
        .unwrap_or_else(|| panic!("no result {id} in {body}"));
    let numbers = |member: &str| -> Vec<f64> {
        result[member]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_f64)
            .collect()
    };
    (numbers("Timestamps"), numbers("Values"))
}

/// One row of the operand-fill table: the case, the expression, and the `(offset from the
/// base, value)` pairs the result must carry.
type FillRow = (&'static str, &'static str, Vec<(f64, f64)>);

#[tokio::test]
async fn an_operator_over_two_series_counts_an_absent_operand_as_zero() {
    let router = build_router();
    let base = base_time();

    // `left` holds a datapoint in the first and third minute, `right` only in the first: the
    // second minute has NEITHER (so no row reports a value for it) and the third has only the
    // left operand.
    publish(&router, "Left", base, json!({"Value": 10.0})).await;
    publish(&router, "Left", base + 2.0 * MINUTE, json!({"Value": 4.0})).await;
    publish(&router, "Right", base, json!({"Value": 3.0})).await;

    // (case, expression, the (offset from the base, value) pairs it must answer with). The
    // absent operand reads as 0 whatever the operator does with it, so every row answers both
    // instants either operand has data at — except where that leaves the operator no finite
    // number, which is no datapoint at all.
    let rows: Vec<FillRow> = vec![
        // The recorded case: a sum keeps the bucket only one operand has data in.
        ("a sum", "m1 + m2", vec![(0.0, 13.0), (2.0 * MINUTE, 4.0)]),
        // A difference fills the absent operand the same way, which is what makes the sum the
        // other operand's own value.
        ("a difference", "m1 - m2", vec![(0.0, 7.0), (2.0 * MINUTE, 4.0)]),
        (
            "the absent operand on the LEFT of a difference",
            "m2 - m1",
            vec![(0.0, -7.0), (2.0 * MINUTE, -4.0)],
        ),
        // Over a metric nothing has ever written, an additive expression is the other
        // operand's own series — the recorded `m1 + mghost` case.
        (
            "a sum with a never-written metric",
            "m1 + mghost",
            vec![(0.0, 10.0), (2.0 * MINUTE, 4.0)],
        ),
        // The recorded product: the bucket only one operand has data in is ZERO, which is
        // neither the present operand's value nor a missing datapoint.
        ("a product", "m1 * m2", vec![(0.0, 30.0), (2.0 * MINUTE, 0.0)]),
        (
            "a product with a never-written metric",
            "m1 * mghost",
            vec![(0.0, 0.0), (2.0 * MINUTE, 0.0)],
        ),
        // A comparison reads the absent operand as 0 too, and answers 1 or 0.
        ("a comparison", "m1 > m2", vec![(0.0, 1.0), (2.0 * MINUTE, 1.0)]),
        // Dividing BY the idle series leaves no finite number for that bucket, so the result
        // carries no datapoint there.
        ("a quotient", "m1 / m2", vec![(0.0, 10.0 / 3.0)]),
        // A sum feeding a division by a scalar keeps both instants: the fill is about the
        // operand, so it reaches whatever reads the filled series.
        (
            "a sum, then scaled",
            "(m1 + m2) / 2",
            vec![(0.0, 6.5), (2.0 * MINUTE, 2.0)],
        ),
    ];

    for (case, expression, expected) in rows {
        let queries = vec![
            metric_query("m1", "Left", "Sum"),
            metric_query("m2", "Right", "Sum"),
            metric_query("mghost", "NeverWritten", "Sum"),
            json!({"Id": "e1", "Expression": expression, "ReturnData": true}),
        ];
        let (timestamps, values) = read_series(&router, &queries, base, "e1").await;
        let answered: Vec<(f64, f64)> = timestamps
            .iter()
            .map(|at| at - base)
            .zip(values.iter().copied())
            .collect();
        assert_eq!(
            answered.len(),
            expected.len(),
            "case {case:?} ({expression}): expected {expected:?}, got {answered:?}"
        );
        for ((offset, value), (wanted_offset, wanted)) in answered.iter().zip(&expected) {
            assert!(
                (offset - wanted_offset).abs() < f64::EPSILON && (value - wanted).abs() < 1e-9,
                "case {case:?} ({expression}): expected {expected:?}, got {answered:?}"
            );
        }
    }
}

/// One row of the unit-resolution table: the case, the `(unit, value)` observations one
/// instant carries, the unit a query naming none resolves to, and that unit's `Sum`.
type UnitRow = (&'static str, Vec<(&'static str, f64)>, &'static str, f64);

#[tokio::test]
async fn a_query_naming_no_unit_resolves_to_one_units_series() {
    let router = build_router();
    let base = base_time();

    // The units are the model's own `StandardUnit` values
    // (`smithy_query(query="shape",shape="StandardUnit")`), including `Terabits`, which no
    // report of this behaviour cites.
    let rows: Vec<UnitRow> = vec![
        // The recorded shape: two units at one instant, and the read is the alphabetically
        // first one's series rather than the 107.0 cross-unit sum.
        ("Count beside Megabytes", vec![("Count", 7.0), ("Megabytes", 100.0)], "Count", 7.0),
        // The publish ORDER does not decide it: the same two observations the other way round
        // resolve to the same unit.
        ("the same two units published the other way round", vec![("Megabytes", 100.0), ("Count", 7.0)], "Count", 7.0),
        // Alphabetically first is not "the one that is not None": a unit before `Count` wins.
        ("Bytes beside Count", vec![("Bytes", 2.0), ("Count", 7.0)], "Bytes", 2.0),
        ("three units at one instant", vec![("Seconds", 1.0), ("Terabits", 5.0), ("Percent", 50.0)], "Percent", 50.0),
        // Several observations of the resolved unit fold together, and none of the others
        // reaches the answer.
        ("the resolved unit repeated", vec![("Count", 2.0), ("Count", 3.0), ("Percent", 90.0)], "Count", 5.0),
        // One unit is the whole series, so the resolution is that unit.
        ("one unit only", vec![("Terabits", 4.0)], "Terabits", 4.0),
    ];

    for (index, (case, published, unit, sum)) in rows.into_iter().enumerate() {
        let metric = format!("Unitful{index}");
        for (published_unit, value) in &published {
            publish(
                &router,
                &metric,
                base,
                json!({"Value": value, "Unit": published_unit}),
            )
            .await;
        }

        // A query naming NO unit: one series, the resolved unit's own.
        let unscoped = vec![json!({
            "Id": "m1",
            "MetricStat": {
                "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
                "Period": 60,
                "Stat": "Sum",
            },
        })];
        let (_, values) = read_series(&router, &unscoped, base, "m1").await;
        assert_eq!(
            values,
            vec![sum],
            "case {case:?}: a query naming no Unit must resolve to the {unit} series, got \
             {values:?}"
        );
        // Reading it again answers the same series: the resolution is a function of the data,
        // not of the call.
        let (_, again) = read_series(&router, &unscoped, base, "m1").await;
        assert_eq!(
            again, values,
            "case {case:?}: the resolved unit must be stable across calls, got {again:?}"
        );

        // Each unit the instant holds is readable on its own, which is what makes the answer
        // above ONE of them rather than all of them.
        for (published_unit, value) in &published {
            let scoped = vec![json!({
                "Id": "m1",
                "MetricStat": {
                    "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
                    "Period": 60,
                    "Stat": "Sum",
                    "Unit": published_unit,
                },
            })];
            let (_, scoped_values) = read_series(&router, &scoped, base, "m1").await;
            let expected: f64 = published
                .iter()
                .filter(|(held, _)| held == published_unit)
                .map(|(_, held)| held)
                .sum();
            assert_eq!(
                scoped_values,
                vec![expected],
                "case {case:?}: the {published_unit} series must read back as {expected} \
                 (published {value}), got {scoped_values:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_dynamic_label_is_resolved_from_the_data_the_result_returns() {
    let router = build_router();
    let base = base_time();
    let metric = "Labelled";
    // Three datapoints of one series, so every statistic a token names has a value of its own
    // and the instants differ.
    for (offset, value) in [(0.0, 2.0), (MINUTE, 8.0), (2.0 * MINUTE, 5.0)] {
        publish(
            &router,
            metric,
            base + offset,
            json!({"Value": value, "Unit": "Count"}),
        )
        .await;
    }

    let instant = |offset: f64| -> String {
        let stamp = base + offset;
        let whole = stamp.trunc();
        #[allow(clippy::cast_possible_truncation)]
        let secs = whole as i64;
        // The instants a label states are the modelled timestamp type's own RFC 3339
        // rendering, which is what the service writes them with.
        ::aws_smithy_types::DateTime::from_secs(secs)
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .expect("the instant renders")
    };

    // (case, the label the request sends, the label the response must carry). The tokens are
    // the published dynamic-label set (AWS CloudWatch's "Using dynamic labels"), including
    // `${AVG}`, `${FIRST}`, `${LAST}`, the `_TIME` tokens and `${PROP('…')}`, which no report
    // of this behaviour cites.
    let rows: Vec<(&str, String, String)> = vec![
        // The recorded label: every token of it resolved, and none of the `${…}` text left.
        (
            "the recorded label",
            "min ${MIN} max ${MAX} sum ${SUM} n ${DATAPOINT_COUNT} of ${LABEL}".to_string(),
            format!("min 2 max 8 sum 15 n 3 of {metric}"),
        ),
        ("the average", "${AVG}".to_string(), "5".to_string()),
        ("the first and last value", "${FIRST} -> ${LAST}".to_string(), "2 -> 5".to_string()),
        (
            "the instants the extremes occurred at",
            "${MIN_TIME} ${MAX_TIME}".to_string(),
            format!("{} {}", instant(0.0), instant(MINUTE)),
        ),
        (
            "the instants the series opens and closes at",
            "${FIRST_TIME} ${LAST_TIME}".to_string(),
            format!("{} {}", instant(0.0), instant(2.0 * MINUTE)),
        ),
        (
            "the metric properties the query names",
            "${PROP('Namespace')}/${PROP('MetricName')} by ${PROP('Stat')} over ${PROP('Period')}s"
                .to_string(),
            format!("{NAMESPACE}/{metric} by Sum over 60s"),
        ),
        (
            "a token this service does not resolve stays as it was written",
            "${NOT_A_TOKEN}".to_string(),
            "${NOT_A_TOKEN}".to_string(),
        ),
        (
            "text with no token at all",
            "a plain label".to_string(),
            "a plain label".to_string(),
        ),
    ];

    for (case, label, expected) in rows {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [{
                    "Id": "m1",
                    "Label": label,
                    "MetricStat": {
                        "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
                        "Period": 60,
                        "Stat": "Sum",
                    },
                }],
                "StartTime": base,
                "EndTime": base + 5.0 * MINUTE,
                "ScanBy": "TimestampAscending",
            }),
        )
        .await;
        let answered = body["MetricDataResults"][0]
            .get("Label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            answered, expected,
            "case {case:?} (label {label:?}): expected {expected:?}, got {answered:?}"
        );
    }

    // "The time zone to use for metric data return in this operation. The format is + or -
    // followed by four digits." (`LabelOptions$Timezone`) — the instants a label states are
    // written in the zone the request names, so the same label over the same data reads
    // differently under a different zone.
    let mut rendered: Vec<String> = Vec::new();
    for timezone in ["+0000", "+0530", "-0800"] {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [{
                    "Id": "m1",
                    "Label": "peak at ${MAX_TIME}",
                    "MetricStat": {
                        "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
                        "Period": 60,
                        "Stat": "Sum",
                    },
                }],
                "StartTime": base,
                "EndTime": base + 5.0 * MINUTE,
                "LabelOptions": {"Timezone": timezone},
            }),
        )
        .await;
        let label = body["MetricDataResults"][0]
            .get("Label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            label.ends_with(timezone),
            "case the zone {timezone:?}: the label must state the instant in the zone the \
             request named, got {label:?}"
        );
        rendered.push(label);
    }
    rendered.sort();
    rendered.dedup();
    assert_eq!(
        rendered.len(),
        3,
        "three different zones must render three different labels, got {rendered:?}"
    );
}

#[tokio::test]
async fn a_percentile_over_an_aggregate_with_no_distribution_answers_no_datapoint() {
    let router = build_router();
    let base = base_time();

    // (case, the datum published, whether a percentile over it has a value). A statistic set
    // keeps no distribution unless the documented conditions hold: "The SampleCount value of
    // the statistic set is 1. The Min and the Max values of the statistic set are equal."
    // (`GetMetricStatistics`)
    let rows: Vec<(&str, Value, bool)> = vec![
        // The recorded case: a spread statistic set, over which no percentile can be computed.
        (
            "a statistic set of several unequal observations",
            json!({"StatisticValues": {"SampleCount": 4.0, "Sum": 40.0, "Minimum": 1.0, "Maximum": 20.0}}),
            false,
        ),
        // One observation: the set IS its own distribution.
        (
            "a statistic set of one observation",
            json!({"StatisticValues": {"SampleCount": 1.0, "Sum": 7.0, "Minimum": 7.0, "Maximum": 7.0}}),
            true,
        ),
        // Equal bounds: every observation took the same value, so the distribution is known.
        (
            "a statistic set whose minimum and maximum are equal",
            json!({"StatisticValues": {"SampleCount": 5.0, "Sum": 15.0, "Minimum": 3.0, "Maximum": 3.0}}),
            true,
        ),
        // A plain value, and a values/counts histogram: the individual values are published,
        // so the percentile is computed from them.
        ("a plain value", json!({"Value": 11.0}), true),
        (
            "a values array weighted by counts",
            json!({"Values": [1.0, 2.0, 3.0], "Counts": [10.0, 1.0, 1.0]}),
            true,
        ),
    ];

    for (index, (case, datum, has_percentile)) in rows.into_iter().enumerate() {
        let metric = format!("Spread{index}");
        publish(&router, &metric, base, datum).await;

        let read = |statistics: Value| {
            let mut request = json!({
                "Namespace": NAMESPACE,
                "MetricName": metric,
                "StartTime": base,
                "EndTime": base + MINUTE,
                "Period": 60,
            });
            for (member, value) in statistics.as_object().cloned().unwrap_or_default() {
                request[member] = value;
            }
            request
        };

        // The percentile alone: either one datapoint carrying it, or no datapoint at all.
        let body = ok(
            &router,
            "GetMetricStatistics",
            &read(json!({"ExtendedStatistics": ["p90"]})),
        )
        .await;
        let datapoints = body["Datapoints"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            datapoints.len(),
            usize::from(has_percentile),
            "case {case:?}: expected {} datapoint(s) for p90, got {body}",
            usize::from(has_percentile)
        );
        if has_percentile {
            assert!(
                datapoints[0]
                    .pointer("/ExtendedStatistics/p90")
                    .and_then(Value::as_f64)
                    .is_some(),
                "case {case:?}: the datapoint must carry the percentile it was asked for, got \
                 {body}"
            );
        }

        // A basic statistic over the SAME window is still a datapoint, so the absence above is
        // about the percentile having no value rather than about the window being empty.
        let folded = ok(
            &router,
            "GetMetricStatistics",
            &read(json!({"Statistics": ["Sum"]})),
        )
        .await;
        assert_eq!(
            folded["Datapoints"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the window must still fold to one datapoint for Sum, got {folded}"
        );
    }
}

#[tokio::test]
async fn an_alarm_reports_an_absent_list_member_as_the_empty_list_it_cannot_be_told_from() {
    let router = build_router();

    // (case, the list member one alarm of the pair leaves out, the empty value the other
    // carries). Each row is a PAIR of alarms whose requests differ only in whether the member
    // carries the empty list or nothing at all — a difference the wire cannot express, so the
    // reader has to report the two alike.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("the dimensions of the metric", "Dimensions", json!([])),
        ("the actions of the ALARM state", "AlarmActions", json!([])),
        ("the actions of the OK state", "OKActions", json!([])),
        (
            "the actions of the INSUFFICIENT_DATA state",
            "InsufficientDataActions",
            json!([]),
        ),
    ];

    for (index, (case, member, empty)) in rows.into_iter().enumerate() {
        let base = json!({
            "Namespace": NAMESPACE,
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
        });

        let mut described: Vec<Option<Value>> = Vec::new();
        for (form, carried) in [("empty", Some(empty.clone())), ("absent", None)] {
            let name = format!("list-member-{index}-{form}");
            let mut request = base.clone();
            request["AlarmName"] = json!(name);
            if let Some(value) = carried {
                request[member] = value;
            }
            ok(&router, "PutMetricAlarm", &request).await;

            let body = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
            described.push(body["MetricAlarms"][0].get(member).cloned());
        }

        assert_eq!(
            described[0], described[1],
            "case {case:?}: the alarm written with an empty {member} and the one written \
             without it must report it alike, got {described:?}"
        );
        assert_eq!(
            described[0],
            Some(empty),
            "case {case:?}: both must report {member} as the empty list, got {described:?}"
        );
    }
}

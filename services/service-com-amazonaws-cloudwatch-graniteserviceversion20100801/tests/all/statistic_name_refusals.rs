// SPDX-License-Identifier: CC0-1.0

//! How a read of one metric refuses a statistic NAME that the member it arrived on cannot
//! serve, and what it still serves.
//!
//! One rule, asserted over two tables: a refusal of a value inside a request's LIST member
//! names the element by its list, its 1-based position and the list's own target shape
//! (`<List>.member.<n>.<list element>`), and states the requirement of that member — for a
//! member whose elements are an enumerated type, the whole value set the type declares and
//! NOT the submitted value, because the set is what a client repairs the request from; for the
//! percentile-only member, the submitted name as one it does not support.
//!
//! `GetMetricStatisticsInput$Statistics` is a list whose member targets the `Statistic` enum
//! and `GetMetricStatisticsInput$ExtendedStatistics` a list of a plain string, so the two
//! names are not interchangeable: the third table is the serving side of that, every declared
//! statistic read through `Statistics` and every percentile-family name read through
//! `ExtendedStatistics`.

use crate::harness::{build_router, call, error_code, error_message, ok};
use serde_json::{json, Value};

/// The namespace every series and every read in this suite names.
const NAMESPACE: &str = "MyApp/StatisticNames";

/// The values the `Statistic` enum declares, in the order the model declares them
/// (`smithy_query(query="shape",shape="Statistic")`): the five basic statistics, which are the
/// only names `Statistics` accepts.
pub const STATISTIC_VALUES: [&str; 5] = ["SampleCount", "Average", "Sum", "Minimum", "Maximum"];

/// The error class a refusal the BOUNDARY validator makes carries, as the body of the answer
/// names it: the modelled `InvalidParameterValueException`, whose legacy `awsQueryError` code
/// (`InvalidParameterValue`) is what a CloudWatch client branches on and travels in the
/// `x-amzn-query-error` header.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The error class a refusal a HANDLER makes carries over this harness's wire: the same
/// modelled shape name, since the class is the same whichever layer refused the request.
const INVALID_PARAMETER_VALUE_SHAPE: &str = "InvalidParameterValueException";

/// What a refusal of element `position` (1-based) of `Statistics` says: the element's path,
/// down to the list's own target shape, and the whole set its enumerated type declares.
pub fn value_in_set_message(position: usize) -> String {
    format!(
        "The parameter Statistics.member.{position}.<list element> must be a value in the set \
         [{}].",
        STATISTIC_VALUES.join(", ")
    )
}

/// What a refusal of element `position` (1-based) of `ExtendedStatistics` says: the submitted
/// name and that element's path.
fn not_supported_message(name: &str, position: usize) -> String {
    format!(
        "The value {name} for parameter ExtendedStatistics.member.{position}.<list element> is \
         not supported."
    )
}

/// A minute-aligned instant ten minutes in the past: inside every retention tier, and aligned
/// so a read's start time rounds to where the request put it.
fn recent() -> f64 {
    (crate::harness::now_secs() / 60.0).floor() * 60.0 - 10.0 * 60.0
}

/// A `GetMetricStatistics` request over `metric` for the one minute at `at`, carrying whatever
/// statistic member `patch` names.
fn read(metric: &str, at: f64, patch: Value) -> Value {
    crate::harness::merged(
        json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "StartTime": at,
            "EndTime": at + 60.0,
            "Period": 60,
        }),
        &patch,
    )
}

/// The one datapoint a read of a seeded minute answers with.
fn only_datapoint(body: &Value, case: &str) -> Value {
    let points = body
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        points.len(),
        1,
        "case {case:?}: expected exactly one datapoint, got {points:?} from {body}"
    );
    points[0].clone()
}

#[tokio::test]
async fn a_statistics_element_outside_the_enum_is_refused_with_the_declared_value_set() {
    let router = build_router();
    let at = recent();

    // The rows differ along what the rule generalises over: WHICH kind of name is outside the
    // enum (a percentile, a bounded-family statistic, a declared name in another case, a name
    // of no statistic at all, the empty name), WHERE in the list it sits — first, last, and the
    // fifth element of a list at the member's documented maximum — and HOW MANY elements the
    // list carries. Nothing is published first: the name is judged before any data is read.
    let rows: Vec<(&str, Vec<&str>, usize)> = vec![
        ("a percentile alone", vec!["p90"], 1),
        ("a percentile after a declared name", vec!["Average", "p90"], 2),
        (
            "a bounded-family statistic at the fifth element of a full list",
            vec!["SampleCount", "Average", "Sum", "Minimum", "tm90"],
            5,
        ),
        ("the interquartile mean", vec!["IQM"], 1),
        ("a declared name in another case", vec!["average"], 1),
        ("a name of no statistic at all", vec!["Sum", "Median"], 2),
        ("the empty name", vec!["Maximum", ""], 2),
        (
            "two offending elements, the earlier one named",
            vec!["p99", "Median", "Sum"],
            1,
        ),
    ];

    for (case, statistics, position) in rows {
        let (status, body) = call(
            &router,
            "GetMetricStatistics",
            &read("Unpublished", at, json!({"Statistics": statistics})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected code {INVALID_PARAMETER_VALUE}, got {code:?} from {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            value_in_set_message(position),
            "case {case:?}: expected the declared value set named for element {position}, got \
             {message:?}"
        );
        // The submitted value is not part of this sentence: the set it had to come from is.
        for name in statistics {
            assert!(
                name.is_empty() || !message.contains(name) || STATISTIC_VALUES.contains(&name),
                "case {case:?}: the refusal must not echo the submitted {name:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_extended_statistics_element_that_is_no_percentile_is_refused_as_unsupported() {
    let router = build_router();
    let at = recent();

    // The rows differ along which name the percentile-only member cannot serve — each of the
    // five declared basic statistics, a statistic over a span of the window's distribution
    // (which belongs to the members that take any CloudWatch statistic, not to this one), and a
    // name that is no statistic at all — and along where in the list it sits: first, second, and
    // the tenth element of a list at that member's documented maximum.
    let mut rows: Vec<(&str, Vec<&str>, usize, &str)> = STATISTIC_VALUES
        .iter()
        .map(|name| ("a declared basic statistic alone", vec![*name], 1, *name))
        .collect();
    rows.push((
        "a basic statistic after a percentile",
        vec!["p90", "Sum"],
        2,
        "Sum",
    ));
    rows.push((
        "a basic statistic at the tenth element of a full list",
        vec![
            "p10", "p20", "p30", "p40", "p50", "p60", "p70", "p80", "p90", "Average",
        ],
        10,
        "Average",
    ));
    rows.push((
        "a name of no statistic at all",
        vec!["Median"],
        1,
        "Median",
    ));
    // "The percentile statistics. Specify values between p0.0 and p100."
    // (`smithy_query(query="doc",shape="GetMetricStatisticsInput",member="ExtendedStatistics")`)
    // — a statistic over a span of the window's distribution is not a percentile, in either of
    // the forms the definitions page writes one in, and nor is a percentile spelled with the
    // wrong case.
    rows.push((
        "a bounded-family statistic under its bare form",
        vec!["tm90"],
        1,
        "tm90",
    ));
    rows.push((
        "a bounded-family statistic under its span form",
        vec!["p99", "TM(10%:90%)"],
        2,
        "TM(10%:90%)",
    ));
    rows.push(("the interquartile mean", vec!["IQM"], 1, "IQM"));
    rows.push((
        "a percentile spelled with an upper-case prefix",
        vec!["P90"],
        1,
        "P90",
    ));

    for (case, extended, position, offending) in rows {
        let (status, body) = call(
            &router,
            "GetMetricStatistics",
            &read("Unpublished", at, json!({"ExtendedStatistics": extended})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({offending}): expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE_SHAPE,
            "case {case:?} ({offending}): expected code {INVALID_PARAMETER_VALUE_SHAPE}, got \
             {code:?} from {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            not_supported_message(offending, position),
            "case {case:?}: expected {offending} named unsupported at element {position}, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn each_member_still_serves_the_statistic_names_that_belong_on_it() {
    let router = build_router();
    let at = recent();
    let metric = "Dist";

    // One hundred observations at one instant, so every basic statistic and every percentile
    // of the window has a value a row can state.
    let values: Vec<f64> = (1..=100).map(f64::from).collect();
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": metric,
                "Timestamp": at,
                "Values": values,
            }],
        }),
    )
    .await;

    // Every value the enum declares is served on `Statistics`, each reported under its own
    // output member and nowhere else.
    let expected: Vec<(&str, f64)> = vec![
        ("SampleCount", 100.0),
        ("Average", 50.5),
        ("Sum", 5_050.0),
        ("Minimum", 1.0),
        ("Maximum", 100.0),
    ];
    for (statistic, want) in expected {
        let body = ok(
            &router,
            "GetMetricStatistics",
            &read(metric, at, json!({"Statistics": [statistic]})),
        )
        .await;
        let point = only_datapoint(&body, statistic);
        let got = point.get(statistic).and_then(Value::as_f64);
        assert_eq!(
            got,
            Some(want),
            "case {statistic:?}: expected {want} under {statistic}, got {got:?} from {point}"
        );
        // "The metric statistics, other than percentile" — one statistic asked for is one
        // statistic answered.
        assert!(
            point.get("ExtendedStatistics").is_none(),
            "case {statistic:?}: a basic statistic read must report no extended statistic, got \
             {point}"
        );
    }

    // And every percentile is served on `ExtendedStatistics`, under the name the request
    // spelled, with no basic statistic beside it. The rows span the percentile bounds and a
    // fractional percentile between them.
    for name in ["p0", "p50", "p90", "p99.9", "p100"] {
        let body = ok(
            &router,
            "GetMetricStatistics",
            &read(metric, at, json!({"ExtendedStatistics": [name]})),
        )
        .await;
        let point = only_datapoint(&body, name);
        let got = point
            .get("ExtendedStatistics")
            .and_then(|reported| reported.get(name))
            .and_then(Value::as_f64);
        assert!(
            got.is_some_and(|value| (1.0..=100.0).contains(&value)),
            "case {name:?}: expected a value inside the published range under {name}, got \
             {got:?} from {point}"
        );
        for statistic in STATISTIC_VALUES {
            assert!(
                point.get(statistic).is_none(),
                "case {name:?}: a percentile read must report no {statistic}, got {point}"
            );
        }
    }

    // The percentile of the published distribution, which is what the two members disagreeing
    // about a name is about: asked for as the request spells it, computed over the window.
    let body = ok(
        &router,
        "GetMetricStatistics",
        &read(metric, at, json!({"ExtendedStatistics": ["p90"]})),
    )
    .await;
    let point = only_datapoint(&body, "p90");
    let got = point["ExtendedStatistics"]["p90"].as_f64();
    assert!(
        got.is_some_and(|value| (88.0..=92.0).contains(&value)),
        "case p90: expected the 90th percentile of 1..=100, got {got:?} from {point}"
    );
}

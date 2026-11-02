// SPDX-License-Identifier: CC0-1.0

//! The `Values`/`Counts` publication form of `PutMetricData`, end to end over the wire.
//!
//! ONE rule, asserted over a table: **a datum carrying `Values` and `Counts` of the SAME length
//! is accepted and aggregated as the histogram it describes — each value weighted by the count
//! beside it — and one whose two arrays differ in length is refused naming the pair**. "Each
//! number in the `Count` array is the number of times the corresponding value in the `Values`
//! array occurred during the period. If you omit the `Counts` array, the default of 1 is used
//! as the value for each count. If you include a `Counts` array, it must include the same
//! amount of values as the `Values` array."
//! (`smithy_query(query="doc",shape="MetricDatum",member="Counts")`)
//!
//! The rows differ along what the rule generalises over: how many values the datum carries,
//! whether `Counts` is there at all, which side is longer when the two disagree, and WHERE in
//! the `MetricData` list the offending datum sits — since the member path a refusal names is
//! taken from the request. The refusal's own SENTENCE is the one this service already carried:
//! this suite pins the member path and the pair it names, which are the parts the request
//! decides.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, error_message, ok};

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/Histogram";

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// A minute-aligned instant ten minutes in the past, so the published datapoints sit inside
/// every retention tier and a request's start time rounds to where it was put.
fn base_time() -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now / MINUTE).floor() * MINUTE - 10.0 * MINUTE
}

/// The `(SampleCount, Sum, Minimum, Maximum)` one window of a series folds to.
type Aggregate = (f64, f64, f64, f64);

/// The `(SampleCount, Sum, Minimum, Maximum)` the window holding `metric`'s datapoints folds
/// to.
async fn aggregated(
    router: &smithy_server_runtime::MultiServiceRouter,
    metric: &str,
    base: f64,
) -> Aggregate {
    let body = ok(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "StartTime": base,
            "EndTime": base + MINUTE,
            "Period": 60,
            "Statistics": ["SampleCount", "Sum", "Minimum", "Maximum"],
        }),
    )
    .await;
    let datapoint = body["Datapoints"][0].clone();
    let number = |member: &str| {
        datapoint
            .get(member)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("no {member} in {body}"))
    };
    (
        number("SampleCount"),
        number("Sum"),
        number("Minimum"),
        number("Maximum"),
    )
}

#[tokio::test]
async fn a_values_and_counts_histogram_is_accepted_and_aggregated() {
    let router = build_router();
    let base = base_time();

    // (case, the datum's observation members, the aggregate the window must fold to)
    let rows: Vec<(&str, Value, Aggregate)> = vec![
        // The recorded shape: three values, each weighted by its own count.
        (
            "three values weighted by counts",
            json!({"Values": [1.0, 2.0, 3.0], "Counts": [10.0, 1.0, 1.0]}),
            (12.0, 15.0, 1.0, 3.0),
        ),
        (
            "one value weighted many times",
            json!({"Values": [4.0], "Counts": [25.0]}),
            (25.0, 100.0, 4.0, 4.0),
        ),
        // "If you omit the Counts array, the default of 1 is used as the value for each count."
        (
            "values with no Counts array at all",
            json!({"Values": [2.0, 5.0]}),
            (2.0, 7.0, 2.0, 5.0),
        ),
        // A count of 1 everywhere is the same histogram as omitting the array.
        (
            "values with every count set to one",
            json!({"Values": [2.0, 5.0], "Counts": [1.0, 1.0]}),
            (2.0, 7.0, 2.0, 5.0),
        ),
        // A repeated value is listed once with the count it occurred, which is what the form
        // is for.
        (
            "the same value listed twice",
            json!({"Values": [7.0, 7.0], "Counts": [2.0, 3.0]}),
            (5.0, 35.0, 7.0, 7.0),
        ),
    ];

    for (index, (case, observations, expected)) in rows.into_iter().enumerate() {
        let metric = format!("Histogram{index}");
        let mut datum = json!({"MetricName": metric, "Timestamp": base, "Unit": "Count"});
        for (member, value) in observations.as_object().cloned().unwrap_or_default() {
            datum[member] = value;
        }
        let (status, body) = call(
            &router,
            "PutMetricData",
            &json!({"Namespace": NAMESPACE, "MetricData": [datum]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a histogram whose arrays agree in length must be accepted, got \
             {status} {body}"
        );

        let folded = aggregated(&router, &metric, base).await;
        assert_eq!(
            folded, expected,
            "case {case:?}: expected the window to fold to {expected:?}, got {folded:?}"
        );
    }
}

#[tokio::test]
async fn arrays_that_disagree_in_length_are_refused_naming_the_pair() {
    let router = build_router();
    let base = base_time();

    // (case, the offending datum's observation members, its 1-based position in MetricData)
    let rows: Vec<(&str, Value, usize)> = vec![
        (
            "fewer counts than values",
            json!({"Values": [1.0, 2.0, 3.0], "Counts": [1.0, 1.0]}),
            1,
        ),
        (
            "more counts than values",
            json!({"Values": [1.0], "Counts": [1.0, 1.0]}),
            1,
        ),
        (
            "the offending datum second in the list",
            json!({"Values": [1.0, 2.0], "Counts": [1.0]}),
            2,
        ),
        (
            "the offending datum third in the list",
            json!({"Values": [1.0, 2.0], "Counts": [1.0, 1.0, 1.0]}),
            3,
        ),
    ];

    for (case, observations, position) in rows {
        // The data before the offending one are well-formed publishes of their own, so what is
        // refused is the datum the row is about.
        let mut data: Vec<Value> = (1..position)
            .map(|index| {
                json!({
                    "MetricName": format!("Filler{index}"),
                    "Timestamp": base,
                    "Value": 1.0,
                })
            })
            .collect();
        let mut datum = json!({"MetricName": "Mismatched", "Timestamp": base});
        for (member, value) in observations.as_object().cloned().unwrap_or_default() {
            datum[member] = value;
        }
        data.push(datum);

        let (status, body) = call(
            &router,
            "PutMetricData",
            &json!({"Namespace": NAMESPACE, "MetricData": data}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected the publish to be refused as a bad parameter value, got \
             {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "The parameters MetricData.member.{position}.Values and \
                 MetricData.member.{position}.Counts must be of the same size."
            ),
            "case {case:?}: expected the refusal to name the pair at position {position}, got \
             {body}"
        );

        // Nothing of a refused publish is stored, not even the data that were well formed.
        let listed = ok(&router, "ListMetrics", &json!({"Namespace": NAMESPACE})).await;
        assert_eq!(
            listed["Metrics"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused publish must store no series at all, got {listed}"
        );
    }
}

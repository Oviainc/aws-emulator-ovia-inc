// SPDX-License-Identifier: CC0-1.0

//! The TEXT a metric read is refused with for a `Period` that is not a granularity a metric
//! can be read at, and the granularities it accepts.
//!
//! The rule under test: **a `Period` outside the granularities a metric is stored at is
//! refused `InvalidParameterValue` with CloudWatch's own sentence for a value outside the set
//! its member accepts — the member named and the sub-minute granularities enumerated, and
//! nothing of the submitted value** — while every documented granularity (the sub-minute
//! divisions of a minute, and any multiple of 60) is accepted. `GetMetricStatisticsInput$Period`
//! and `MetricStat$Period` document the same set ("For metrics with regular resolution, a
//! period can be as short as one minute (60 seconds) and must be a multiple of 60. For
//! high-resolution metrics […] the period can be 1, 5, 10, 20, 30, 60, or any multiple of 60"),
//! so both readers answer alike.
//!
//! The rows differ along the period itself — each published sub-minute granularity, values on
//! either side of them, a value just under and just over a minute, and multiples of 60 up to an
//! hour — and along which reader the period arrived on. The period a recorded request happened
//! to carry is one row among them.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, error_message, now_secs, ok};

/// The sentence CloudWatch refuses a period outside the readable granularities with. Entirely
/// fixed: it names the member and the set, and carries nothing of the submitted value.
const PERIOD_REFUSAL: &str = "The parameter Period must be a value in the set [1, 5, 10, 20, 30].";

/// The wire error code of that refusal, as an SDK of this service sees it.
const REFUSAL_CODE: &str = "InvalidParameterValueException";

/// A `GetMetricStatistics` request reading `period`-wide windows of the ten periods before now.
///
/// The range is ten periods whatever the row's period, so no row is refused for the range
/// instead of the period: a short period keeps its datapoints readable (they are kept for
/// three hours below a minute) and a long one stays inside the datapoints one call returns.
fn statistics_request(period: i64) -> Value {
    #[allow(clippy::cast_precision_loss)]
    let span = (period * 10) as f64;
    let now = now_secs();
    json!({
        "Namespace": "Custom/Periods",
        "MetricName": "Latency",
        "StartTime": now - span,
        "EndTime": now,
        "Period": period,
        "Statistics": ["Sum"],
    })
}

/// A `GetMetricData` request reading the same series at the same granularity, with the period
/// on the query's `MetricStat`.
fn data_request(period: i64) -> Value {
    #[allow(clippy::cast_precision_loss)]
    let span = (period * 10) as f64;
    let now = now_secs();
    json!({
        "MetricDataQueries": [{
            "Id": "m1",
            "MetricStat": {
                "Metric": {"Namespace": "Custom/Periods", "MetricName": "Latency"},
                "Period": period,
                "Stat": "Sum",
            },
        }],
        "StartTime": now - span,
        "EndTime": now,
    })
}

#[tokio::test]
async fn a_period_outside_the_readable_granularities_is_refused_with_the_set_it_must_come_from() {
    let router = build_router();
    // Each row is a period no documented granularity admits: either side of every published
    // sub-minute value, just under and just over a minute, and between two multiples of 60.
    // Source for the set: `GetMetricStatisticsInput$Period` / `MetricStat$Period`.
    let periods: [i64; 9] = [2, 4, 7, 11, 21, 31, 45, 59, 61];

    for period in periods {
        for (reader, request) in [
            ("GetMetricStatistics", statistics_request(period)),
            ("GetMetricData", data_request(period)),
        ] {
            let (status, body) = call(&router, reader, &request).await;
            assert_eq!(
                status, 400,
                "case period {period} on {reader}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                REFUSAL_CODE,
                "case period {period} on {reader}: expected {REFUSAL_CODE}, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, PERIOD_REFUSAL,
                "case period {period} on {reader}: expected the fixed refusal \
                 {PERIOD_REFUSAL:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_documented_granularity_is_read_rather_than_refused() {
    let router = build_router();
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "Custom/Periods",
            "MetricData": [{"MetricName": "Latency", "Value": 1.0, "StorageResolution": 1}],
        }),
    )
    .await;

    // Every published sub-minute granularity, the minute itself, and multiples of it up to an
    // hour: the values the two members document as readable.
    let periods: [i64; 9] = [1, 5, 10, 20, 30, 60, 120, 300, 3_600];

    for period in periods {
        for (reader, request) in [
            ("GetMetricStatistics", statistics_request(period)),
            ("GetMetricData", data_request(period)),
        ] {
            let (status, body) = call(&router, reader, &request).await;
            assert_eq!(
                status, 200,
                "case period {period} on {reader}: a documented granularity is read, got \
                 {status} with {body}"
            );
        }
    }
}

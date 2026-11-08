// SPDX-License-Identifier: CC0-1.0

//! The TEXT a metric read is refused with for a `Period` that is not a granularity a metric
//! can be read at, and the granularities it accepts.
//!
//! The rule under test: **a `Period` outside the granularities a metric is stored at is refused
//! `InvalidParameterValue` against the requirement it broke — a period SHORTER than a minute
//! had to be one of the sub-minute divisions of one, stated as CloudWatch states a value
//! outside the set its member accepts, and a LONGER one had to be a whole number of minutes,
//! stated as the multiple it was not — with nothing of the submitted value in either
//! sentence**, while every documented granularity (the sub-minute divisions of a minute, and
//! any multiple of 60) is accepted. `GetMetricStatisticsInput$Period` and `MetricStat$Period`
//! document the same set ("For metrics with regular resolution, a period can be as short as
//! one minute (60 seconds) and must be a multiple of 60. For high-resolution metrics […] the
//! period can be 1, 5, 10, 20, 30, 60, or any multiple of 60"), so both readers answer alike.
//!
//! The rows differ along the period itself — each published sub-minute granularity, values on
//! either side of them, a value just under and just over a minute, and one between two
//! multiples of 60 — and along which reader the period arrived on. The period a recorded
//! request happened to carry is one row among them.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, error_message, now_secs, ok};

/// The sentence CloudWatch refuses a period SHORTER than a minute with: the member and the
/// sub-minute set it had to come from, and nothing of the submitted value.
const SUB_MINUTE_REFUSAL: &str =
    "The parameter Period must be a value in the set [1, 5, 10, 20, 30].";

/// The sentence it refuses a LONGER one with: the whole number of minutes it was not.
const MULTIPLE_REFUSAL: &str = "The parameter Period must be a multiple of 60.";

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
async fn a_period_outside_the_readable_granularities_is_refused_with_the_requirement_it_broke() {
    let router = build_router();
    // (the period, the requirement its refusal states): every period below a minute that no
    // sub-minute granularity admits is told the set, and every one above it that is not a whole
    // number of minutes is told the multiple.
    // Source for the set: `GetMetricStatisticsInput$Period` / `MetricStat$Period`.
    let periods: [(i64, &str); 9] = [
        (2, SUB_MINUTE_REFUSAL),
        (4, SUB_MINUTE_REFUSAL),
        (7, SUB_MINUTE_REFUSAL),
        (11, SUB_MINUTE_REFUSAL),
        (21, SUB_MINUTE_REFUSAL),
        (31, SUB_MINUTE_REFUSAL),
        (45, SUB_MINUTE_REFUSAL),
        (59, SUB_MINUTE_REFUSAL),
        (61, MULTIPLE_REFUSAL),
    ];

    for (period, expected) in periods {
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
                message, expected,
                "case period {period} on {reader}: expected the fixed refusal {expected:?}, got \
                 {message:?}"
            );
        }
    }

    // A period far above a minute that is not a whole number of them is told the same
    // requirement, so the second sentence is the rule and not the one value just over a minute.
    for period in [121, 3_599, 86_401] {
        let (_, body) = call(&router, "GetMetricStatistics", &statistics_request(period)).await;
        assert_eq!(
            error_message(&body),
            MULTIPLE_REFUSAL,
            "case period {period}: expected {MULTIPLE_REFUSAL:?}, got {body}"
        );
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

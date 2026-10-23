// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInsightsMetricData`.
//!
//! Every case goes over the wire: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: CloudTrail_20131101.ListInsightsMetricData` through the same
//! `MultiServiceRouter` the production router serves, so what is asserted here is
//! what a client sees — including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * the reported series is the one the request names (`EventSource`, `EventName`,
//!   `InsightType`, and `ErrorCode` — "only returned if InsightType parameter was set
//!   to ApiErrorRateInsight", and conditionally required for it);
//! * `Period` is "60, 300, and 3600" seconds and "if you specify any other value, you
//!   will get an error";
//! * a data point sits on a multiple of `Period` since the epoch, inside the window
//!   `[StartTime, EndTime)` — the start inclusive, the end exclusive;
//! * how far back data points reach is the `Period`'s documented cutoff (60s/15 days,
//!   300s/63 days, 3600s/90 days), each cutoff inclusive;
//! * `FillWithZeros` reports every period in the window and `NonZeroData` — the
//!   default — only the periods that measured something, of which this emulator
//!   records none: it observes no API traffic to generate an Insights event from;
//! * `MaxResults` bounds the page and `NextToken` continues it, losing and repeating
//!   no data point;
//! * `TrailName` is "the Amazon Resource Name(ARN) or name of the trail", resolved
//!   through the trail registry every other trail operation reads;
//! * reporting a series writes nothing: a second identical call reports the same.

use serde_json::{json, Value};

// The emulator-under-test harness is shared with this crate's other suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, HOME_REGION};

/// Seconds in a day, the unit the retention cutoffs are documented in.
const SECONDS_PER_DAY: i64 = 86_400;

/// Each `Period` the operation doc says CloudTrail serves, and how many days back
/// its data points reach: "Data points with a period of 60 seconds (1-minute) are
/// available for 15 days. Data points with a period of 300 seconds (5-minute) are
/// available for 63 days. Data points with a period of 3600 seconds (1 hour) are
/// available for 90 days."
const PERIOD_RETENTION_DAYS: [(i64, i64); 3] = [(60, 15), (300, 63), (3600, 90)];

/// One row of [`the_reported_series_is_the_one_the_request_names`]'s table: the
/// case, the `EventSource`, the `EventName`, the `InsightType`, the `ErrorCode` the
/// request sends, and the `ErrorCode` the response is expected to report.
type SeriesRow<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
);

/// One row of [`the_trail_named_is_resolved_through_the_registry`]'s table: the
/// case, the `TrailName` the request sends, the Region it is sent from, the
/// `TrailARN` the response is expected to report, and the error code it is expected
/// to fail with (`""` for a success).
type TrailRow<'a> = (&'a str, Option<&'a str>, &'a str, Option<&'a str>, &'a str);

/// The epoch second this test is running at.
fn now_secs() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch");
    i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)
}

/// A reply's `Timestamps`, as epoch seconds (the wire form of an `awsJson1_1`
/// timestamp).
fn timestamps(body: &Value) -> Vec<i64> {
    body.get("Timestamps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Timestamps list, got {body:?}"))
        .iter()
        .map(|at| {
            let seconds = at
                .as_f64()
                .unwrap_or_else(|| panic!("expected an epoch-second timestamp, got {at:?}"));
            seconds.round() as i64
        })
        .collect()
}

/// A reply's `Values`.
fn values(body: &Value) -> Vec<f64> {
    body.get("Values")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Values list, got {body:?}"))
        .iter()
        .map(|value| {
            value
                .as_f64()
                .unwrap_or_else(|| panic!("expected a numeric value, got {value:?}"))
        })
        .collect()
}

/// The multiples of `period` in `[start, end)` — the grid a data point sits on.
fn grid(start: i64, end: i64, period: i64) -> Vec<i64> {
    let mut points = Vec::new();
    let aligned = start.div_euclid(period) * period;
    let mut at = if aligned == start {
        aligned
    } else {
        aligned + period
    };
    while at < end {
        points.push(at);
        at += period;
    }
    points
}

/// The series identity is the one the request names, and `ErrorCode` travels only
/// with the Insights type it keys.
#[tokio::test]
async fn the_reported_series_is_the_one_the_request_names() {
    let emulator = Emulator::new();

    // (case, event source, event name, insight type, error code, echoed error code)
    let rows: [SeriesRow<'_>; 5] = [
        (
            "call rate, no error code",
            "iam.amazonaws.com",
            "AddUserToGroup",
            "ApiCallRateInsight",
            None,
            None,
        ),
        (
            // "Only returned if InsightType parameter was set to
            // ApiErrorRateInsight": a code sent with the call-rate type keys
            // nothing, so it comes back on neither.
            "call rate with an error code",
            "ec2.amazonaws.com",
            "RunInstances",
            "ApiCallRateInsight",
            Some("AccessDenied"),
            None,
        ),
        (
            "error rate on AccessDenied",
            "s3.amazonaws.com",
            "PutBucketPolicy",
            "ApiErrorRateInsight",
            Some("AccessDenied"),
            Some("AccessDenied"),
        ),
        (
            "error rate on a throttle",
            "kms.amazonaws.com",
            "Decrypt",
            "ApiErrorRateInsight",
            Some("ThrottlingException"),
            Some("ThrottlingException"),
        ),
        (
            "error rate on a dotted code",
            "cloudtrail.amazonaws.com",
            "CreateTrail",
            "ApiErrorRateInsight",
            Some("Client.RequestLimitExceeded"),
            Some("Client.RequestLimitExceeded"),
        ),
    ];

    for (case, event_source, event_name, insight_type, error_code, echoed) in rows {
        let mut request = json!({
            "EventSource": event_source,
            "EventName": event_name,
            "InsightType": insight_type,
        });
        if let Some(code) = error_code {
            request["ErrorCode"] = json!(code);
        }
        let reply = emulator.call("ListInsightsMetricData", &request).await;
        let body = reply.expect_ok(case).clone();
        assert_eq!(
            (
                body["EventSource"].as_str(),
                body["EventName"].as_str(),
                body["InsightType"].as_str(),
            ),
            (Some(event_source), Some(event_name), Some(insight_type)),
            "case {case}: the response does not report the series the request named, \
             got {body:?}"
        );
        assert_eq!(
            body.get("ErrorCode").and_then(Value::as_str),
            echoed,
            "case {case}: expected ErrorCode {echoed:?}, got {body:?}"
        );
        assert_eq!(
            timestamps(&body).len(),
            values(&body).len(),
            "case {case}: Timestamps and Values must be the same length, got {body:?}"
        );
    }

    // "Conditionally required if the InsightType parameter is set to
    // ApiErrorRateInsight": that series counts one error, so without it there is no
    // series to report.
    emulator
        .call(
            "ListInsightsMetricData",
            &json!({
                "EventSource": "s3.amazonaws.com",
                "EventName": "PutBucketPolicy",
                "InsightType": "ApiErrorRateInsight",
            }),
        )
        .await
        .expect_error(
            "ApiErrorRateInsight without an ErrorCode",
            400,
            "InvalidParameterException",
        );
}

/// `Period` is one of the three the service serves; any other value is refused, and
/// a value outside the modelled range is refused by the model itself.
#[tokio::test]
async fn only_a_served_period_is_accepted() {
    let emulator = Emulator::new();

    // (case, Period, expected error code — "" for a success)
    let rows: [(&str, Value, &str); 11] = [
        ("one minute", json!(60), ""),
        ("five minutes", json!(300), ""),
        ("one hour", json!(3600), ""),
        ("omitted, defaults to 3600", Value::Null, ""),
        ("just past the minute", json!(61), "InvalidParameterException"),
        ("two minutes", json!(120), "InvalidParameterException"),
        (
            "just under five minutes",
            json!(299),
            "InvalidParameterException",
        ),
        ("half an hour", json!(1800), "InvalidParameterException"),
        (
            "just under the hour",
            json!(3599),
            "InvalidParameterException",
        ),
        // One past each published bound of `InsightsMetricPeriod` (range 60..3600),
        // which the model refuses before the handler runs.
        ("under the modelled minimum", json!(59), "ValidationException"),
        ("over the modelled maximum", json!(3601), "ValidationException"),
    ];

    for (case, period, expected) in rows {
        let mut request = json!({
            "EventSource": "iam.amazonaws.com",
            "EventName": "AddUserToGroup",
            "InsightType": "ApiCallRateInsight",
        });
        if !period.is_null() {
            request["Period"] = period.clone();
        }
        let reply = emulator.call("ListInsightsMetricData", &request).await;
        if expected.is_empty() {
            reply.expect_ok(case);
        } else {
            reply.expect_error(case, 400, expected);
        }
    }
}

/// `FillWithZeros` reports every period boundary in `[StartTime, EndTime)`, and each
/// value is the zero the emulator measured.
#[tokio::test]
async fn fill_with_zeros_reports_every_period_in_the_window() {
    let emulator = Emulator::new();

    // (period, how many periods the window spans)
    let rows: [(i64, i64); 6] = [(60, 5), (60, 1), (300, 4), (3600, 3), (3600, 1), (60, 0)];

    for (period, spans) in rows {
        let case = format!("period {period}, {spans} period(s)");
        // A window aligned to the period, so the expected grid is exact, and recent
        // enough to sit inside every retention cutoff.
        let now = now_secs();
        let end = now - now.rem_euclid(period);
        let start = end - spans * period;
        let reply = emulator
            .call(
                "ListInsightsMetricData",
                &json!({
                    "EventSource": "s3.amazonaws.com",
                    "EventName": "GetBucketPolicy",
                    "InsightType": "ApiCallRateInsight",
                    "Period": period,
                    "DataType": "FillWithZeros",
                    "StartTime": start,
                    "EndTime": end,
                }),
            )
            .await;
        let body = reply.expect_ok(&case).clone();
        let expected = grid(start, end, period);
        assert_eq!(
            timestamps(&body),
            expected,
            "case {case}: expected the period grid {expected:?}, got {body:?}"
        );
        assert_eq!(
            values(&body),
            vec![0.0; expected.len()],
            "case {case}: expected a zero for every period, got {body:?}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {case}: the whole window fits in one page, so there is nothing to \
             continue, got {body:?}"
        );
    }
}

/// `NonZeroData` — the default — reports only periods that measured something, and
/// this emulator observes no API traffic to generate an Insights event from.
#[tokio::test]
async fn non_zero_data_reports_no_data_point() {
    let emulator = Emulator::new();

    // (case, DataType, Period)
    let rows: [(&str, Value, Value); 5] = [
        ("default, default period", Value::Null, Value::Null),
        ("default, one minute", Value::Null, json!(60)),
        ("explicit, one minute", json!("NonZeroData"), json!(60)),
        ("explicit, five minutes", json!("NonZeroData"), json!(300)),
        ("explicit, one hour", json!("NonZeroData"), json!(3600)),
    ];

    for (case, data_type, period) in rows {
        let mut request = json!({
            "EventSource": "iam.amazonaws.com",
            "EventName": "CreateUser",
            "InsightType": "ApiCallRateInsight",
        });
        if !data_type.is_null() {
            request["DataType"] = data_type.clone();
        }
        if !period.is_null() {
            request["Period"] = period.clone();
        }
        let reply = emulator.call("ListInsightsMetricData", &request).await;
        let body = reply.expect_ok(case).clone();
        assert_eq!(
            (timestamps(&body), values(&body)),
            (Vec::new(), Vec::new()),
            "case {case}: no Insights event is recorded, so no period measured a \
             non-zero value, got {body:?}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {case}: an empty series has nothing to continue, got {body:?}"
        );
    }
}

/// `MaxResults` bounds the page; following `NextToken` walks the rest of the window,
/// losing and repeating no data point.
#[tokio::test]
async fn max_results_pages_the_window_through_next_token() {
    let emulator = Emulator::new();

    const PERIOD: i64 = 60;
    const SPANS: i64 = 7;

    for max_results in [1_i64, 2, 3, 7, 10] {
        let case = format!("MaxResults {max_results}");
        let now = now_secs();
        let end = now - now.rem_euclid(PERIOD);
        let start = end - SPANS * PERIOD;
        let expected = grid(start, end, PERIOD);

        let mut walked: Vec<i64> = Vec::new();
        let mut pages: usize = 0;
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({
                "EventSource": "lambda.amazonaws.com",
                "EventName": "Invoke",
                "InsightType": "ApiCallRateInsight",
                "Period": PERIOD,
                "DataType": "FillWithZeros",
                "StartTime": start,
                "EndTime": end,
                "MaxResults": max_results,
            });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let reply = emulator.call("ListInsightsMetricData", &request).await;
            let body = reply.expect_ok(&case).clone();
            let page = timestamps(&body);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                "case {case}: a page may not carry more than MaxResults data points, \
                 got {body:?}"
            );
            assert_eq!(
                values(&body).len(),
                page.len(),
                "case {case}: Timestamps and Values must be the same length, got {body:?}"
            );
            walked.extend(page);
            pages += 1;
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                i64::try_from(pages).unwrap_or(i64::MAX) <= SPANS + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            walked, expected,
            "case {case}: paging must walk the window's grid exactly once, got {walked:?}"
        );
        let page_size = usize::try_from(max_results).unwrap_or(1).max(1);
        let expected_pages = expected.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} page(s) for {} data points, got {pages}",
            expected.len()
        );
    }
}

/// How far back a data point reaches is the `Period`'s documented cutoff: a window
/// starting before it starts at it instead.
#[tokio::test]
async fn the_retention_cutoff_bounds_how_far_back_a_data_point_reaches() {
    let emulator = Emulator::new();

    for (period, days) in PERIOD_RETENTION_DAYS {
        let case = format!("period {period}, {days} days of retention");
        let now = now_secs();
        let reply = emulator
            .call(
                "ListInsightsMetricData",
                &json!({
                    "EventSource": "sts.amazonaws.com",
                    "EventName": "AssumeRole",
                    "InsightType": "ApiCallRateInsight",
                    "Period": period,
                    "DataType": "FillWithZeros",
                    // Well before every published cutoff.
                    "StartTime": now - 200 * SECONDS_PER_DAY,
                    "MaxResults": 1,
                }),
            )
            .await;
        let body = reply.expect_ok(&case).clone();
        let reported = timestamps(&body);
        let cutoff = now - days * SECONDS_PER_DAY;
        assert_eq!(
            reported.len(),
            1,
            "case {case}: MaxResults 1 asks for one data point, got {body:?}"
        );
        let first = reported[0];
        assert!(
            first >= cutoff,
            "case {case}: the oldest data point must not precede the cutoff {cutoff}, \
             got {first}"
        );
        // The first point is the first period boundary at or after the cutoff, plus
        // whatever the clock moved between this test reading it and the handler.
        assert!(
            first <= cutoff + period + 2,
            "case {case}: the oldest data point must be the first period boundary at \
             or after the cutoff {cutoff}, got {first}"
        );
        assert!(
            body.get("NextToken").is_some(),
            "case {case}: a window wider than MaxResults must report where to resume, \
             got {body:?}"
        );
    }
}

/// A window that runs backwards is refused; an empty one holds no data point.
#[tokio::test]
async fn an_inverted_window_is_refused_and_an_empty_one_is_empty() {
    let emulator = Emulator::new();
    let now = now_secs();

    // (case, StartTime, EndTime, expected error code — "" for a success)
    let rows: [(&str, i64, i64, &str); 4] = [
        ("one second backwards", now, now - 1, "InvalidParameterException"),
        ("an hour backwards", now, now - 3600, "InvalidParameterException"),
        ("a day backwards", now, now - SECONDS_PER_DAY, "InvalidParameterException"),
        // `StartTime` is inclusive and `EndTime` exclusive, so an instant-wide
        // window contains no period at all.
        ("an instant-wide window", now, now, ""),
    ];

    for (case, start, end, expected) in rows {
        let reply = emulator
            .call(
                "ListInsightsMetricData",
                &json!({
                    "EventSource": "iam.amazonaws.com",
                    "EventName": "AddUserToGroup",
                    "InsightType": "ApiCallRateInsight",
                    "DataType": "FillWithZeros",
                    "Period": 60,
                    "StartTime": start,
                    "EndTime": end,
                }),
            )
            .await;
        if expected.is_empty() {
            let body = reply.expect_ok(case).clone();
            assert_eq!(
                (timestamps(&body), values(&body)),
                (Vec::new(), Vec::new()),
                "case {case}: an empty window holds no data point, got {body:?}"
            );
        } else {
            reply.expect_error(case, 400, expected);
        }
    }
}

/// An `EndTime` after the time of request reports the window up to the time of
/// request: no data point exists after it, so no request can ask this handler to
/// walk an unbounded number of periods.
#[tokio::test]
async fn an_end_time_after_the_request_is_bounded_by_the_time_of_request() {
    let emulator = Emulator::new();

    // (case, DataType, EndTime, MaxResults, expected data points)
    let rows: [(&str, Value, i64, Value, Option<usize>); 5] = [
        // The default `DataType` reports the measurements, of which there are none,
        // however far ahead the window reaches.
        ("default type, a day ahead", Value::Null, SECONDS_PER_DAY, Value::Null, Some(0)),
        (
            "default type, a century ahead",
            Value::Null,
            100 * 365 * SECONDS_PER_DAY,
            Value::Null,
            Some(0),
        ),
        (
            "default type, far past any clock",
            Value::Null,
            1_000_000_000_000_000,
            Value::Null,
            Some(0),
        ),
        // `FillWithZeros` reports periods, so the page — not the caller's window —
        // bounds how many come back.
        (
            "fill with zeros, a century ahead",
            json!("FillWithZeros"),
            100 * 365 * SECONDS_PER_DAY,
            json!(5),
            Some(5),
        ),
        (
            "fill with zeros, far past any clock",
            json!("FillWithZeros"),
            1_000_000_000_000_000,
            json!(7),
            Some(7),
        ),
    ];

    for (case, data_type, ahead, max_results, expected_points) in rows {
        let now = now_secs();
        let mut request = json!({
            "EventSource": "iam.amazonaws.com",
            "EventName": "AddUserToGroup",
            "InsightType": "ApiCallRateInsight",
            // The shortest period, so the widest grid a window can span.
            "Period": 60,
            "EndTime": now.saturating_add(ahead),
        });
        if !data_type.is_null() {
            request["DataType"] = data_type.clone();
        }
        if !max_results.is_null() {
            request["MaxResults"] = max_results.clone();
        }

        let started = std::time::Instant::now();
        let reply = emulator.call("ListInsightsMetricData", &request).await;
        let elapsed = started.elapsed();
        let body = reply.expect_ok(case).clone();
        let reported = timestamps(&body);
        assert_eq!(
            Some(reported.len()),
            expected_points,
            "case {case}: expected {expected_points:?} data point(s), got {body:?}"
        );
        assert!(
            reported.iter().all(|at| *at <= now + 60),
            "case {case}: no data point exists after the time of request, got {reported:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "case {case}: the window is bounded by the time of request, so the report \
             must not walk the caller's window; it took {elapsed:?}"
        );
    }
}

/// A `NextToken` this service did not issue is refused rather than silently ignored.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let emulator = Emulator::new();
    let now = now_secs();
    let end = now - now.rem_euclid(60);

    // Each token is read against a `Period` of 60: a token this service issued names
    // the epoch second of a data point, which is always on the period grid.
    for token in [" ", "not-a-number", "3.5", "1e9", "90061", "60.0", "0x3c"] {
        let case = format!("NextToken {token:?}");
        emulator
            .call(
                "ListInsightsMetricData",
                &json!({
                    "EventSource": "iam.amazonaws.com",
                    "EventName": "AddUserToGroup",
                    "InsightType": "ApiCallRateInsight",
                    "Period": 60,
                    "DataType": "FillWithZeros",
                    "StartTime": end - 600,
                    "EndTime": end,
                    "NextToken": token,
                }),
            )
            .await
            .expect_error(&case, 400, "InvalidParameterException");
    }
}

/// `TrailName` is a bare name or a trail ARN, resolved through the trail registry.
#[tokio::test]
async fn the_trail_named_is_resolved_through_the_registry() {
    let emulator = Emulator::new();

    let created = emulator
        .call(
            "CreateTrail",
            &json!({
                "Name": "insights-data-trail",
                "S3BucketName": "insights-bucket",
                "IsMultiRegionTrail": true,
            }),
        )
        .await;
    let trail_arn = created.expect_ok("CreateTrail")["TrailARN"]
        .as_str()
        .expect("a created trail carries its ARN")
        .to_string();

    // (case, TrailName, Region to call from, expected TrailARN, expected error code)
    let rows: [TrailRow<'_>; 9] = [
        ("no trail named", None, HOME_REGION, None, ""),
        (
            "a bare name",
            Some("insights-data-trail"),
            HOME_REGION,
            Some(trail_arn.as_str()),
            "",
        ),
        (
            "the trail ARN",
            Some(trail_arn.as_str()),
            HOME_REGION,
            Some(trail_arn.as_str()),
            "",
        ),
        (
            // A trail ARN pins the trail's own Region, so a shadow Region reaches it.
            "the trail ARN from another Region",
            Some(trail_arn.as_str()),
            "eu-west-1",
            Some(trail_arn.as_str()),
            "",
        ),
        (
            // No trail of that name, so no Insights events and no ARN to report —
            // and no modelled not-found error on this operation.
            "a name no trail carries",
            Some("no-such-trail"),
            HOME_REGION,
            None,
            "",
        ),
        (
            "a name shorter than the minimum",
            Some("ab"),
            HOME_REGION,
            None,
            "InvalidTrailNameException",
        ),
        (
            "a name with adjacent dashes",
            Some("my--namespace"),
            HOME_REGION,
            None,
            "InvalidTrailNameException",
        ),
        (
            "a name in IP-address format",
            Some("192.168.5.4"),
            HOME_REGION,
            None,
            "InvalidTrailNameException",
        ),
        (
            // This operation models no `CloudTrailARNInvalidException`, so a
            // malformed ARN is refused as the name error it does model.
            "an ARN of another service",
            Some("arn:aws:s3:::insights-bucket"),
            HOME_REGION,
            None,
            "InvalidTrailNameException",
        ),
    ];

    for (case, trail_name, region, expected_arn, expected_error) in rows {
        let mut request = json!({
            "EventSource": "s3.amazonaws.com",
            "EventName": "PutObject",
            "InsightType": "ApiCallRateInsight",
        });
        if let Some(name) = trail_name {
            request["TrailName"] = json!(name);
        }
        let reply = emulator
            .call_in("ListInsightsMetricData", region, &request)
            .await;
        if expected_error.is_empty() {
            let body = reply.expect_ok(case).clone();
            assert_eq!(
                body.get("TrailARN").and_then(Value::as_str),
                expected_arn,
                "case {case}: expected TrailARN {expected_arn:?}, got {body:?}"
            );
        } else {
            reply.expect_error(case, 400, expected_error);
        }
    }

    // A bare name resolves in the Region the call was made in, so the trail is not
    // reachable by name from another Region — and that is not an error here either.
    let elsewhere = emulator
        .call_in(
            "ListInsightsMetricData",
            "eu-west-1",
            &json!({
                "EventSource": "s3.amazonaws.com",
                "EventName": "PutObject",
                "InsightType": "ApiCallRateInsight",
                "TrailName": "insights-data-trail",
            }),
        )
        .await;
    let body = elsewhere.expect_ok("a bare name from another Region").clone();
    assert!(
        body.get("TrailARN").is_none(),
        "case a bare name from another Region: a bare name names a trail in the \
         caller's own Region, got {body:?}"
    );
}

/// Reporting a series is a read: it changes nothing a later caller sees.
#[tokio::test]
async fn reporting_a_series_changes_nothing() {
    let emulator = Emulator::new();

    emulator
        .call(
            "CreateTrail",
            &json!({
                "Name": "insights-read-trail",
                "S3BucketName": "insights-bucket",
            }),
        )
        .await
        .expect_ok("CreateTrail");
    let before = emulator
        .call("GetTrail", &json!({"Name": "insights-read-trail"}))
        .await
        .expect_ok("GetTrail before")
        .clone();

    let now = now_secs();
    let end = now - now.rem_euclid(300);
    let request = json!({
        "EventSource": "dynamodb.amazonaws.com",
        "EventName": "PutItem",
        "InsightType": "ApiErrorRateInsight",
        "ErrorCode": "ProvisionedThroughputExceededException",
        "TrailName": "insights-read-trail",
        "Period": 300,
        "DataType": "FillWithZeros",
        "StartTime": end - 1_500,
        "EndTime": end,
    });

    let first = emulator
        .call("ListInsightsMetricData", &request)
        .await
        .expect_ok("the first report")
        .clone();
    let second = emulator
        .call("ListInsightsMetricData", &request)
        .await
        .expect_ok("the second report")
        .clone();
    assert_eq!(
        first, second,
        "case repeated report: the same request over the same window must report the \
         same series, got {first:?} then {second:?}"
    );
    assert_eq!(
        timestamps(&first).len(),
        5,
        "case repeated report: five 300-second periods span the 1500-second window, \
         got {first:?}"
    );

    let after = emulator
        .call("GetTrail", &json!({"Name": "insights-read-trail"}))
        .await
        .expect_ok("GetTrail after")
        .clone();
    assert_eq!(
        before, after,
        "case trail after the report: reporting a series must not touch the trail \
         registry, got {before:?} then {after:?}"
    );
}

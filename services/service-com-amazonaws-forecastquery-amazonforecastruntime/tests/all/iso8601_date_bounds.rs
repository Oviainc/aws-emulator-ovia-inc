// SPDX-License-Identifier: CC0-1.0

//! A query window's bounds are read as ISO 8601 INSTANTS, not as one spelling of one.
//!
//! `StartDate` and `EndDate` are documented as "Specify the date using this format:
//! yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format)": that layout is the example and ISO 8601 is the
//! format, so every spelling of the format names the instant its text declares and clips
//! the series on that instant. The forms the rule generalises over are the ones ISO 8601
//! adds to the example layout, and the ones the validation pattern this service enforces
//! admits:
//!
//! * the PRECISION of the time of day — through the minutes, through the seconds, or with
//!   a fractional second, which is truncated to the resolution the instant is kept at
//!   rather than refused;
//! * the CASE of the date/time designator (`T` or `t`) and of the UTC designator (`Z` or
//!   `z`);
//! * the ZONE — none at all, the UTC designator in either case, or a numeric offset,
//!   written in the `±hh:mm` form the AWS date type reads, of either sign and any
//!   magnitude.
//!
//! What stays refused is text the validation pattern admits and ISO 8601 has no reading
//! for: a calendar field out of range, a half-written field, a zone region in brackets, a
//! trailing word. That is the operation's own `InvalidInputException` (400), because such a
//! value gets past the shape layer and then names no instant.
//!
//! Both query operations are driven, through the real router as a client does, because each
//! reads its window through the same projection.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// The statistic the seeded forecast reports.
const STATISTIC: &str = "p50";

/// The seeded series: six-hourly points, so an offset of a few hours moves the window
/// across them and a bound is not forced onto a midnight boundary.
const TIMESTAMPS: [&str; 5] = [
    "2015-01-01T00:00:00",
    "2015-01-01T06:00:00",
    "2015-01-01T12:00:00",
    "2015-01-01T18:00:00",
    "2015-01-02T00:00:00",
];

/// The index in [`TIMESTAMPS`] of the point every spelling in
/// [`spellings_of_the_middle_point`] names.
const MIDDLE: usize = 2;

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // which instant a bound names, so it turns verification off rather than signing.
    router.set_request_authentication(false);
    router.register_service(forecastquery::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// An ARN of the shape the control plane mints.
fn arn(resource: &str) -> String {
    engine::emulator_arn("forecast", "us-east-1", resource)
}

/// One awsJson1_1 call: `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonForecastRuntime.{operation}"))
        // The routing key: the SigV4 credential scope's signing name (`forecast`).
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAEXAMPLEEXAMPLE/20150101/us-east-1/forecast/aws4_request, \
             SignedHeaders=content-type;host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize request body"),
        ))
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Message` of a failure response, in either casing the protocols use.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The timestamps the response reports for [`STATISTIC`], in the order they arrived.
fn reported_timestamps(body: &Value) -> Vec<String> {
    body.get("Forecast")
        .and_then(|forecast| forecast.get("Predictions"))
        .and_then(|predictions| predictions.get(STATISTIC))
        .and_then(Value::as_array)
        .map(|points| {
            points
                .iter()
                .map(|point| {
                    point["Timestamp"]
                        .as_str()
                        .expect("DataPoint.Timestamp must be a string")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A forecast with one series over [`TIMESTAMPS`].
fn seeded_forecast(forecast_arn: &str) -> StoredForecast {
    let series = StoredSeries::new([("item_id", "client_21")]).with_statistic(
        STATISTIC,
        TIMESTAMPS
            .iter()
            .enumerate()
            .map(|(step, timestamp)| StoredPoint::new(*timestamp, 10.0 + step as f64)),
    );
    StoredForecast::new(forecast_arn, "electricity_demand", "H", ["0.5"]).with_series(series)
}

/// The two operations, each with its ARN member and an ARN the store holds a forecast
/// under.
struct Op {
    name: &'static str,
    arn_member: &'static str,
    stored_arn: String,
}

fn operations() -> [Op; 2] {
    [
        Op {
            name: "QueryForecast",
            arn_member: "ForecastArn",
            stored_arn: arn("forecast/electricity_demand"),
        },
        Op {
            name: "QueryWhatIfForecast",
            arn_member: "WhatIfForecastArn",
            stored_arn: arn("what-if-forecast/electricity_analysis/price_cut"),
        },
    ]
}

/// Seed one forecast and one what-if forecast under the ARNs [`operations`] names, as the
/// control plane's create operations would.
fn seed(engine: &InMemoryEngine) {
    let [forecast, what_if] = operations();
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_forecast(seeded_forecast(&forecast.stored_arn));
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_what_if_forecast(
            seeded_forecast(&what_if.stored_arn).derived_from(&forecast.stored_arn),
        );
}

/// A request naming the stored resource of `op`, with `member` set to `value`.
fn request_with(op: &Op, member: &str, value: &str) -> Value {
    let mut request = json!({
        op.arn_member: op.stored_arn,
        "Filters": {"item_id": "client_21"},
    });
    request[member] = json!(value);
    request
}

/// Spellings of ONE instant — the point at [`MIDDLE`], 12:00 on the first day, read on the
/// scale the stored points are on.
///
/// `(what a failing assertion calls it, the text sent)`. The rows differ along the
/// precision of the time of day, the case of the two designators, and whether and how the
/// value declares a zone (none, the UTC designator, a numeric offset) with which sign and
/// magnitude.
fn spellings_of_the_middle_point() -> Vec<(&'static str, &'static str)> {
    vec![
        ("the documented example layout", "2015-01-01T12:00:00"),
        ("minute precision", "2015-01-01T12:00"),
        ("the UTC designator", "2015-01-01T12:00:00Z"),
        ("a lowercase UTC designator", "2015-01-01T12:00:00z"),
        ("a lowercase date/time designator", "2015-01-01t12:00:00"),
        ("a zero fractional second", "2015-01-01T12:00:00.000"),
        ("a fractional second and a zone", "2015-01-01T12:00:00.000Z"),
        (
            "a fractional second at the nanosecond resolution",
            "2015-01-01T12:00:00.000000000",
        ),
        (
            "a fractional second finer than the resolution kept",
            "2015-01-01T12:00:00.0000000000",
        ),
        ("a zero offset, hh:mm form", "2015-01-01T12:00:00+00:00"),
        ("a positive offset", "2015-01-01T13:00:00+01:00"),
        ("a negative offset", "2015-01-01T07:00:00-05:00"),
        ("an offset with minutes of its own", "2015-01-01T14:30:00+02:30"),
        ("minute precision with an offset", "2015-01-01T13:00+01:00"),
        // A zone shifts the CALENDAR DAY as well as the time of day: 02:00 on the 2nd at
        // +14:00 is 12:00 on the 1st in UTC.
        ("an offset that moves the date", "2015-01-02T02:00:00+14:00"),
    ]
}

/// Every ISO 8601 spelling of one instant clips the series at that instant — as a
/// `StartDate` and as an `EndDate`, for both operations.
#[tokio::test]
async fn every_iso8601_spelling_of_one_instant_clips_the_series_identically() {
    let (engine, router) = harness();
    seed(&engine);

    // The window is inclusive, so the bound's own point is kept on both sides.
    let from_the_middle: Vec<String> = TIMESTAMPS[MIDDLE..]
        .iter()
        .map(|timestamp| (*timestamp).to_string())
        .collect();
    let up_to_the_middle: Vec<String> = TIMESTAMPS[..=MIDDLE]
        .iter()
        .map(|timestamp| (*timestamp).to_string())
        .collect();

    for op in operations() {
        for (member, expected) in [
            ("StartDate", &from_the_middle),
            ("EndDate", &up_to_the_middle),
        ] {
            for (spelling, value) in spellings_of_the_middle_point() {
                let case = (op.name, member, spelling, value);
                let request = request_with(&op, member, value);
                let (status, body) = call(&router, op.name, &request).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {body}"
                );
                let reported = reported_timestamps(&body);
                assert_eq!(
                    &reported, expected,
                    "case {case:?}: expected {expected:?}, got {reported:?}"
                );
            }
        }
    }
}

/// The two bounds are read on the same scale, so a window written with a zone on one side
/// and none on the other is the window those two instants name.
#[tokio::test]
async fn a_window_whose_two_bounds_are_spelled_differently_is_still_one_window() {
    let (engine, router) = harness();
    seed(&engine);

    // (case, StartDate, EndDate, the timestamps kept)
    let windows: [(&str, &str, &str, &[&str]); 5] = [
        (
            "both bounds on the same instant, spelled differently",
            "2015-01-01T12:00:00Z",
            "2015-01-01T13:00:00+01:00",
            &[TIMESTAMPS[2]],
        ),
        (
            "a zoned start and a bare end",
            "2015-01-01T05:00:00-01:00",
            "2015-01-01T18:00:00",
            &[TIMESTAMPS[1], TIMESTAMPS[2], TIMESTAMPS[3]],
        ),
        (
            "a bare start and a minute-precision end",
            "2015-01-01T06:00:00",
            "2015-01-01T12:00",
            &[TIMESTAMPS[1], TIMESTAMPS[2]],
        ),
        (
            "a window the series starts after",
            "2014-12-01T00:00:00Z",
            "2014-12-31T23:59:59Z",
            &[],
        ),
        (
            "a window spanning the whole series",
            "2014-12-31T19:00:00-05:00",
            "2015-01-02T00:00:00.000Z",
            &TIMESTAMPS,
        ),
    ];

    for op in operations() {
        for (case_name, start, end, kept) in windows {
            let case = (op.name, case_name, start, end);
            let mut request = request_with(&op, "StartDate", start);
            request["EndDate"] = json!(end);
            let (status, body) = call(&router, op.name, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            let reported = reported_timestamps(&body);
            let expected: Vec<String> = kept.iter().map(|point| (*point).to_string()).collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }

    // An inverted window is still refused when its bounds are spelled differently: the
    // comparison is between instants, not between strings — `14:00Z` is later than
    // `13:00+01:00` (12:00Z) although it sorts earlier as text.
    for op in operations() {
        let mut request = request_with(&op, "StartDate", "2015-01-01T14:00:00Z");
        request["EndDate"] = json!("2015-01-01T13:00:00+01:00");
        let (status, body) = call(&router, op.name, &request).await;
        assert_eq!(
            status, 400,
            "case ({:?}, \"an inverted window across two zones\"): expected HTTP 400, got \
             {status} with {body}",
            op.name
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case ({:?}, \"an inverted window across two zones\"): expected \
             InvalidInputException, got {body}",
            op.name
        );
    }
}

/// A fractional second is part of the instant, not decoration: a bound a fraction of a
/// second past a point no longer includes it.
#[tokio::test]
async fn a_fractional_second_is_read_as_part_of_the_instant() {
    let (engine, router) = harness();
    seed(&engine);

    // (case, member, the bound, the timestamps kept)
    let rows: [(&str, &str, &str, &[&str]); 4] = [
        (
            "a start a thousandth past the middle point excludes it",
            "StartDate",
            "2015-01-01T12:00:00.001",
            &[TIMESTAMPS[3], TIMESTAMPS[4]],
        ),
        (
            "a start a thousandth before it keeps it",
            "StartDate",
            "2015-01-01T11:59:59.999",
            &[TIMESTAMPS[2], TIMESTAMPS[3], TIMESTAMPS[4]],
        ),
        (
            "an end a thousandth before the middle point excludes it",
            "EndDate",
            "2015-01-01T11:59:59.999",
            &[TIMESTAMPS[0], TIMESTAMPS[1]],
        ),
        (
            "an end a thousandth past it keeps it",
            "EndDate",
            "2015-01-01T12:00:00.001",
            &[TIMESTAMPS[0], TIMESTAMPS[1], TIMESTAMPS[2]],
        ),
    ];

    for op in operations() {
        for (case_name, member, value, kept) in rows {
            let case = (op.name, case_name, member, value);
            let (status, body) = call(&router, op.name, &request_with(&op, member, value)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            let reported = reported_timestamps(&body);
            let expected: Vec<String> = kept.iter().map(|point| (*point).to_string()).collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }
}

/// Text the validation pattern admits but ISO 8601 has no reading for names no instant: the
/// operation's own `InvalidInputException` (400), quoting the member and the value sent.
///
/// The rows differ along WHY there is no instant: a calendar field outside its range, a
/// field written short, a decimal point with no fraction, a zone outside its range or
/// written as a region, and a trailing word.
#[tokio::test]
async fn a_bound_the_pattern_admits_that_names_no_instant_is_refused() {
    let (engine, router) = harness();
    seed(&engine);

    let no_instant: [(&str, &str); 12] = [
        ("a month past 12", "2015-13-01T12:00:00"),
        ("a day February never has", "2015-02-30T12:00:00"),
        ("an hour past 23", "2015-01-01T25:00:00"),
        ("a minute past 59", "2015-01-01T12:60"),
        ("a seconds field written short", "2015-01-01T12:00:0"),
        ("a decimal point with no fraction", "2015-01-01T12:00:00."),
        // ISO 8601 puts the fraction on the LAST field present, and the layout this
        // service's points are stored in carries seconds, so a fraction with no seconds
        // field names no instant it can place on its timeline.
        ("a fraction with no seconds field", "2015-01-01T12:00.5"),
        ("an offset past its range", "2015-01-01T12:00:00+24:00"),
        // The zone forms read are the ones the AWS date type reads: the designator and a
        // numeric offset written `±hh:mm`. The basic and hour-only forms carry no colon,
        // so they are text that names no instant here rather than a window.
        ("an offset in the basic form", "2015-01-01T12:00:00+0100"),
        ("an hour-only offset", "2015-01-01T12:00:00-08"),
        ("a zone region in brackets", "2015-01-01T12:00:00Z[UTC]"),
        ("a trailing word", "2015-01-01T12:00:00tomorrow"),
    ];

    for op in operations() {
        for member in ["StartDate", "EndDate"] {
            for (reason, value) in no_instant {
                let case = (op.name, member, reason, value);
                let request = request_with(&op, member, value);
                let (status, body) = call(&router, op.name, &request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidInputException",
                    "case {case:?}: expected InvalidInputException, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!(
                        "{member} '{value}' is not a valid date; specify the date using this \
                         format: yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format)"
                    ),
                    "case {case:?}: the refusal must name the member and the value sent, got \
                     {body}"
                );
            }
        }
    }
}

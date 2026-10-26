// SPDX-License-Identifier: CC0-1.0

//! The documented date layout, read through ONE positional walk.
//!
//! Two layers of this crate judge a `StartDate`/`EndDate`: the shape layer, which refuses
//! a value that is not a calendar date and a time of day at all before anything is
//! resolved, and the projection, which needs the instant a date names in order to clip a
//! series. Both read the SAME documented layout — `yyyy-MM-dd`, a date/time separator,
//! `HH:mm[:ss]` — so the positions that layout fixes (`-` at index 4 and 7, the separator
//! at index 10, `:` at index 13 and 16, an ASCII digit everywhere else) are read in one
//! place, and the two layers differ only where the documentation makes them differ: the
//! shape layer stops after the minutes and admits the lowercase separator its pattern's
//! `[Tt]` does, while the instant the projection needs is the full `yyyy-MM-dd'T'HH:mm:ss`.
//!
//! What that buys, asserted here over a set of values rather than one request:
//!
//! 1. A value broken at a position BOTH layers read is refused by the first of them —
//!    HTTP 400 `ValidationException` — and never reaches the projection, for every
//!    operation and both date members.
//! 2. A value the shared positions admit but the stricter, longer layout does not gets
//!    PAST the shape layer and is the operation's own `InvalidInputException` (400), which
//!    is the only way the two layers are allowed to disagree.
//! 3. A value in the documented layout is read by both layers and answers from the store:
//!    the window it names clips the stored points.
//!
//! Every row drives the service the way a client does: an awsJson1_1 `POST /` through the
//! real router, so the shape layer, the handler and the projection all run. Both
//! operations are driven, because each reads a date through both layers.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// The statistic the seeded forecast reports, and the timestamps its points carry — the
/// series every window row below clips.
const STATISTIC: &str = "p50";
const TIMESTAMPS: [&str; 3] = [
    "2015-01-01T00:00:00",
    "2015-01-02T00:00:00",
    "2015-01-03T00:00:00",
];

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // which values are read as dates, so it turns verification off rather than signing.
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
            .map(|(day, timestamp)| StoredPoint::new(*timestamp, 10.0 + day as f64)),
    );
    StoredForecast::new(forecast_arn, "electricity_demand", "D", ["0.5"]).with_series(series)
}

/// The two operations, each with its ARN member and an ARN the store holds a forecast
/// under — one per former copy of the layout walk, since each handler reads a date through
/// both layers.
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

/// Rule 1: a value broken at a position both layers read is refused by the shape layer.
///
/// The rows differ along WHICH shared position is broken — each fixed separator of the
/// layout, the date/time separator, a digit field, and the ASCII-only decision — and all
/// but the last are 19 bytes long, so the only thing a row changes is the one position.
#[tokio::test]
async fn a_value_broken_at_a_shared_layout_position_is_refused_by_the_shape_layer() {
    let (engine, router) = harness();
    seed(&engine);

    let broken: [(&str, &str); 7] = [
        ("the '-' at index 4", "2015+01-01T00:00:00"),
        ("the '-' at index 7", "2015-01+01T00:00:00"),
        ("a space where the date/time separator belongs", "2015-01-01 00:00:00"),
        ("a letter where the date/time separator belongs", "2015-01-01X00:00:00"),
        ("the ':' at index 13", "2015-01-01T00-00:00"),
        ("a letter inside a digit field", "20x5-01-01T00:00:00"),
        // The layout is ASCII-only, as the shape pattern's `\d` is: digits of another
        // script are not a date this service can place on a timeline.
        ("Arabic-Indic digits", "٢٠١٥-٠١-٠١T٠٠:٠٠:٠٠"),
    ];

    for op in operations() {
        for member in ["StartDate", "EndDate"] {
            for (position, value) in broken {
                let case = (op.name, member, position, value);
                let (status, body) =
                    call(&router, op.name, &request_with(&op, member, value)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: a shared position must be refused by the shape layer, \
                     got {body}"
                );
            }
        }
    }
}

/// Rule 2: a value the shared positions admit, but the stricter instant layout does not,
/// gets past the shape layer and is the operation's own `InvalidInputException`.
///
/// These are the only two differences between the layers — how far the layout must reach,
/// and whether a lowercase date/time separator is admitted — so each row is a value that
/// one layer reads as a date and the other does not.
#[tokio::test]
async fn a_value_only_the_stricter_layout_refuses_reaches_the_projection() {
    let (engine, router) = harness();
    seed(&engine);

    let past_the_shape_layer: [(&str, &str); 4] = [
        ("no seconds field", "2015-01-01T08:00"),
        ("a lowercase date/time separator", "2015-01-01t08:00:00"),
        ("the wrong separator at index 16", "2015-01-01T08:00-00"),
        ("a zone designator the instant layout omits", "2015-01-01T08:00:00Z"),
    ];

    for op in operations() {
        for member in ["StartDate", "EndDate"] {
            for (difference, value) in past_the_shape_layer {
                let case = (op.name, member, difference, value);
                let (status, body) =
                    call(&router, op.name, &request_with(&op, member, value)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidInputException",
                    "case {case:?}: the shape layer must admit this spelling and the \
                     projection refuse it, got {body}"
                );
            }
        }
    }
}

/// Rule 3: a value in the documented layout is read by BOTH layers — it passes the shape
/// layer and the window it names clips the stored series.
///
/// The rows differ along which bounds the window carries and where they fall against the
/// stored points: no bounds, each bound alone, both on one point, a window past the series,
/// and a leap day (a date only a calendar check can judge).
#[tokio::test]
async fn a_value_in_the_documented_layout_is_read_by_both_layers() {
    let (engine, router) = harness();
    seed(&engine);

    struct Window {
        name: &'static str,
        start: Option<&'static str>,
        end: Option<&'static str>,
        kept: Vec<&'static str>,
    }

    let windows = vec![
        Window {
            name: "no bounds: the complete date range",
            start: None,
            end: None,
            kept: TIMESTAMPS.to_vec(),
        },
        Window {
            name: "a start on the second point",
            start: Some(TIMESTAMPS[1]),
            end: None,
            kept: vec![TIMESTAMPS[1], TIMESTAMPS[2]],
        },
        Window {
            name: "an end on the second point",
            start: None,
            end: Some(TIMESTAMPS[1]),
            kept: vec![TIMESTAMPS[0], TIMESTAMPS[1]],
        },
        Window {
            name: "both bounds on one point",
            start: Some(TIMESTAMPS[2]),
            end: Some(TIMESTAMPS[2]),
            kept: vec![TIMESTAMPS[2]],
        },
        Window {
            name: "a window the series ends before",
            start: Some("2015-02-01T00:00:00"),
            end: None,
            kept: Vec::new(),
        },
        Window {
            name: "a leap day",
            start: Some("2016-02-29T00:00:00"),
            end: None,
            kept: Vec::new(),
        },
    ];

    for op in operations() {
        for window in &windows {
            let case = (op.name, window.name);
            let mut request = json!({
                op.arn_member: op.stored_arn,
                "Filters": {"item_id": "client_21"},
            });
            if let Some(start) = window.start {
                request["StartDate"] = json!(start);
            }
            if let Some(end) = window.end {
                request["EndDate"] = json!(end);
            }
            let (status, body) = call(&router, op.name, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            assert_eq!(
                reported_timestamps(&body),
                window.kept,
                "case {case:?}: expected {:?}, got {:?}",
                window.kept,
                reported_timestamps(&body)
            );
        }
    }
}

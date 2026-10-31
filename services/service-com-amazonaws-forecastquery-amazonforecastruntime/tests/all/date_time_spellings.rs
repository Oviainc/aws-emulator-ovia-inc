// SPDX-License-Identifier: CC0-1.0

//! The ISO 8601 spelling a STORED point's `Timestamp` is written in names its instant.
//!
//! `iso8601_date_bounds.rs` pins which spellings a query's `StartDate`/`EndDate` names an
//! instant in. The projection reads the OTHER side of that comparison through the very same
//! reading (`crate::state::parse_date_time`, over `DataPoint.Timestamp` as the control
//! plane stored it), so what this suite pins is the consequence only the stored side can
//! show: a point sits at the instant its own text declares, whichever spelling that text
//! uses, so a window written in a different spelling clips it, and the series comes back in
//! INSTANT order however the points were ordered or spelled in the store. Each point is
//! reported with the text it was stored with — the projection PLACES a point, it does not
//! rewrite it.
//!
//! A stored timestamp that names no instant at all is the subject of
//! `stored_point_timestamps.rs`.
//!
//! Both operations are driven, through the real router as a client does, because each
//! projects its series through the same shared code.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// The statistic the seeded forecasts report.
const STATISTIC: &str = "p50";

/// Points a few hours apart on one day, each written in a DIFFERENT ISO 8601 spelling and
/// listed here in the order of the instants they name: the documented layout, minute
/// precision, the UTC designator, a numeric offset (19:00 at +01:00 is 18:00 on the scale
/// the others are on), and a lowercase date/time designator with a fractional second.
const SPELLED_TIMESTAMPS: [&str; 5] = [
    "2015-01-01T00:00:00",
    "2015-01-01T06:00",
    "2015-01-01T12:00:00Z",
    "2015-01-01T19:00:00+01:00",
    "2015-01-01t23:00:00.500",
];

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // which instant a stored point sits at, so it turns verification off rather than
    // signing.
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

/// A forecast whose one series carries `timestamps`, in the order given.
fn forecast_with(forecast_arn: &str, timestamps: &[&str]) -> StoredForecast {
    let series = StoredSeries::new([("item_id", "client_21")]).with_statistic(
        STATISTIC,
        timestamps
            .iter()
            .enumerate()
            .map(|(step, timestamp)| StoredPoint::new(*timestamp, 10.0 + step as f64)),
    );
    StoredForecast::new(forecast_arn, "electricity_demand", "H", ["0.5"]).with_series(series)
}

/// One of the two query operations: which ARN member it reads, and which store it answers
/// from.
struct Op {
    name: &'static str,
    arn_member: &'static str,
    what_if: bool,
}

const OPERATIONS: [Op; 2] = [
    Op {
        name: "QueryForecast",
        arn_member: "ForecastArn",
        what_if: false,
    },
    Op {
        name: "QueryWhatIfForecast",
        arn_member: "WhatIfForecastArn",
        what_if: true,
    },
];

impl Op {
    /// Persist a forecast carrying `timestamps` under an ARN this operation can resolve, as
    /// the control plane's create operations would, and answer that ARN.
    fn seed(&self, engine: &InMemoryEngine, name: &str, timestamps: &[&str]) -> String {
        let resource_arn = if self.what_if {
            arn(&format!("what-if-forecast/{name}/scenario"))
        } else {
            arn(&format!("forecast/{name}"))
        };
        let forecast = forecast_with(&resource_arn, timestamps);
        let store = engine.state::<ForecastQueryState>();
        let mut state = store.lock();
        if self.what_if {
            state.put_what_if_forecast(forecast.derived_from(arn("forecast/base")));
        } else {
            state.put_forecast(forecast);
        }
        resource_arn
    }

    /// A request naming `resource_arn`, carrying the window members in `window`.
    fn request(&self, resource_arn: &str, window: &[(&str, &str)]) -> Value {
        let mut request = json!({
            self.arn_member: resource_arn,
            "Filters": {"item_id": "client_21"},
        });
        for (member, value) in window {
            request[*member] = json!(value);
        }
        request
    }
}

/// One row of the window table: what a failing assertion calls it, the window members sent,
/// and the stored spellings the window keeps.
type WindowCase = (&'static str, &'static [(&'static str, &'static str)], &'static [&'static str]);

/// A point sits at the instant its own spelling names, so a window written in another
/// spelling clips it and the series is reported in instant order.
///
/// The rows differ along which bound the window carries and how it is written against the
/// point it lands on: a bare bound against a zoned point, a zoned bound against a point
/// written with an offset, a minute-precision bound, a bound inside the fractional second
/// of the last point, and a window whose two bounds are spelled differently from each
/// other.
#[tokio::test]
async fn a_stored_point_sits_at_the_instant_its_own_spelling_names() {
    let (engine, router) = harness();

    // Stored OUT of instant order — the later points first — so the reported order cannot
    // have come from the stored order.
    let stored: Vec<&str> = SPELLED_TIMESTAMPS[3..]
        .iter()
        .chain(SPELLED_TIMESTAMPS[..3].iter())
        .copied()
        .collect();

    let windows: [WindowCase; 6] = [
        (
            "no bounds: the whole series, in instant order",
            &[],
            &SPELLED_TIMESTAMPS,
        ),
        (
            // 12:00 with no zone, against the point written with the designator.
            "a bare start on the designator-spelled point",
            &[("StartDate", "2015-01-01T12:00:00")],
            &[
                SPELLED_TIMESTAMPS[2],
                SPELLED_TIMESTAMPS[3],
                SPELLED_TIMESTAMPS[4],
            ],
        ),
        (
            // 18:00 reached through an offset, against the point written at +01:00.
            "a zoned start on the offset-spelled point",
            &[("StartDate", "2015-01-01T13:00:00-05:00")],
            &[SPELLED_TIMESTAMPS[3], SPELLED_TIMESTAMPS[4]],
        ),
        (
            "a minute-precision end on the minute-precision point",
            &[("EndDate", "2015-01-01T06:00")],
            &[SPELLED_TIMESTAMPS[0], SPELLED_TIMESTAMPS[1]],
        ),
        (
            // The last point is half a second past 23:00, so a bound AT 23:00 is before it.
            "an end on the second the fractional point falls in",
            &[("EndDate", "2015-01-01T23:00:00Z")],
            &SPELLED_TIMESTAMPS[..4],
        ),
        (
            "a window between two spellings of the same scale",
            &[
                ("StartDate", "2015-01-01T06:00:00"),
                ("EndDate", "2015-01-01T19:00:00+01:00"),
            ],
            &[
                SPELLED_TIMESTAMPS[1],
                SPELLED_TIMESTAMPS[2],
                SPELLED_TIMESTAMPS[3],
            ],
        ),
    ];

    for op in &OPERATIONS {
        let resource_arn = op.seed(&engine, "spelled_points", &stored);
        for (case_name, window, kept) in windows {
            let case = (op.name, case_name);
            let (status, body) = call(&router, op.name, &op.request(&resource_arn, window)).await;
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

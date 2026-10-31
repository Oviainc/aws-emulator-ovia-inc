// SPDX-License-Identifier: CC0-1.0

//! A STORED point whose `Timestamp` names no instant is skipped, never reported.
//!
//! The projection has to place every point of the matched series on one timeline in order
//! to clip and order it (`crate::state::parse_date_time`, the same reading a query's
//! window bounds get — `iso8601_date_bounds.rs` pins which spellings that is, and
//! `date_time_spellings.rs` that a stored point sits at the instant its own spelling
//! names). A stored timestamp the reading cannot place is therefore the one case the
//! projection has to answer for on its own: it is SKIPPED, the rest of the series is still
//! reported, and nothing is invented in its place — whatever bytes that timestamp carries,
//! and whether or not the request asked for a window.
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

/// The one stored point this suite's series carries that names an instant.
const READABLE: &str = "2015-01-01T12:00:00";

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // which stored points can be placed on a timeline, so it turns verification off rather
    // than signing.
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

/// Every stored point the reading cannot place is skipped, and the one it can is still
/// reported — with no window, and with windows that would otherwise span the unplaceable
/// points.
///
/// The stored rows differ along WHY the text names no instant: a field written short, a
/// fraction on the minutes field, a zone written as a region, a non-ASCII character where
/// the zone belongs, no date at all, and a calendar that has no such day.
#[tokio::test]
async fn a_stored_point_naming_no_instant_is_skipped() {
    let (engine, router) = harness();

    let names_no_instant = [
        "2015-01-01T00:00:0",
        "2015-01-01T00:00.500",
        "2015-01-01T00:00:00Z[UTC]",
        "2015-01-01T00:00+a\u{20ac}",
        "whenever",
        "2015-02-30T00:00:00",
    ];
    let mut stored = vec![READABLE];
    stored.extend(names_no_instant);

    // (case, the window sent)
    let windows: [(&str, &[(&str, &str)]); 3] = [
        ("no bounds: the complete date range", &[]),
        (
            "a start before every stored point",
            &[("StartDate", "2015-01-01T00:00Z")],
        ),
        (
            "a window around the point that names an instant",
            &[
                ("StartDate", "2015-01-01T11:00:00"),
                ("EndDate", "2015-01-01T13:00:00+00:00"),
            ],
        ),
    ];

    for op in &OPERATIONS {
        let resource_arn = op.seed(&engine, "unreadable_points", &stored);
        for (case_name, window) in windows {
            let case = (op.name, case_name);
            let (status, body) = call(&router, op.name, &op.request(&resource_arn, window)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            let reported = reported_timestamps(&body);
            assert_eq!(
                reported,
                vec![READABLE.to_string()],
                "case {case:?}: expected only the point that names an instant, got {reported:?}"
            );
        }
    }
}

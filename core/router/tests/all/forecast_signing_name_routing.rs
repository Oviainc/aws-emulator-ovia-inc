// SPDX-License-Identifier: CC0-1.0

//! Amazon Forecast (the control plane) and Amazon Forecast Runtime (`forecastquery`, the
//! query data plane) are TWO services behind ONE SigV4 signing name, `forecast`: their
//! models share `aws.auth#sigv4 { name: "forecast" }` while declaring different
//! `endpointPrefix`es (`forecast` and `forecastquery`). No Smithy trait expresses that
//! sharing, and the router resolves a request's routing key from the SigV4 credential
//! scope first, so with both mounted the default signing-name `can_handle` makes BOTH
//! claim every Forecast-signed request and the router answers `500 AmbiguousService` for
//! all of it — neither service is reachable, and `AmbiguousService` is not a shape either
//! model declares.
//!
//! Both are `awsJson1_1`, so both post every operation to `/` and a path test cannot
//! separate them: what does is the service each request NAMES in `X-Amz-Target`
//! (`AmazonForecast.<Op>` vs `AmazonForecastRuntime.<Op>`), which is also how real AWS
//! tells the two endpoints' traffic apart.
//!
//! This suite needs the WHOLE router: the ambiguity only exists when both services are
//! mounted, so a single-service test cannot see it. Each row is answered by the ONE
//! service that owns it — a success computed from state, or that service's own modelled
//! error — and never by a routing failure.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use service_com_amazonaws_forecastquery_amazonforecastruntime::state::{
    StoredForecast, StoredPoint, StoredSeries,
};
use service_com_amazonaws_forecastquery_amazonforecastruntime::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming `forecast` — what BOTH services' requests are signed
/// with, and therefore the whole problem. The signature itself is never checked: the
/// router here runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/forecast/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=deadbeef";

/// The forecast the query service answers from, as `CreateForecast` would have minted it.
const FORECAST_ARN: &str = "arn:aws:forecast:us-east-1:000000000000:forecast/routing";

/// A well-formed Forecast ARN nothing is stored under.
const ABSENT_FORECAST_ARN: &str = "arn:aws:forecast:us-east-1:000000000000:forecast/absent";

/// A well-formed what-if forecast ARN nothing is stored under.
const ABSENT_WHAT_IF_ARN: &str =
    "arn:aws:forecast:us-east-1:000000000000:what-if-forecast/absent/variant";

/// The one stored point's timestamp and value: what a 200 must report, so the answer is a
/// projection of state rather than anything the router invented.
const POINT_TIMESTAMP: &str = "2015-01-01T00:00:00";
/// The stored value of that point.
const POINT_VALUE: f64 = 42.0;

/// One request, and the answer its OWNING service gives.
struct Case {
    /// How the row reads in an assertion message.
    name: &'static str,
    /// The `X-Amz-Target` that names the service and operation.
    target: &'static str,
    /// The awsJson1_1 request body.
    body: String,
    /// The HTTP status the owning service answers with.
    status: u16,
    /// The wire error code it answers with, or `None` for a success.
    code: Option<&'static str>,
}

/// The wire error code of a response body: `__type` for the awsJson protocols (and for
/// the router's own refusals), with any namespace stripped.
fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|json| {
            json.get("__type")
                .and_then(serde_json::Value::as_str)
                .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        })
}

/// One awsJson1_1 `POST /` naming `target`, as both services' clients send it.
fn request(target: &str, body: String) -> ::http::Request<::bytes::Bytes> {
    ::http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .body(::bytes::Bytes::from(body))
        .expect("a well-formed request")
}

/// The body of the query that reads the seeded forecast back.
fn seeded_query_body() -> String {
    format!(r#"{{"ForecastArn":"{FORECAST_ARN}","Filters":{{"item_id":"client_21"}}}}"#)
}

#[test]
fn forecast_and_forecastquery_route_apart_under_one_signing_name() {
    std::thread::Builder::new()
        .name("forecast-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            // The query service holds forecasts the CONTROL plane produces; seeding one
            // directly stands in for `CreateForecast` (still scaffolded) and is what lets
            // a 200 prove the request reached the query service's handler.
            engine
                .state::<ForecastQueryState>()
                .lock()
                .put_forecast(
                    StoredForecast::new(FORECAST_ARN, "routing", "D", ["0.5"]).with_series(
                        StoredSeries::new([("item_id", "client_21")]).with_statistic(
                            "p50",
                            vec![StoredPoint::new(POINT_TIMESTAMP, POINT_VALUE)],
                        ),
                    ),
                );

            let mut router = build_router(Arc::clone(&engine));
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The rows span BOTH services and, within the query service, the three answers
            // that distinguish a served request from a refused one: a success computed
            // from state, resource resolution (404) and ARN syntax (400). The control-plane
            // row is an operation only IT can serve — the query service's routing table
            // holds no `ListDatasets` — so a 200 there can only come from the control
            // plane, and neither routing failure (500 `AmbiguousService`, 404
            // `UnknownService`) is a 200 either.
            let cases: [Case; 5] = [
                Case {
                    name: "the query service answers a stored forecast",
                    target: "AmazonForecastRuntime.QueryForecast",
                    body: seeded_query_body(),
                    status: 200,
                    code: None,
                },
                Case {
                    name: "the query service reports a forecast it holds nothing for",
                    target: "AmazonForecastRuntime.QueryForecast",
                    body: format!(
                        r#"{{"ForecastArn":"{ABSENT_FORECAST_ARN}","Filters":{{"item_id":"client_21"}}}}"#
                    ),
                    status: 404,
                    code: Some("ResourceNotFoundException"),
                },
                Case {
                    name: "the query service refuses an ARN that is not an ARN",
                    target: "AmazonForecastRuntime.QueryForecast",
                    body: r#"{"ForecastArn":"definitely-not-an-arn","Filters":{"item_id":"client_21"}}"#
                        .to_string(),
                    status: 400,
                    code: Some("InvalidInputException"),
                },
                Case {
                    name: "the query service's what-if operation is reachable too",
                    target: "AmazonForecastRuntime.QueryWhatIfForecast",
                    body: format!(
                        r#"{{"WhatIfForecastArn":"{ABSENT_WHAT_IF_ARN}","Filters":{{"item_id":"client_21"}}}}"#
                    ),
                    status: 404,
                    code: Some("ResourceNotFoundException"),
                },
                Case {
                    name: "the control plane serves its own operation",
                    target: "AmazonForecast.ListDatasets",
                    body: "{}".to_string(),
                    status: 200,
                    code: None,
                },
            ];

            for case in cases {
                let Case {
                    name,
                    target,
                    body,
                    status: expected_status,
                    code: expected_code,
                } = case;
                let response = runtime.block_on(router.dispatch(request(target, body)));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let code = error_code(&rendered);
                assert!(
                    code.as_deref() != Some("AmbiguousService")
                        && code.as_deref() != Some("UnknownService"),
                    "case {name:?} ({target}): the request must reach exactly one service, \
                     got {status} with {rendered}"
                );
                assert_eq!(
                    (status, code.as_deref()),
                    (expected_status, expected_code),
                    "case {name:?} ({target}): expected HTTP {expected_status} \
                     {expected_code:?} from the one service that owns it, got {status} \
                     {code:?} with {rendered}"
                );
            }

            // The successful query's answer is the SEEDED series, so that request reached
            // the query service's handler and read this engine's state.
            let response = runtime.block_on(router.dispatch(request(
                "AmazonForecastRuntime.QueryForecast",
                seeded_query_body(),
            )));
            let rendered = String::from_utf8_lossy(response.body()).to_string();
            let parsed: serde_json::Value =
                serde_json::from_str(&rendered).unwrap_or(serde_json::Value::Null);
            let points = parsed
                .get("Forecast")
                .and_then(|forecast| forecast.get("Predictions"))
                .and_then(|predictions| predictions.get("p50"))
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("no Forecast.Predictions.p50 in {rendered}"))
                .clone();
            assert_eq!(
                points.len(),
                1,
                "the query service must answer the one seeded point, got {rendered}"
            );
            assert_eq!(
                points[0].get("Timestamp").and_then(serde_json::Value::as_str),
                Some(POINT_TIMESTAMP),
                "the answer must carry the stored timestamp, got {rendered}"
            );
            assert_eq!(
                points[0].get("Value").and_then(serde_json::Value::as_f64),
                Some(POINT_VALUE),
                "the answer must carry the stored value, got {rendered}"
            );
        })
        .expect("spawn the routing thread")
        .join()
        .expect("Forecast and forecastquery must route apart");
}

// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS, where naming decides and nothing else does.
//!
//! `forecastquery` (Amazon Forecast Runtime, endpoint prefix `forecastquery`) signs as
//! `forecast` — the signing name it shares with the Amazon Forecast control plane
//! (`smithy_query(query="service_info")`: `sigv4Name: forecast` on both, `endpointPrefix`
//! `forecastquery` vs `forecast`). The router resolves a request's routing key from the
//! SigV4 credential scope, so the signing name alone cannot tell the two apart and a
//! service claiming on it alone claims its peer's traffic as well — which the router
//! answers `500 AmbiguousService`, a code no AWS model declares.
//!
//! > A service whose signing name is shared claims a request only when the request
//! > addresses its OWN modelled wire surface. For an RPC protocol — this model is
//! > `awsJson1_1`, where every operation posts to `/` — that surface is named out of
//! > band, in `X-Amz-Target: <Service>.<Operation>`, so the service the target NAMES must
//! > be this one.
//!
//! `forecastquery.rs`'s own `only_requests_naming_this_service_are_claimed` pins the base
//! table (its own target, the versioned spelling, two control-plane targets, a third
//! service's). The rows here are the ones that separate NAMING from everything else a
//! request carries: the peer's prefix on an operation name THIS service has, a target
//! whose letter case differs, this service's target signed for another service, and no
//! target at all. With only this service mounted, a request it declines reaches no
//! service and the router answers `404 UnknownService`.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// The ARN of the forecast every claimed row queries.
fn forecast_arn() -> String {
    engine::emulator_arn("forecast", "us-east-1", "forecast/electricity_demand")
}

/// The ARN of the what-if forecast the `QueryWhatIfForecast` row queries.
fn what_if_arn() -> String {
    engine::emulator_arn(
        "forecast",
        "us-east-1",
        "what-if-forecast/electricity_analysis/price_cut",
    )
}

/// A router carrying only this service, with one forecast and one what-if forecast stored
/// so a claimed request has something to answer with.
fn harness() -> MultiServiceRouter {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let series = StoredSeries::new([("item_id", "client_21")]).with_statistic(
        "p50",
        [StoredPoint::new("2015-01-01T00:00:00", 42.0)],
    );
    {
        let state = engine.state::<ForecastQueryState>();
        let mut state = state.lock();
        state.put_forecast(
            StoredForecast::new(forecast_arn(), "electricity_demand", "D", ["0.5"])
                .with_series(series.clone()),
        );
        state.put_what_if_forecast(
            StoredForecast::new(what_if_arn(), "price_cut", "D", ["0.5"])
                .with_series(series)
                .derived_from(forecast_arn()),
        );
    }
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // which service claims a request, so it turns verification off.
    router.set_request_authentication(false);
    router.register_service(forecastquery::EmulatedService::new(engine));
    router
}

/// One request, and whether this service claims it.
struct Case {
    /// How the row reads in an assertion message.
    name: &'static str,
    /// The signing name in the request's SigV4 credential scope.
    signing_name: &'static str,
    /// The `X-Amz-Target` header, if the request carries one.
    target: Option<&'static str>,
    /// The request body.
    body: Value,
    /// Whether this service must serve the request.
    claimed: bool,
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

#[tokio::test]
async fn claiming_keys_on_the_service_the_target_names_and_nothing_else() {
    let router = harness();
    let query_forecast = json!({
        "ForecastArn": forecast_arn(),
        "Filters": {"item_id": "client_21"},
    });
    let query_what_if = json!({
        "WhatIfForecastArn": what_if_arn(),
        "Filters": {"item_id": "client_21"},
    });

    let cases: Vec<Case> = vec![
        Case {
            name: "its own target, QueryWhatIfForecast",
            signing_name: "forecast",
            target: Some("AmazonForecastRuntime.QueryWhatIfForecast"),
            body: query_what_if,
            claimed: true,
        },
        Case {
            // The framework's target reader lower-cases the service it reads, so the same
            // service is named however the client spelled it.
            name: "its own target in another letter case",
            signing_name: "forecast",
            target: Some("amazonforecastruntime.QueryForecast"),
            body: query_forecast.clone(),
            claimed: true,
        },
        Case {
            // The SERVICE the target names decides, never the operation name: an
            // operation name this service does know, under the peer's service prefix, is
            // still not this service's traffic.
            name: "the control plane's prefix on this service's operation name",
            signing_name: "forecast",
            target: Some("AmazonForecast.QueryForecast"),
            body: query_forecast.clone(),
            claimed: false,
        },
        Case {
            name: "no target at all",
            signing_name: "forecast",
            target: None,
            body: query_forecast.clone(),
            claimed: false,
        },
        Case {
            name: "its own target, signed for another service",
            signing_name: "s3",
            target: Some("AmazonForecastRuntime.QueryForecast"),
            body: query_forecast.clone(),
            claimed: false,
        },
    ];

    for case in cases {
        let Case {
            name,
            signing_name,
            target,
            body,
            claimed,
        } = case;
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAEXAMPLEEXAMPLE/20150101/us-east-1/{signing_name}/aws4_request, \
                     SignedHeaders=content-type;host;x-amz-target, Signature=0"
                ),
            );
        if let Some(target) = target {
            request = request.header("x-amz-target", target);
        }
        let request = request
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("serialize request body"),
            ))
            .expect("build request");

        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        let code = error_code(&parsed);
        assert_ne!(
            code, "AmbiguousService",
            "case {name:?}: a claim must never be shared, got {status} with {parsed}"
        );
        if claimed {
            assert_eq!(
                status, 200,
                "case {name:?}: this service must serve the request, got {status} with \
                 {parsed}"
            );
            assert!(
                parsed.pointer("/Forecast/Predictions/p50").is_some(),
                "case {name:?}: expected the stored series, got {parsed}"
            );
        } else {
            assert_eq!(
                code, "UnknownService",
                "case {name:?}: this service must not claim the request, got {status} with \
                 {parsed}"
            );
        }
    }
}

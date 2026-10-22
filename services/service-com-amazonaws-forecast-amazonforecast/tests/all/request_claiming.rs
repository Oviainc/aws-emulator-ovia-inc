// SPDX-License-Identifier: CC0-1.0

//! Which requests the Amazon Forecast control plane CLAIMS.
//!
//! This service's SigV4 signing name, `forecast`, is SHARED with Amazon Forecast Runtime
//! (`forecastquery`, the query data plane): both models carry
//! `aws.auth#sigv4 { name: "forecast" }` and differ in `endpointPrefix` (`forecast` vs
//! `forecastquery`). The router resolves a routing key from the SigV4 credential scope
//! first, so claiming on the signing name alone makes this service claim the query
//! service's traffic too — and a request two services claim is answered by NEITHER (the
//! router refuses an ambiguous claim).
//!
//! Both services are `awsJson1_1` and post every operation to `/`, so the discriminator
//! is the service each request NAMES in `X-Amz-Target`. These tests mount ONLY this
//! service, so a request it declines is answered `404 UnknownService` by the router: that
//! is exactly the evidence wanted here — this service does not claim the peer's traffic —
//! while the full two-service split is pinned in
//! `core/router/tests/all/forecast_signing_name_routing.rs`.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::Value;
use service_com_amazonaws_forecast_amazonforecast as forecast;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming `forecast` — the signing name BOTH services use. The
/// signature is never checked: this router runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/forecast/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=deadbeef";

/// A router carrying only this service.
fn harness() -> MultiServiceRouter {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(forecast::EmulatedService::new(engine));
    router
}

/// One awsJson1_1 call: `(http status, wire error code or `None`)`.
async fn call(router: &MultiServiceRouter, target: &str, body: &str) -> (u16, Option<String>) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let rendered = String::from_utf8_lossy(response.body()).to_string();
    let code = serde_json::from_str::<Value>(&rendered)
        .ok()
        .and_then(|json| {
            json.get("__type")
                .and_then(Value::as_str)
                // `__type` may be namespaced (`com.amazonaws.forecast#Foo`).
                .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        });
    (status, code)
}

/// A request that names THIS service is served; one that names the query service sharing
/// the signing name is not claimed at all.
///
/// The rows differ along the service the request names, and — for this service's own
/// traffic — along the operation named, so the claim is not keyed to one operation. The
/// versioned spelling is a row because the framework's target reader strips a trailing
/// `_<digits>` API-version suffix, so both spellings name one service.
#[tokio::test]
async fn only_requests_naming_this_service_are_claimed() {
    let router = harness();

    // (case, x-amz-target, body, expected status, expected error code)
    let rows: [(&str, &str, &str, u16, Option<&str>); 7] = [
        (
            // The framework's target reader strips a trailing `_<digits>` API-version
            // suffix, so a client sending the versioned spelling names the same service.
            "its own ListDatasets, versioned target spelling",
            "AmazonForecast_20180627.ListDatasets",
            "{}",
            200,
            None,
        ),
        (
            // Implemented, and with nothing stored it answers its own empty listing —
            // which only this service can produce.
            "its own ListDatasets",
            "AmazonForecast.ListDatasets",
            "{}",
            200,
            None,
        ),
        (
            "its own ListDatasetGroups",
            "AmazonForecast.ListDatasetGroups",
            "{}",
            200,
            None,
        ),
        (
            // Still scaffolded: a 501 is this service answering, not a routing failure.
            "one of its scaffolded operations",
            "AmazonForecast.ListForecasts",
            "{}",
            501,
            Some("NotImplemented"),
        ),
        (
            // The query service's traffic: declined here, so with only this service
            // mounted the router finds no claimant.
            "the query service's QueryForecast",
            "AmazonForecastRuntime.QueryForecast",
            r#"{"ForecastArn":"arn:aws:forecast:us-east-1:000000000000:forecast/x","Filters":{"item_id":"i"}}"#,
            404,
            Some("UnknownService"),
        ),
        (
            "the query service's QueryWhatIfForecast",
            "AmazonForecastRuntime.QueryWhatIfForecast",
            r#"{"WhatIfForecastArn":"arn:aws:forecast:us-east-1:000000000000:what-if-forecast/x/v","Filters":{"item_id":"i"}}"#,
            404,
            Some("UnknownService"),
        ),
        (
            // A third service's target, signed for `forecast`: not this service's either.
            "another service entirely",
            "AWSKMS.Encrypt",
            "{}",
            404,
            Some("UnknownService"),
        ),
    ];

    for (case, target, body, expected_status, expected_code) in rows {
        let (status, code) = call(&router, target, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, expected_code),
            "case {case:?} ({target}): expected HTTP {expected_status} {expected_code:?}, \
             got {status} {code:?}"
        );
    }
}

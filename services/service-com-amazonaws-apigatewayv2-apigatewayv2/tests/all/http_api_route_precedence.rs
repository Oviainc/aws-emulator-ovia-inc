// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` HTTP data plane's route resolution: WHICH route of a deployed HTTP api
//! stage answers a request.
//!
//! The rule these cases assert, one level above the recorded case (an api holding
//! `GET /items/{id}` beside `GET /items/special`):
//!
//! > A request to a deployed stage of an HTTP api is served by the MOST SPECIFIC matching
//! > route key, decided from the key itself and never from the order the routes were
//! > created in: segment by segment from the left, a literal segment is more specific than
//! > a `{variable}`, which is more specific than the greedy `{proxy+}` standing for the
//! > remainder of the path; a key naming the request's own method beats the `ANY`
//! > catch-all for the same path; and `$default` serves only what no other key matches.
//!
//! Which route answered is OBSERVABLE because each route of the table fronts its own
//! `HTTP_PROXY` integration: an upstream is a URI the caller can reach, so the data plane
//! sends the caller there — `307` with that integration's own `IntegrationUri` in
//! `Location` — rather than connecting to it. The `Location` therefore names the route that
//! served the request.
//!
//! What this suite does NOT assert, stated so a passing run is not read as more than it is:
//! the path-parameter VALUES a matched `{variable}` binds are delivered to the integration
//! in its event, and an `AWS_PROXY` integration's event is a Lambda invocation this
//! emulator has no runtime for (lambda `Invoke` answers 501 for a deployment package), so
//! no case here can observe a bound value reaching a backend.
//!
//! The rows span what the rule generalises over: the form of each key's segments (literal,
//! single-segment variable, greedy remainder), where the keys part company (the first
//! segment, a later one), the length of the request's path, the method (named and `ANY`),
//! and whether anything matches at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};
use crate::websocket_handshake_data_plane::{send, Answer};

/// The status the data plane answers a route fronting an `HTTP_PROXY` upstream with.
const TEMPORARY_REDIRECT: u16 = 307;

/// The error code the front door answers when NO service claims a request: an invoke of a
/// deployed stage must never be answered with it.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// Every route the api carries, as `(RouteKey, upstream)`. The upstream is what identifies
/// the route in an answer, so each one is distinct.
///
/// Within one method's path space a position that is a variable has ONE name, so the keys
/// below never put two differently named variables at the same level of the same method —
/// the service refuses such a pair at `CreateRoute`.
const ROUTES: &[(&str, &str)] = &[
    ("GET /{resource}/{id}", "https://generic.example.com"),
    ("ANY /files/{proxy+}", "https://files-greedy.example.com"),
    ("$default", "https://fallback.example.com"),
    ("GET /items/special", "https://special.example.com"),
    ("GET /items/{id}", "https://by-id.example.com"),
    (
        "GET /items/{id}/reviews/{reviewId}",
        "https://reviews.example.com",
    ),
    ("GET /files/{id}", "https://file-by-id.example.com"),
    ("GET /health", "https://health-get.example.com"),
    ("ANY /health", "https://health-any.example.com"),
];

/// One row: a request to the deployed stage, and the upstream the route that must serve it
/// fronts.
struct Row {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    upstream: &'static str,
}

fn rows() -> &'static [Row] {
    &[
        Row {
            case: "the recorded case: a literal segment beats the variable at the same \
                   level",
            method: "GET",
            path: "/items/special",
            upstream: "https://special.example.com",
        },
        Row {
            case: "a value no literal key names binds the variable",
            method: "GET",
            path: "/items/42",
            upstream: "https://by-id.example.com",
        },
        Row {
            case: "a literal first segment beats a variable first segment",
            method: "GET",
            path: "/orders/9",
            upstream: "https://generic.example.com",
        },
        Row {
            case: "the longer run of matching segments wins",
            method: "GET",
            path: "/items/42/reviews/7",
            upstream: "https://reviews.example.com",
        },
        Row {
            case: "a variable beats the greedy remainder at the same level",
            method: "GET",
            path: "/files/report.pdf",
            upstream: "https://file-by-id.example.com",
        },
        Row {
            case: "the greedy remainder takes a path no segment-for-segment key matches",
            method: "GET",
            path: "/files/2026/q1/report.pdf",
            upstream: "https://files-greedy.example.com",
        },
        Row {
            case: "the greedy remainder's ANY key catches another method",
            method: "DELETE",
            path: "/files/report.pdf",
            upstream: "https://files-greedy.example.com",
        },
        Row {
            case: "a key naming the request's method beats the ANY key for the same path",
            method: "GET",
            path: "/health",
            upstream: "https://health-get.example.com",
        },
        Row {
            case: "the ANY key serves the method no key names",
            method: "POST",
            path: "/health",
            upstream: "https://health-any.example.com",
        },
        Row {
            case: "$default serves a path no key matches",
            method: "DELETE",
            path: "/nothing/here/at/all",
            upstream: "https://fallback.example.com",
        },
        Row {
            case: "$default serves a path with MORE segments than the keys that share its \
                   prefix",
            method: "GET",
            path: "/items/42/extra",
            upstream: "https://fallback.example.com",
        },
        Row {
            case: "$default serves the api's root, which no key with a segment matches",
            method: "GET",
            path: "/",
            upstream: "https://fallback.example.com",
        },
    ]
}

/// Create the api, one `HTTP_PROXY` integration and route per entry of `routes` (in the
/// order given), and the `$default` stage that serves the api's root; answer the host of
/// the api's own `ApiEndpoint`.
async fn deploy(
    emulator: &MultiServiceRouter,
    case: &str,
    routes: &[(&str, &str)],
) -> String {
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "precedence-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    for (route_key, upstream) in routes {
        let (status, integration) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": upstream,
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0"
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateIntegration for {route_key:?} must answer 201, got \
             {status} {integration}"
        );
        let integration_id = as_str(&integration, "integrationId");
        let (status, route) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": route_key,
                "target": format!("integrations/{integration_id}")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRoute for {route_key:?} must answer 201, got {status} {route}"
        );
    }

    let (status, stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default", "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {status} {stage}"
    );
    assert!(
        stage.get("deploymentId").and_then(Value::as_str).is_some(),
        "case {case:?}: an auto-deploying stage must carry a deployment, got {stage}"
    );
    host
}

/// Assert the request was served by the route fronting `upstream`.
fn served_by(case: &str, method: &str, path: &str, answer: &Answer, upstream: &str) {
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: {method} {path} must be served by this service, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.status, TEMPORARY_REDIRECT,
        "case {case:?}: {method} {path} expected {TEMPORARY_REDIRECT}, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.location.as_deref(),
        Some(upstream),
        "case {case:?}: {method} {path} expected the route fronting {upstream:?} to serve \
         it, got {:?}",
        answer.location
    );
}

#[tokio::test]
async fn the_most_specific_route_key_serves_the_request() {
    let emulator = emulator();
    let host = deploy(&emulator, "the whole table", ROUTES).await;

    for row in rows() {
        let answer = send(&emulator, &host, row.method, row.path, &[]).await;
        served_by(row.case, row.method, row.path, &answer, row.upstream);
    }
}

#[tokio::test]
async fn route_precedence_does_not_depend_on_the_order_the_routes_were_created_in() {
    // The same keys, created in the opposite order: specificity is read off the key, so
    // every row of the table is answered by the same route as above.
    let reversed: Vec<(&str, &str)> = ROUTES.iter().copied().rev().collect();
    let emulator = emulator();
    let host = deploy(&emulator, "the table, created in reverse", &reversed).await;

    for row in rows() {
        let answer = send(&emulator, &host, row.method, row.path, &[]).await;
        served_by(row.case, row.method, row.path, &answer, row.upstream);
    }
}

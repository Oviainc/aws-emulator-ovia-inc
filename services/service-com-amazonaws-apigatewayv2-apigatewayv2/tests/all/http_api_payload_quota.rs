// SPDX-License-Identifier: CC0-1.0

//! The request-payload quota of an HTTP api's `execute-api` endpoint: which bodies the
//! gateway passes on to a route's integration, and which it refuses itself.
//!
//! The rule these cases assert, one level above the recorded case (one `POST` of
//! 10485760 bytes and one of 10485761 to a proxy route of a deployed `$default` stage):
//!
//! > **An HTTP api's endpoint accepts a request payload up to
//! > [`MAX_HTTP_API_PAYLOAD_BYTES`] and refuses a larger one itself.** The quota is the
//! > data plane's and is not adjustable — no member of this model names it — so it is the
//! > same for every api, stage, route, method and integration type: a body of at most the
//! > quota is passed on to the route's integration, which decides the answer, and a body
//! > of one byte more is refused by the GATEWAY with HTTP 413 ahead of the dispatch, so
//! > the integration observes no invocation for it.
//!
//! The rows span what the rule generalises over: the body's size (empty, small, one byte
//! under the quota, exactly the quota, one byte over it, far over it), the request method
//! (`POST`, `PUT`), and the route's integration type — an `HTTP_PROXY` route, whose
//! dispatch answers `307` to the upstream, and an `AWS_PROXY` route, whose dispatch
//! answers `500` because no deployment package runs here. The two integration types are
//! what makes "ahead of the dispatch" observable: an admitted body reaches a DIFFERENT
//! answer per integration, while an over-quota body earns the same `413` on both.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_HTTP_API_PAYLOAD_BYTES;

use crate::api_lifecycle::{as_str, call, emulator};
use crate::websocket_handshake_data_plane::{message, send_with_body, Answer};

/// The status the gateway refuses an over-quota payload with, and the sentence it carries.
const PAYLOAD_TOO_LARGE: u16 = 413;
const PAYLOAD_TOO_LARGE_MESSAGE: &str = "Request Entity Too Large";

/// The stage the api is deployed to, and the route path every case invokes.
const STAGE_NAME: &str = "$default";
const ROUTE_KEY: &str = "ANY /{proxy+}";
const ROUTE_PATH: &str = "/upload";

/// The upstream an `HTTP_PROXY` route is sent to, and the status that dispatch answers:
/// `307` to the stored `IntegrationUri`, the redirect that preserves the request's method
/// and body.
const UPSTREAM: &str = "https://backend.example.com/upload";
const REDIRECT: u16 = 307;

/// The status an `AWS_PROXY` route's dispatch answers: the integration produced no
/// response, because no Lambda deployment package runs in this emulator.
const INTEGRATION_FAULT: u16 = 500;

/// One deployed api: how its route dispatches, and what an ADMITTED request to it is
/// answered with.
struct Backend {
    case: &'static str,
    /// The `CreateIntegration` request body.
    integration: Value,
    /// The status the integration's own dispatch answers an admitted request with.
    admitted_status: u16,
}

fn backends() -> Vec<Backend> {
    vec![
        Backend {
            case: "an HTTP_PROXY route, whose dispatch redirects to its upstream",
            integration: json!({
                "integrationType": "HTTP_PROXY",
                "integrationMethod": "ANY",
                "integrationUri": UPSTREAM,
                "payloadFormatVersion": "1.0"
            }),
            admitted_status: REDIRECT,
        },
        Backend {
            case: "an AWS_PROXY route, whose dispatch reaches a function that cannot run",
            integration: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri":
                    "arn:aws:lambda:us-east-1:123456789012:function:upload-handler",
                "payloadFormatVersion": "2.0"
            }),
            admitted_status: INTEGRATION_FAULT,
        },
    ]
}

/// One body size, and whether the quota admits it.
struct Size {
    case: &'static str,
    bytes: usize,
    admitted: bool,
}

/// The size table: the quota's own boundary, and a row either side of the classes it
/// separates. Every value is computed from [`MAX_HTTP_API_PAYLOAD_BYTES`], so no row
/// restates the byte count.
fn sizes() -> Vec<Size> {
    vec![
        Size {
            case: "no payload at all",
            bytes: 0,
            admitted: true,
        },
        Size {
            case: "a one-byte payload",
            bytes: 1,
            admitted: true,
        },
        Size {
            case: "one byte under the quota",
            bytes: MAX_HTTP_API_PAYLOAD_BYTES - 1,
            admitted: true,
        },
        Size {
            case: "exactly the quota",
            bytes: MAX_HTTP_API_PAYLOAD_BYTES,
            admitted: true,
        },
        Size {
            case: "one byte over the quota",
            bytes: MAX_HTTP_API_PAYLOAD_BYTES + 1,
            admitted: false,
        },
        Size {
            case: "far over the quota",
            bytes: MAX_HTTP_API_PAYLOAD_BYTES + 4096,
            admitted: false,
        },
    ]
}

/// Deploy one HTTP api whose `ROUTE_KEY` route targets `integration`, and answer
/// `(ApiId, the host of its own ApiEndpoint)`.
async fn deploy(emulator: &MultiServiceRouter, case: &str, integration: &Value) -> String {
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "payload-quota-api", "protocolType": "HTTP" })),
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

    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(integration.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {status} {created}"
    );
    let integration_id = as_str(&created, "integrationId");

    let (status, route) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": ROUTE_KEY,
            "target": format!("integrations/{integration_id}"),
            "authorizationType": "NONE"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {status} {route}"
    );

    let (status, stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": STAGE_NAME, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {status} {stage}"
    );
    host
}

/// POST (or PUT) `bytes` bytes to the deployed route of the api served on `host`.
///
/// The `$default` stage "serves an api's root", so the invoke path carries the route path
/// alone — the addressing `Api$ApiEndpoint` documents for it.
async fn invoke(
    emulator: &MultiServiceRouter,
    host: &str,
    method: &str,
    bytes: usize,
) -> Answer {
    send_with_body(
        emulator,
        host,
        method,
        ROUTE_PATH,
        &[("content-type", "application/octet-stream")],
        vec![b'x'; bytes],
    )
    .await
}

/// A body within the quota is passed to the route's integration, whose own dispatch
/// answers it; a body past the quota is refused by the gateway with `413` and the
/// integration is never reached — whatever the integration is, and whatever method the
/// payload arrived by.
#[tokio::test]
async fn the_payload_quota_is_applied_ahead_of_the_route_s_integration() {
    let emulator = emulator();

    for backend in backends() {
        let host = deploy(&emulator, backend.case, &backend.integration).await;

        for method in ["POST", "PUT"] {
            for size in sizes() {
                let case = format!("{} / {method} / {}", backend.case, size.case);
                let answer = invoke(&emulator, &host, method, size.bytes).await;
                if size.admitted {
                    assert_eq!(
                        answer.status,
                        backend.admitted_status,
                        "case {case:?} ({} bytes): a payload within the quota must reach \
                         the route's integration, whose dispatch answers {}, got {}",
                        size.bytes,
                        backend.admitted_status,
                        answer.summary()
                    );
                    assert_ne!(
                        answer.status, PAYLOAD_TOO_LARGE,
                        "case {case:?} ({} bytes): a payload within the quota must not be \
                         refused {PAYLOAD_TOO_LARGE}, got {}",
                        size.bytes,
                        answer.summary()
                    );
                } else {
                    assert_eq!(
                        answer.status, PAYLOAD_TOO_LARGE,
                        "case {case:?} ({} bytes): a payload past the quota must be \
                         refused {PAYLOAD_TOO_LARGE}, got {}",
                        size.bytes,
                        answer.summary()
                    );
                    assert_eq!(
                        message(&answer),
                        PAYLOAD_TOO_LARGE_MESSAGE,
                        "case {case:?} ({} bytes): expected the \
                         {PAYLOAD_TOO_LARGE_MESSAGE:?} body, got {}",
                        size.bytes,
                        answer.summary()
                    );
                    assert!(
                        answer.location.is_none(),
                        "case {case:?} ({} bytes): a refused payload must not be \
                         forwarded to the integration's upstream, got {}",
                        size.bytes,
                        answer.summary()
                    );
                }
            }
        }
    }
}

/// The refusal is the GATEWAY's, so it is answered for a route whose own authorization
/// admitted the request and before anything the route fronts: the `HTTP_PROXY` route that
/// redirects an admitted request answers `413` with no `location` at all once the payload
/// is past the quota.
#[tokio::test]
async fn an_over_quota_payload_is_never_forwarded_to_the_upstream() {
    let emulator = emulator();
    let backend = backends()
        .into_iter()
        .find(|backend| backend.admitted_status == REDIRECT)
        .expect("the HTTP_PROXY row");
    let host = deploy(&emulator, backend.case, &backend.integration).await;

    let admitted = invoke(&emulator, &host, "POST", MAX_HTTP_API_PAYLOAD_BYTES).await;
    assert_eq!(
        admitted.location.as_deref(),
        Some(UPSTREAM),
        "case \"exactly the quota\": the payload must be forwarded to {UPSTREAM:?}, got {}",
        admitted.summary()
    );

    let refused = invoke(&emulator, &host, "POST", MAX_HTTP_API_PAYLOAD_BYTES + 1).await;
    assert_eq!(
        refused.status, PAYLOAD_TOO_LARGE,
        "case \"one byte over the quota\": expected {PAYLOAD_TOO_LARGE}, got {}",
        refused.summary()
    );
    assert_eq!(
        refused.location, None,
        "case \"one byte over the quota\": the refusal must name no upstream, got {}",
        refused.summary()
    );
}

// SPDX-License-Identifier: CC0-1.0

//! The shared response-media-type rule, driven through this service's operations.
//!
//! The rule, the one function that applies it and the operation-seam wrapper that carries
//! it to the framework's refusal tail live in `service_common::response_media_type`; this
//! service states only the pair its responses are re-announced across
//! (`src/service/response_media_type.rs`). This suite drives that one shared path through
//! the wire, at each of the three seams the service applies it at — the wrapper
//! `Service::override_operation` hands back, `Service::override_routing_error` and
//! `Service::override_framework_error` — so the service and the shared crate cannot drift
//! apart unnoticed.
//!
//! The expectations are the shared crate's own two constants rather than strings spelled
//! here: `AWS_JSON_1_0` is the media type the model's `aws.protocols#awsJson1_0` trait
//! implies and the framework's renderers stamp, and `AWS_JSON_1_1` is the one every
//! recorded response of this service announces.
//!
//! The rows span two dimensions: the LAYER that produced the response (a handler success,
//! a handler's modelled exception, the boundary refusal the framework renders inside the
//! request walk, the routing failure rendered before any operation runs, and the transport
//! failure rendered before the request is routed), across several operations; and the media
//! type the REQUEST was sent under, which is not re-read — a call sent under either awsJson
//! media type is understood and answered under the announced one.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_common::response_media_type::{AWS_JSON_1_0, AWS_JSON_1_1};

use super::dashboard_lifecycle::{ok_call, router};

/// The SigV4 authorization header the router resolves this service's route key from. The
/// router these tests use has request authentication off, so the signature is never
/// checked — only the credential scope's signing name is read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20250818/us-east-1/bcm-dashboards/aws4_request, \
     SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000";

/// One request, sent under `request_media_type`, answered with the WHOLE response so the
/// header a client reads is visible. `target` is `None` for a request carrying no
/// `X-Amz-Target` at all.
async fn send(
    router: &MultiServiceRouter,
    request_media_type: &str,
    target: Option<&str>,
    chunked: bool,
    body: &Value,
) -> http::Response<Bytes> {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", AUTHORIZATION)
        .header("host", "bcm-dashboards.us-east-1.amazonaws.com")
        .header("content-type", request_media_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", format!("AWSBCMDashboardsService.{target}"));
    }
    if chunked {
        // Declares `aws-chunked` framing and carries none of it, which the router refuses
        // before the request is routed at all.
        request = request.header("content-encoding", "aws-chunked");
    }
    let request = request
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    router.dispatch(request).await
}

/// The media type a response announces, as a client reads it.
fn media_type(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<no content-type>")
        .to_string()
}

/// One call through the shared path, and the layer that answers it.
struct Case {
    /// What produced the response, and through which seam the shared rule reaches it.
    label: &'static str,
    /// The operation named in `X-Amz-Target`, or `None` for a request carrying none.
    target: Option<&'static str>,
    /// Whether the request declares `aws-chunked` framing it does not carry.
    chunked: bool,
    body: Value,
    status: u16,
}

/// Every layer that can answer a request for this service announces the media type the
/// shared pair names, whichever awsJson media type the request was sent under.
#[tokio::test]
async fn every_layer_announces_the_shared_pairs_media_type() {
    // Both awsJson media types, because the REQUEST side is not re-read: the codec is the
    // same `JsonCodec` over the same body either way.
    for request_media_type in [AWS_JSON_1_0, AWS_JSON_1_1] {
        let router = router();
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({"name": "Shared Rule Fixture", "widgets": []}),
        )
        .await;
        let arn = created["arn"].as_str().expect("an arn").to_string();
        // A well-formed `DashboardArn` for a dashboard nothing is stored under: the
        // created one with its resource id replaced, so the account id stays the engine's.
        let (prefix, _) = arn.rsplit_once('/').expect("an arn naming a resource");
        let unknown_arn = format!("{prefix}/no-such-dashboard");

        let cases = [
            Case {
                label: "a handler success through the shared wrapper: a create",
                target: Some("CreateDashboard"),
                chunked: false,
                body: json!({"name": "Shared Rule Second", "widgets": []}),
                status: 200,
            },
            Case {
                label: "a handler success through the shared wrapper: a single read",
                target: Some("GetDashboard"),
                chunked: false,
                body: json!({"arn": arn.clone()}),
                status: 200,
            },
            Case {
                label: "a handler success through the shared wrapper: a listing",
                target: Some("ListDashboards"),
                chunked: false,
                body: json!({"maxResults": 1}),
                status: 200,
            },
            Case {
                label: "a handler's modelled exception: a dashboard nothing is stored under",
                target: Some("GetDashboard"),
                chunked: false,
                body: json!({"arn": unknown_arn}),
                status: 400,
            },
            Case {
                label: "the refusal tail INSIDE the walk: an absent @required member",
                target: Some("GetDashboard"),
                chunked: false,
                body: json!({}),
                status: 400,
            },
            Case {
                label: "the refusal tail INSIDE the walk: a value outside a @pattern",
                target: Some("UpdateDashboard"),
                chunked: false,
                body: json!({
                    "arn": "arn:aws:bcm-dashboards::123456789012:dashboard/NOT-LOWERCASE"
                }),
                status: 400,
            },
            Case {
                label: "the routing failure: an X-Amz-Target naming no modelled operation",
                target: Some("NoSuchOperation"),
                chunked: false,
                body: json!({}),
                status: 501,
            },
            Case {
                label: "the routing failure: no X-Amz-Target to read a routing key from",
                target: None,
                chunked: false,
                body: json!({}),
                status: 400,
            },
            Case {
                label: "the transport failure: an aws-chunked body that is not framed",
                target: Some("ListScheduledReports"),
                chunked: true,
                body: json!({}),
                status: 400,
            },
            // Last: it removes the fixture the rows above read.
            Case {
                label: "a handler success through the shared wrapper: a delete",
                target: Some("DeleteDashboard"),
                chunked: false,
                body: json!({"arn": arn.clone()}),
                status: 200,
            },
        ];

        for case in &cases {
            let response = send(
                &router,
                request_media_type,
                case.target,
                case.chunked,
                &case.body,
            )
            .await;
            let actual_status = response.status().as_u16();
            let actual = media_type(&response);
            assert_eq!(
                actual_status, case.status,
                "case {:?} sent under {request_media_type}: expected HTTP {}, got \
                 {actual_status} ({actual})",
                case.label, case.status
            );
            assert_eq!(
                actual, AWS_JSON_1_1,
                "case {:?} sent under {request_media_type}: expected content-type \
                 {AWS_JSON_1_1}, got {actual:?}",
                case.label
            );
            let parsed: Result<Value, _> = serde_json::from_slice(response.body());
            assert!(
                parsed.is_ok(),
                "case {:?} sent under {request_media_type}: the body under {actual} must be \
                 the JSON document that media type promises, got {:?}",
                case.label,
                String::from_utf8_lossy(response.body())
            );
        }
    }
}

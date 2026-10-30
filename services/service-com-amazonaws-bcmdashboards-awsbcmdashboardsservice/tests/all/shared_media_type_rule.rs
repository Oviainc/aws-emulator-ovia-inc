// SPDX-License-Identifier: CC0-1.0

//! The media type a REQUEST announces, and why it changes no response.
//!
//! The media types themselves are the shared crate's two constants rather than strings
//! spelled here: `service_common::response_media_type::AWS_JSON_1_0` is what this service's
//! `aws.protocols#awsJson1_0` model implies — the value the framework stamps, and the one
//! every response answered in front of the dispatcher announces — and `AWS_JSON_1_1` is
//! the one the dashboard and scheduled-report dispatcher announces. The shared crate is
//! also where the mechanism lives that this service re-announces with
//! (`src/service/response_media_type.rs` states only which announcer answers for what).
//!
//! The rule under test: which announcer produced a response is what decides the media type
//! it announces, and the media type the REQUEST carried is not re-read — the awsJson `1.0`
//! and `1.1` codecs are the same JSON over the same `X-Amz-Target` routing key, so a call
//! sent under either one, under a bare `application/json`, under a parameterised value, or
//! under no `content-type` at all is understood and answered identically. The rows cross
//! that request dimension with one call from each announcer: a dispatcher success, a
//! refusal that reports the request as invalid, and a resource-tagging read.
//! `tests/all/response_media_type.rs` owns the other dimension of the same rule: every
//! layer that can produce a response.

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

/// One request, sent under `request_media_type` (or under none at all when it is `None`),
/// answered with the WHOLE response so the header a client reads is visible. `target` is
/// `None` for a request carrying no `X-Amz-Target` at all.
async fn send(
    router: &MultiServiceRouter,
    request_media_type: Option<&str>,
    target: Option<&str>,
    chunked: bool,
    body: &Value,
) -> http::Response<Bytes> {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", AUTHORIZATION)
        .header("host", "bcm-dashboards.us-east-1.amazonaws.com");
    if let Some(target) = target {
        request = request.header("x-amz-target", format!("AWSBCMDashboardsService.{target}"));
    }
    if let Some(request_media_type) = request_media_type {
        request = request.header("content-type", request_media_type);
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

/// One call, the announcer that answers it and the status it carries.
struct Case {
    /// What the call asks for, and which announcer answers.
    label: &'static str,
    /// The operation named in `X-Amz-Target`, or `None` for a request carrying none.
    target: Option<&'static str>,
    /// Whether the request declares `aws-chunked` framing it does not carry.
    chunked: bool,
    body: Value,
    status: u16,
    /// The media type this response announces, whatever the request announced.
    media_type: &'static str,
}

/// Whatever media type the request announced, every response announces the media type of
/// the announcer that produced it — and the call is understood either way.
#[tokio::test]
async fn the_requests_media_type_changes_no_response() {
    // The media types a caller can put on the request: both awsJson versions, the bare
    // JSON media type, a parameterised one, and none at all.
    let request_media_types = [
        Some(AWS_JSON_1_0),
        Some(AWS_JSON_1_1),
        Some("application/json"),
        Some("application/x-amz-json-1.0; charset=utf-8"),
        None,
    ];

    for request_media_type in request_media_types {
        let router = router();
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({"name": "Request Media Type Fixture", "widgets": []}),
        )
        .await;
        let arn = created["arn"].as_str().expect("an arn").to_string();

        let cases = [
            Case {
                label: "the dispatcher: a read of the stored dashboard",
                target: Some("GetDashboard"),
                chunked: false,
                body: json!({"arn": arn.clone()}),
                status: 200,
                media_type: AWS_JSON_1_1,
            },
            Case {
                label: "the validator: no @required member in the body",
                target: Some("GetDashboard"),
                chunked: false,
                body: json!({}),
                status: 400,
                media_type: AWS_JSON_1_0,
            },
            Case {
                label: "the tagging family: a tag listing",
                target: Some("ListTagsForResource"),
                chunked: false,
                body: json!({"resourceArn": arn.clone()}),
                status: 200,
                media_type: AWS_JSON_1_0,
            },
            Case {
                label: "before routing: an X-Amz-Target naming no modelled operation",
                target: Some("NoSuchOperation"),
                chunked: false,
                body: json!({}),
                status: 501,
                media_type: AWS_JSON_1_0,
            },
            Case {
                label: "before routing: no X-Amz-Target to read a routing key from",
                target: None,
                chunked: false,
                body: json!({}),
                status: 400,
                media_type: AWS_JSON_1_0,
            },
            Case {
                label: "before routing: an aws-chunked body that is not framed",
                target: Some("ListScheduledReports"),
                chunked: true,
                body: json!({}),
                status: 400,
                media_type: AWS_JSON_1_0,
            },
            // Last: it removes the fixture the rows above read.
            Case {
                label: "the dispatcher: a delete of the stored dashboard",
                target: Some("DeleteDashboard"),
                chunked: false,
                body: json!({"arn": arn.clone()}),
                status: 200,
                media_type: AWS_JSON_1_1,
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
                "case {:?} sent under {request_media_type:?}: expected HTTP {}, got \
                 {actual_status} ({actual})",
                case.label, case.status
            );
            assert_eq!(
                actual, case.media_type,
                "case {:?} sent under {request_media_type:?}: expected content-type {}, \
                 got {actual:?} (the request's media type is not re-read)",
                case.label, case.media_type
            );
            let parsed: Result<Value, _> = serde_json::from_slice(response.body());
            assert!(
                parsed.is_ok(),
                "case {:?} sent under {request_media_type:?}: the body under {actual} must \
                 be the JSON document that media type promises, got {:?}",
                case.label,
                String::from_utf8_lossy(response.body())
            );
        }
    }
}

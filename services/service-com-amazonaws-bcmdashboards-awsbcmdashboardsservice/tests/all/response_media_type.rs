// SPDX-License-Identifier: CC0-1.0

//! The media type every response of this service announces.
//!
//! Every recorded response of this service — a success and an error alike — carries
//! `content-type: application/x-amz-json-1.1`, while the media type the model's
//! `aws.protocols#awsJson1_0` trait implies, and which the framework's serializers and
//! pre-dispatch error renderers stamp, is `application/x-amz-json-1.0`. A client (or a
//! recording proxy) selects its JSON-protocol decoder from that header, so the one the
//! service announces is part of its wire contract.
//!
//! The rule under test: EVERY response this service answers announces
//! `application/x-amz-json-1.1`, whichever layer produced it, while the REQUEST side stays
//! awsJson1_0 — the calls below are routed by `X-Amz-Target` and sent under
//! `content-type: application/x-amz-json-1.0` (see [`super::dashboard_lifecycle::call`]),
//! and every one of them is understood.
//!
//! The rows span the layers that can produce a response, because the rule is about all of
//! them and not about one operation: a handler's success, a handler's modelled exception,
//! a handler's own refusal, the boundary validator's refusal, the scaffold's 501, the
//! routing failure rendered before any operation runs, and the transport failure rendered
//! before the request is even routed.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

use super::dashboard_lifecycle::{ok_call, router};

/// The media type every response of this service announces, as the wire record shows.
const RESPONSE_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The media type the model's `awsJson1_0` protocol trait implies. Requests are still sent
/// under it; no response may announce it.
const PROTOCOL_MEDIA_TYPE: &str = "application/x-amz-json-1.0";

/// The SigV4 authorization header the router resolves this service's route key from. The
/// router these tests use has request authentication off, so the signature is never
/// checked — only the credential scope's signing name is read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20250818/us-east-1/bcm-dashboards/aws4_request, \
     SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000";

/// Call one operation and answer the WHOLE response, headers included — what
/// [`super::dashboard_lifecycle::call`] parses the body out of.
async fn send(
    router: &MultiServiceRouter,
    target: &str,
    body: &Value,
) -> http::Response<Bytes> {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", AUTHORIZATION)
        .header("host", "bcm-dashboards.us-east-1.amazonaws.com")
        // The REQUEST side is awsJson1_0 and stays that way.
        .header("content-type", PROTOCOL_MEDIA_TYPE)
        .header("x-amz-target", format!("AWSBCMDashboardsService.{target}"))
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

/// Assert the rule on one response: it announces the media type this service answers
/// under, and the body under that media type is the JSON document the media type promises.
fn assert_media_type(case: &str, response: &http::Response<Bytes>, expected_status: u16) {
    let actual = media_type(response);
    let status = response.status().as_u16();
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} ({actual})"
    );
    assert_eq!(
        actual, RESPONSE_MEDIA_TYPE,
        "case {case:?}: expected content-type {RESPONSE_MEDIA_TYPE}, got {actual:?}"
    );
    let parsed: Result<Value, _> = serde_json::from_slice(response.body());
    assert!(
        parsed.is_ok(),
        "case {case:?}: the body under {actual} must be the JSON document that media type \
         promises, got {:?}",
        String::from_utf8_lossy(response.body())
    );
}

/// One call, and the status the layer that answers it renders.
struct Case {
    /// What produced the response.
    label: &'static str,
    /// The operation named in `X-Amz-Target`.
    operation: &'static str,
    body: Value,
    status: u16,
}

/// Every response of a routed request announces the service's media type: a success, a
/// modelled exception, a handler refusal, a boundary refusal and the scaffold's 501 all
/// come from different layers and all carry the same header.
#[tokio::test]
async fn every_routed_response_announces_the_services_media_type() {
    let router = router();
    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({"name": "Media Type Fixture", "widgets": []}),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();
    // A well-formed `DashboardArn` for a dashboard nothing is stored under: the created
    // one with its resource id replaced, so the account id stays the engine's.
    let (prefix, _) = arn.rsplit_once('/').expect("an arn naming a resource");
    let unknown_arn = format!("{prefix}/no-such-dashboard");

    let cases = [
        Case {
            label: "a handler success: the create that minted the fixture",
            operation: "CreateDashboard",
            body: json!({"name": "Second Dashboard", "widgets": []}),
            status: 200,
        },
        Case {
            label: "a handler success: a single-resource read",
            operation: "GetDashboard",
            body: json!({"arn": arn}),
            status: 200,
        },
        Case {
            label: "a handler success: a paged listing",
            operation: "ListDashboards",
            body: json!({"maxResults": 1}),
            status: 200,
        },
        Case {
            label: "a handler success: an empty listing",
            operation: "ListScheduledReports",
            body: json!({}),
            status: 200,
        },
        Case {
            label: "a handler success: a projected policy document",
            operation: "GetResourcePolicy",
            body: json!({"resourceArn": arn}),
            status: 200,
        },
        Case {
            label: "a handler success: a tag listing",
            operation: "ListTagsForResource",
            body: json!({"resourceArn": arn}),
            status: 200,
        },
        Case {
            label: "a handler success: an update",
            operation: "UpdateDashboard",
            body: json!({"arn": arn, "name": "Renamed Dashboard"}),
            status: 200,
        },
        Case {
            label: "a modelled exception: ResourceNotFoundException",
            operation: "GetDashboard",
            body: json!({"arn": unknown_arn}),
            status: 404,
        },
        Case {
            label: "a handler refusal: a pagination token this service never minted",
            operation: "ListDashboards",
            body: json!({"nextToken": "not-a-token-this-service-minted"}),
            status: 400,
        },
        Case {
            label: "a boundary refusal: an arn outside the modelled pattern",
            operation: "GetDashboard",
            body: json!({"arn": "arn:aws:bcm-dashboards::123456789012:dashboard/NOT-LOWERCASE"}),
            status: 400,
        },
        Case {
            label: "the scaffold's 501: an operation nobody has implemented yet",
            operation: "TagResource",
            body: json!({"resourceArn": arn, "resourceTags": [{"key": "team", "value": "finops"}]}),
            status: 501,
        },
        // Last: it removes the fixture every row above reads.
        Case {
            label: "a handler success: a delete",
            operation: "DeleteDashboard",
            body: json!({"arn": arn}),
            status: 200,
        },
    ];

    for case in &cases {
        let response = send(&router, case.operation, &case.body).await;
        assert_media_type(case.label, &response, case.status);
    }
}

/// A request that reaches this service but names no modelled operation is answered before
/// any operation runs — and under the same media type. The framework's own rendering for
/// the awsJson family is kept: an unknown operation is a 501 `NotImplemented`, an
/// unreadable routing key a 400 `SerializationException`.
#[tokio::test]
async fn a_response_rendered_before_routing_announces_the_services_media_type() {
    let router = router();

    // An `X-Amz-Target` naming an operation this model does not declare.
    for operation in ["NoSuchOperation", "getdashboard", "GetDashboardExtra"] {
        let response = send(&router, operation, &json!({})).await;
        assert_media_type(operation, &response, 501);
    }

    // No `X-Amz-Target` at all: the route key still resolves from the credential scope, so
    // the request reaches this service and is refused for the unreadable routing key.
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", AUTHORIZATION)
        .header("host", "bcm-dashboards.us-east-1.amazonaws.com")
        .header("content-type", PROTOCOL_MEDIA_TYPE)
        .body(Bytes::from_static(b"{}"))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert_media_type("no x-amz-target", &response, 400);
}

/// A transport-level failure the router detects before the request is routed at all — a
/// body that declares `aws-chunked` framing and is not framed — is answered under the same
/// media type.
#[tokio::test]
async fn a_transport_failure_announces_the_services_media_type() {
    let router = router();
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("authorization", AUTHORIZATION)
        .header("host", "bcm-dashboards.us-east-1.amazonaws.com")
        .header("content-type", PROTOCOL_MEDIA_TYPE)
        .header("content-encoding", "aws-chunked")
        .header("x-amz-target", "AWSBCMDashboardsService.ListDashboards")
        // Declares the framing and carries none of it.
        .body(Bytes::from_static(b"{}"))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    assert_media_type("an aws-chunked body that is not framed", &response, 400);
}

/// The ONE response of this service the framework alone announces: the authentication
/// refusal.
///
/// The router renders it through `definition().refusal_response(..)` before this service is
/// asked anything (`vendor/smithy-server-runtime/src/multi_service_router.rs`), and no
/// author seam is consulted on that path — `Service::override_refusal` chooses the code and
/// the message, never the envelope. So it still announces the media type the model's
/// protocol trait implies. This test pins that gap rather than leaving it in prose: should
/// the framework grow a seam for this path, it fails here and says to take it.
#[tokio::test]
async fn the_authentication_refusal_is_announced_by_the_framework_alone() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut authenticating = MultiServiceRouter::new();
    authenticating.set_request_authentication(true);
    authenticating.register_service(EmulatedService::new(engine));

    // The signature in [`AUTHORIZATION`] is not one this emulator vended, so the request is
    // refused before the operation it names runs.
    let response = send(&authenticating, "GetDashboard", &json!({})).await;
    let status = response.status().as_u16();
    assert_eq!(
        status,
        403,
        "case {:?}: an unverifiable signature must be refused, got {status}",
        "the authentication refusal"
    );
    let actual = media_type(&response);
    assert_eq!(
        actual, PROTOCOL_MEDIA_TYPE,
        "case {:?}: this path is the framework's alone, so it still announces \
         {PROTOCOL_MEDIA_TYPE}; got {actual:?}. If this now answers {RESPONSE_MEDIA_TYPE}, \
         the framework gained a seam for it — take it in src/service.rs and move this \
         case to the routed table above.",
        "the authentication refusal"
    );
}

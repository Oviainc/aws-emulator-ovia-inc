// SPDX-License-Identifier: CC0-1.0

//! The media type every response of this service announces — there are two announcers.
//!
//! The rule under test: a response of the DASHBOARD or SCHEDULED-REPORT dispatcher — its
//! successes, and the errors its handlers raise about stored state — announces
//! `application/x-amz-json-1.1`, while everything answered in FRONT of that dispatcher
//! announces the `application/x-amz-json-1.0` the model's `aws.protocols#awsJson1_0` trait
//! implies: a refusal that reports the request itself as invalid (the modelled
//! `ValidationException`), every response of a resource-tagging operation, an unroutable
//! request, a transport-level failure and the authentication refusal. A client (or a
//! recording proxy) picks its JSON-protocol decoder from that header, so which value a
//! response carries is part of the wire contract.
//!
//! The rows span both halves of the rule at once: the LAYER that produced the response (a
//! handler success, a handler's modelled exception about stored state, the boundary
//! validator's refusal, a validation refusal a handler raises because the boundary
//! validator cannot express the constraint, the routing failure rendered before any
//! operation runs — this service's only 501, now that every operation it models is
//! implemented — the transport failure rendered before the request is routed, and the
//! authentication refusal the router renders before this service is asked anything), and
//! the OPERATION family it belongs to. Every row states the media type it expects, so a
//! row moving from one announcer to the other fails here.
//!
//! The REQUEST side stays awsJson1_0: the calls below are sent under
//! `content-type: application/x-amz-json-1.0` (see [`super::dashboard_lifecycle::call`])
//! and every one of them is understood. `tests/all/shared_media_type_rule.rs` varies that
//! header instead, and shows it changes no response.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

use super::dashboard_lifecycle::{ok_call, router};

/// The media type the dashboard and scheduled-report dispatcher announces on what it
/// returns.
const DISPATCHER_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The media type the model's `awsJson1_0` protocol trait implies, which the framework
/// stamps and everything answered in front of the dispatcher announces. Requests are sent
/// under it too.
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

/// Assert one response: the status it carries, the media type it announces, and that the
/// body under that media type is the JSON document the media type promises.
fn assert_media_type(
    case: &str,
    response: &http::Response<Bytes>,
    expected_status: u16,
    expected_media_type: &str,
) {
    let actual = media_type(response);
    let status = response.status().as_u16();
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} ({actual})"
    );
    assert_eq!(
        actual, expected_media_type,
        "case {case:?}: expected content-type {expected_media_type}, got {actual:?}"
    );
    let parsed: Result<Value, _> = serde_json::from_slice(response.body());
    assert!(
        parsed.is_ok(),
        "case {case:?}: the body under {actual} must be the JSON document that media type \
         promises, got {:?}",
        String::from_utf8_lossy(response.body())
    );
}

/// One call: what produced the response, and what that layer answers with.
struct Case {
    /// What produced the response, and which announcer it belongs to.
    label: &'static str,
    /// The operation named in `X-Amz-Target`.
    operation: &'static str,
    body: Value,
    status: u16,
    /// The media type this response announces.
    media_type: &'static str,
}

/// Every response of a routed request announces the media type of the announcer that
/// produced it: the dispatcher's own successes and state errors on one side, the validation
/// refusals and the whole resource-tagging family on the other.
#[tokio::test]
async fn every_routed_response_announces_its_announcers_media_type() {
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
            label: "the dispatcher, a handler success: a create",
            operation: "CreateDashboard",
            body: json!({"name": "Second Dashboard", "widgets": []}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: a single-resource read",
            operation: "GetDashboard",
            body: json!({"arn": arn}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: a paged listing",
            operation: "ListDashboards",
            body: json!({"maxResults": 1}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: an empty listing",
            operation: "ListScheduledReports",
            body: json!({}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: a projected policy document",
            operation: "GetResourcePolicy",
            body: json!({"resourceArn": arn}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: an update",
            operation: "UpdateDashboard",
            body: json!({"arn": arn, "name": "Renamed Dashboard"}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, an error about stored state: a dashboard nothing is \
                    stored under",
            operation: "GetDashboard",
            body: json!({"arn": unknown_arn.clone()}),
            status: 400,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, an error about stored state: an update that writes \
                    nothing",
            operation: "UpdateScheduledReport",
            body: json!({
                "arn": "arn:aws:bcm-dashboards::123456789012:scheduled-report/never-created",
                "name": "Renamed Report"
            }),
            status: 400,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the validator, inside the walk: an absent @required member",
            operation: "CreateDashboard",
            body: json!({"widgets": []}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the validator, inside the walk: a page size above the modelled range",
            operation: "ListDashboards",
            // `maxResults` is `@range` min 1 max 100, source:
            // `smithy_query(query="operation", operation="ListDashboards")`.
            body: json!({"maxResults": 101}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the validator, inside the walk: a page size below the modelled range",
            operation: "ListDashboards",
            body: json!({"maxResults": 0}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the validator, inside the walk: an arn outside the modelled pattern",
            operation: "GetDashboard",
            body: json!({"arn": "arn:aws:bcm-dashboards::123456789012:dashboard/NOT-LOWERCASE"}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the validator, raised by a handler: a name the boundary's regular \
                    expression engine cannot refuse",
            operation: "CreateDashboard",
            // `DashboardName` is `@pattern` "no two spaces in a row" — a negative
            // lookahead the boundary validator cannot compile, so the handler refuses it
            // with the same modelled ValidationException. Source:
            // `smithy_query(query="operation", operation="CreateDashboard")`.
            body: json!({"name": "Two  Spaces", "widgets": []}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the validator, raised by a handler: a pagination token this service \
                    never minted",
            operation: "ListDashboards",
            body: json!({"nextToken": "not-a-token-this-service-minted"}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag listing",
            operation: "ListTagsForResource",
            body: json!({"resourceArn": arn}),
            status: 200,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag listing for a resource nothing is stored \
                    under",
            operation: "ListTagsForResource",
            body: json!({"resourceArn": unknown_arn.clone()}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag merge",
            operation: "TagResource",
            body: json!({"resourceArn": arn, "resourceTags": [{"key": "team", "value": "finops"}]}),
            status: 200,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag merge onto a resource nothing is stored under",
            operation: "TagResource",
            body: json!({
                "resourceArn": unknown_arn.clone(),
                "resourceTags": [{"key": "team", "value": "finops"}]
            }),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag removal",
            operation: "UntagResource",
            body: json!({"resourceArn": arn, "resourceTagKeys": ["team"]}),
            status: 200,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag removal naming no key the resource carries",
            operation: "UntagResource",
            body: json!({"resourceArn": arn, "resourceTagKeys": ["never-set"]}),
            status: 200,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag listing whose arn is outside the modelled \
                    pattern",
            operation: "ListTagsForResource",
            body: json!({"resourceArn": "arn:aws:bcm-dashboards::123456789012:dashboard/NOT-LOWERCASE"}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        // Last: it removes the fixture the rows above read.
        Case {
            label: "the dispatcher, a handler success: a delete",
            operation: "DeleteDashboard",
            body: json!({"arn": arn}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
    ];

    for case in &cases {
        let response = send(&router, case.operation, &case.body).await;
        assert_media_type(case.label, &response, case.status, case.media_type);
    }
}

/// A request that reaches this service but names no modelled operation is answered before
/// any operation runs — so no dispatcher produced it, and it announces the protocol's media
/// type. The framework's own rendering for the awsJson family is kept: an unknown operation
/// is a 501 `NotImplemented`, an unreadable routing key a 400 `SerializationException`.
#[tokio::test]
async fn a_response_rendered_before_routing_announces_the_protocols_media_type() {
    let router = router();

    // An `X-Amz-Target` naming an operation this model does not declare.
    for operation in ["NoSuchOperation", "getdashboard", "GetDashboardExtra"] {
        let response = send(&router, operation, &json!({})).await;
        assert_media_type(operation, &response, 501, PROTOCOL_MEDIA_TYPE);
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
    assert_media_type("no x-amz-target", &response, 400, PROTOCOL_MEDIA_TYPE);
}

/// A transport-level failure the router detects before the request is routed at all — a
/// body that declares `aws-chunked` framing and is not framed — reaches no dispatcher
/// either, and announces the protocol's media type.
#[tokio::test]
async fn a_transport_failure_announces_the_protocols_media_type() {
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
    assert_media_type(
        "an aws-chunked body that is not framed",
        &response,
        400,
        PROTOCOL_MEDIA_TYPE,
    );
}

/// The authentication refusal, which the router renders through
/// `definition().refusal_response(..)` before this service is asked anything
/// (`vendor/smithy-server-runtime/src/multi_service_router.rs`): no operation of this
/// service ran, so it announces the protocol's media type.
#[tokio::test]
async fn the_authentication_refusal_announces_the_protocols_media_type() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut authenticating = MultiServiceRouter::new();
    authenticating.set_request_authentication(true);
    authenticating.register_service(EmulatedService::new(engine));

    // The signature in [`AUTHORIZATION`] is not one this emulator vended, so the request is
    // refused before the operation it names runs.
    let response = send(&authenticating, "GetDashboard", &json!({})).await;
    assert_media_type(
        "the authentication refusal",
        &response,
        403,
        PROTOCOL_MEDIA_TYPE,
    );
}

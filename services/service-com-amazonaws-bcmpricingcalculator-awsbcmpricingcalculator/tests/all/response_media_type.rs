// SPDX-License-Identifier: CC0-1.0

//! The media type every response of this service announces.
//!
//! Every recorded response of this service — a success and an error alike — carries
//! `content-type: application/x-amz-json-1.1`, while the media type the model's
//! `aws.protocols#awsJson1_0` trait implies, and which the framework's serializers and
//! pre-dispatch error renderers stamp, is `application/x-amz-json-1.0`. A client (or a
//! recording proxy) selects its JSON-protocol decoder from that header, so the value the
//! service announces is part of its wire contract.
//!
//! The RULE under test: EVERY response this service answers announces
//! `application/x-amz-json-1.1`, whichever layer produced it, while the REQUEST side stays
//! awsJson1_0 — every call below is routed by `X-Amz-Target:
//! AWSBCMPricingCalculator.<Operation>` and sent under `content-type:
//! application/x-amz-json-1.0`, and every one of them is understood.
//!
//! The rows span the layers that can produce a response, because the rule is about all of
//! them and not about one operation: a handler's success, a handler's modelled exception,
//! the modelled input refusal, the account-class gate's refusal, the scaffold's 501, the
//! routing failure rendered before any operation runs, and the transport failure rendered
//! before the request is even routed. The recorded case (one `GetPreferences` success) is
//! ONE row.
//!
//! The one response this service does NOT announce itself — the authentication refusal,
//! which the router renders before the service is asked anything — is pinned at the bottom
//! so the gap is recorded rather than hidden.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{
    state::AccountClass, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::bill_scenario_lifecycle::{request, request_builder, Emulator, REQUEST_MEDIA_TYPE};

/// The media type every response of this service announces, as the wire record shows.
const RESPONSE_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The media type the model's `awsJson1_0` protocol trait implies (smithy_query
/// `query="service_info"` → `protocols: ["awsJson1_0"]`). Every request below is sent under
/// it — it is the harness's own [`REQUEST_MEDIA_TYPE`] — and no response of a routed request
/// may announce it.
const PROTOCOL_MEDIA_TYPE: &str = REQUEST_MEDIA_TYPE;

/// A well-formed `ResourceId` (the lowercase-hex v4 UUID form `^[a-f0-9]{8}-[a-f0-9]{4}-…$`
/// the shape pins) that no live resource of this emulator carries.
const ABSENT_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Call one operation and answer the WHOLE response, headers included — what
/// [`Emulator::call`] parses the body out of, sent through the same request builder.
async fn send(emulator: &Emulator, operation: &str, body: &Value) -> http::Response<Bytes> {
    emulator.router.dispatch(request(Some(operation), body)).await
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

/// Assert the rule on one response: the expected status, the media type this service
/// announces, and a body that is the JSON document that media type promises.
fn assert_media_type(case: &str, response: &http::Response<Bytes>, expected_status: u16) {
    let actual = media_type(response);
    let status = response.status().as_u16();
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} (content-type {actual:?}, \
         body {:?})",
        String::from_utf8_lossy(response.body())
    );
    assert_eq!(
        actual, RESPONSE_MEDIA_TYPE,
        "case {case:?}: expected content-type {RESPONSE_MEDIA_TYPE:?}, got {actual:?}"
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

/// Every response of a routed request announces this service's media type: successes from
/// several handlers, a handler's modelled not-found, the modelled input refusal and the
/// scaffold's 501 come from different layers and all carry the same header.
#[tokio::test]
async fn every_routed_response_announces_the_services_media_type() {
    let emulator = Emulator::new();
    let created = send(&emulator, "CreateWorkloadEstimate", &json!({"name": "mediatype"})).await;
    assert_media_type("the fixture: a created workload estimate", &created, 200);
    let created: Value = serde_json::from_slice(created.body()).expect("a JSON body");
    let estimate_id = created["id"].as_str().expect("an id").to_string();

    let cases = [
        Case {
            // The recorded witness of this issue.
            label: "a handler success: the preferences read",
            operation: "GetPreferences",
            body: json!({}),
            status: 200,
        },
        Case {
            label: "a handler success: the preferences write",
            operation: "UpdatePreferences",
            body: json!({"managementAccountRateTypeSelections": ["AFTER_DISCOUNTS"]}),
            status: 200,
        },
        Case {
            label: "a handler success: a single-resource read",
            operation: "GetWorkloadEstimate",
            body: json!({"identifier": estimate_id}),
            status: 200,
        },
        Case {
            label: "a handler success: a paged listing",
            operation: "ListWorkloadEstimates",
            body: json!({"maxResults": 1}),
            status: 200,
        },
        Case {
            label: "a handler success: an empty listing",
            operation: "ListBillScenarios",
            body: json!({"maxResults": 5}),
            status: 200,
        },
        Case {
            label: "a handler success: a create",
            operation: "CreateBillScenario",
            body: json!({"name": "mediatypescenario"}),
            status: 200,
        },
        Case {
            label: "a handler success: a tag listing of a nested usage plane",
            operation: "ListWorkloadEstimateUsage",
            body: json!({"workloadEstimateId": estimate_id}),
            status: 200,
        },
        Case {
            label: "a handler's not-found answer: ResourceNotFoundException",
            operation: "GetWorkloadEstimate",
            body: json!({"identifier": ABSENT_ID}),
            status: 404,
        },
        Case {
            label: "the modelled input refusal: a name outside the modelled pattern",
            operation: "CreateWorkloadEstimate",
            body: json!({"name": "not a legal name!"}),
            status: 400,
        },
        Case {
            label: "the modelled input refusal: an absent required member",
            operation: "CreateWorkloadEstimate",
            body: json!({}),
            status: 400,
        },
        Case {
            label: "the modelled input refusal: an identifier outside the modelled pattern",
            operation: "GetWorkloadEstimate",
            body: json!({"identifier": "not-a-resource-id"}),
            status: 400,
        },
        Case {
            label: "the scaffold's 501: a listing nobody has implemented yet",
            operation: "ListBillEstimates",
            body: json!({}),
            status: 501,
        },
        Case {
            label: "the scaffold's 501: a read nobody has implemented yet",
            operation: "GetBillEstimate",
            body: json!({"identifier": ABSENT_ID}),
            status: 501,
        },
        // Last: it removes the fixture the rows above read.
        Case {
            label: "a handler success: a delete",
            operation: "DeleteWorkloadEstimate",
            body: json!({"identifier": estimate_id}),
            status: 200,
        },
    ];

    for case in &cases {
        let response = send(&emulator, case.operation, &case.body).await;
        assert_media_type(case.label, &response, case.status);
    }
}

/// The account-class gate refuses from inside the handler, which is a third producer again:
/// its refusal announces the same media type, for every class the gate refuses.
#[tokio::test]
async fn the_account_class_refusal_announces_the_services_media_type() {
    let emulator = Emulator::with_account_class(AccountClass::Member);
    let cases = [
        ("the gate on a create", "CreateBillScenario", json!({"name": "gated"})),
        ("the gate on a listing", "ListBillScenarios", json!({"maxResults": 25})),
        (
            "the gate on a read",
            "GetBillScenario",
            json!({"identifier": ABSENT_ID}),
        ),
        (
            "the gate on the preferences write",
            "UpdatePreferences",
            json!({"memberAccountRateTypeSelections": ["AFTER_DISCOUNTS"]}),
        ),
    ];
    for (label, operation, body) in cases {
        let response = send(&emulator, operation, &body).await;
        assert_media_type(label, &response, 400);
    }
}

/// A request that reaches this service but names no modelled operation is answered before
/// any operation runs — and under the same media type. The framework's own rendering for
/// the awsJson family is kept: an unknown operation is a 501 `NotImplemented`, an
/// unreadable routing key a 400 `SerializationException`.
#[tokio::test]
async fn a_response_rendered_before_routing_announces_the_services_media_type() {
    let emulator = Emulator::new();

    // An `X-Amz-Target` naming an operation this model does not declare: one that does not
    // exist at all, one in the wrong case, and one that extends a real name.
    for operation in ["NoSuchOperation", "getpreferences", "GetPreferencesExtra"] {
        let response = send(&emulator, operation, &json!({})).await;
        assert_media_type(operation, &response, 501);
    }

    // No `X-Amz-Target` at all: the route key still resolves from the credential scope, so
    // the request reaches this service and is refused for the unreadable routing key.
    let response = emulator.router.dispatch(request(None, &json!({}))).await;
    assert_media_type("no x-amz-target", &response, 400);
}

/// A transport-level failure the router detects before the request is routed at all — a
/// body that declares `aws-chunked` framing and is not framed — is answered under the same
/// media type.
#[tokio::test]
async fn a_transport_failure_announces_the_services_media_type() {
    let emulator = Emulator::new();
    let request = request_builder(Some("GetPreferences"))
        .header("content-encoding", "aws-chunked")
        // Declares the framing and carries none of it.
        .body(Bytes::from_static(b"{}"))
        .expect("a well-formed request");

    let response = emulator.router.dispatch(request).await;
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

    // The harness's signature is not one this emulator vended, so the request is refused
    // before the operation it names runs.
    let response = authenticating
        .dispatch(request(Some("GetPreferences"), &json!({})))
        .await;
    let status = response.status().as_u16();
    assert_eq!(
        status, 403,
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

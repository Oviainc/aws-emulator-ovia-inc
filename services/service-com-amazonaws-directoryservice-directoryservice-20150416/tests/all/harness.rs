// SPDX-License-Identifier: CC0-1.0

//! The HTTP plumbing a suite in this crate shares.
//!
//! A case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. Nothing here reaches into the state struct,
//! so a response that is not computed from persisted state cannot pass.
//!
//! This is the module a NEW suite should reach for: the suites written before it
//! (`directory_lifecycle`, which the Region-aware suites import from, and `snapshots`)
//! each carry their own copy of these five functions, and this is the one copy that is
//! not tied to a slice of the service.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::state::TRANSITION_SPAN;
use service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name, `DirectoryService_20150416`.
const TARGET_PREFIX: &str = "DirectoryService_20150416";

/// Comfortably longer than the whole teardown `DeleteDirectory` owes the clock — the
/// `Deleting` -> `Deleted` step and the span the settled record is held for — with margin
/// for a loaded test machine, so a case that waits this long is one the directory's record
/// has LEFT the store in.
///
/// DERIVED from the span the service itself publishes rather than restated, so a change to
/// it moves every wait in the crate with it.
pub(crate) const PAST_THE_TEARDOWN: Duration = TRANSITION_SPAN
    .saturating_add(TRANSITION_SPAN)
    .saturating_add(Duration::from_millis(400));

/// Delete the directory `directory_id` and wait until the teardown that delete started has
/// run out, so the record has left the store by the time the caller asserts on its absence.
///
/// `DeleteDirectory` answers while the directory is still being torn down — it is reported
/// in `Deleting`, then in `Deleted`, before the record goes — so a case about what the
/// account no longer holds waits that chain out here rather than restating it.
pub(crate) async fn delete_directory_and_wait(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) {
    ok(
        router,
        "DeleteDirectory",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    tokio::time::sleep(PAST_THE_TEARDOWN).await;
}

/// A router with just this service mounted on a fresh engine.
pub(crate) fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise the service, not SigV4: requests carry a credential scope so
    // the router can name the target service, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One `awsJson1_1` call, returning the WHOLE response — headers included.
///
/// The seam [`call`] is built on, for a rule about the response envelope itself (the
/// request-id header a refusal carries) rather than about its body.
pub(crate) async fn call_raw(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> http::Response<bytes::Bytes> {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "ds.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDDSTEST/20260101/us-east-1/ds/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    router.dispatch(request).await
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let response = call_raw(router, operation, body).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
pub(crate) async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 and the given modelled error code.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected {expected_code} from {operation}, got {parsed}"
    );
}

/// A required string member of a response body.
pub(crate) fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

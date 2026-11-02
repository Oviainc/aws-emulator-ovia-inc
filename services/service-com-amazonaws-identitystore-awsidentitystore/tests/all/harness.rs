// SPDX-License-Identifier: CC0-1.0

//! The front door this crate's suites drive, in ONE place.
//!
//! Every case goes through the same door a client uses: `POST /` with
//! `X-Amz-Target: AWSIdentityStore.<Operation>` and an `awsJson1_1` body
//! (OPERATIONS.md, "Wire protocol"), dispatched by the production
//! [`MultiServiceRouter`]. Nothing here reaches into the state struct, so a response
//! that is not computed from persisted state cannot pass.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_identitystore_awsidentitystore as identitystore;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from OPERATIONS.md, "Wire protocol".
const TARGET_PREFIX: &str = "AWSIdentityStore";

/// The identity store this emulator ADVERTISES: the store IAM Identity Center
/// provisioned for the instance `sso-admin` seeds, which is what `ListInstances` points a
/// client at (`identitystore::state::SEED_IDENTITY_STORE_ID`). Most suites address this
/// one, but it is not privileged: this API creates no identity store, so every
/// well-formed `IdentityStoreId` of this account names the store its caller has and is
/// served.
pub const STORE: &str = identitystore::state::SEED_IDENTITY_STORE_ID;

/// An `IdentityStoreId` the model admits that names NO identity store this emulator
/// serves, so a call addressed to it is refused rather than served.
///
/// It is an ARN, because the ARN form is what carries the OWNING ACCOUNT: this one names
/// a store of ANOTHER account (`210987654321`, which is not
/// `engine::emulator_account_id()`), which is not a store this emulator holds however its
/// id ends. A bare `d-…` id would NOT do: the modelled pattern is all that decides
/// well-formedness, and a well-formed id of this account names the caller's own store.
pub const MISSING_STORE: &str =
    "arn:aws:identitystore::210987654321:identitystore/d-9999999999";

/// The access key id a call is signed with when a case does not care which caller
/// makes it.
pub const DEFAULT_AKID: &str = "AKIDIDENTITYSTORE";

/// The HTTP status this service answers `ResourceNotFoundException` with.
///
/// Named once here because it is a property of the ERROR CLASS and not of any one
/// operation: every suite that expects a resource this service does not hold expects
/// this status beside the not-found error code
/// (`identitystore::service::error_status`).
pub const NOT_FOUND: u16 = 400;

/// A router with just this service mounted on a fresh engine.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the request still carries a SigV4
    // credential scope, which is what routes it to this service and names its caller.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_identitystore_awsidentitystore::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call made by the caller whose access key id is `akid`: the HTTP
/// status, the wire error code and the parsed body.
pub async fn call_as(
    router: &MultiServiceRouter,
    akid: &str,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("host", "identitystore.us-east-1.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/us-east-1\
                 /identitystore/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    let code = if header_code.is_empty() {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    (status, code, parsed)
}

/// [`call_as`] for the default caller.
pub async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    call_as(router, DEFAULT_AKID, operation, body).await
}

/// A call by `akid` that must answer 200, returning its body.
pub async fn ok_as(
    router: &MultiServiceRouter,
    akid: &str,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, code, parsed) = call_as(router, akid, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {code} with {parsed}"
    );
    parsed
}

/// A call that must answer 200, returning its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    ok_as(router, DEFAULT_AKID, operation, body, case).await
}

/// A call by `akid` that must be refused with `status` and an error code naming `error`.
pub async fn refused_as(
    router: &MultiServiceRouter,
    akid: &str,
    operation: &str,
    body: Value,
    status: u16,
    error: &str,
    case: &str,
) -> Value {
    let (actual_status, code, parsed) = call_as(router, akid, operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: expected {operation} to answer {status} {error}, \
         got {actual_status} {code} with {parsed}"
    );
    assert!(
        code.ends_with(error),
        "case {case:?}: expected {operation} to answer {error}, got {code:?} with {parsed}"
    );
    parsed
}

/// A call that must be refused with `status` and an error code naming `error`.
pub async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    status: u16,
    error: &str,
    case: &str,
) -> Value {
    refused_as(router, DEFAULT_AKID, operation, body, status, error, case).await
}

/// A required string member of a response body.
pub fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The `Message` of an error body, whichever casing the protocol rendered it in.
pub fn error_message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the service through:
//! `awsJson1_1`, `POST /` with `X-Amz-Target: TrentService.<Operation>`, dispatched
//! by the same multi-service router the emulator boots.
//!
//! It holds ONLY the plumbing — build a router, call an operation, read a member.
//! Every assertion lives in the suite that makes it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for. Keys are regional, so a case that is about
/// regional scoping names its own.
pub(crate) const REGION: &str = "us-east-1";

/// A router with only this service mounted, over one shared engine — the same
/// assembly `core/router` performs, minus the other services.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // In-process tests sign with a placeholder key; signature verification is a
    // front-door setting, not part of what these tests are about.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation in `region` and return its HTTP status and JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://kms.{region}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEMULATOR/20260101/{region}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Call one operation in [`REGION`], requiring success.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    ok_in(router, REGION, operation, body).await
}

/// Call one operation in `region`, requiring success.
pub(crate) async fn ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, payload) = call(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// Call one operation in [`REGION`], requiring a failure, and report its status and
/// wire error code.
pub(crate) async fn failure(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String) {
    failure_in(router, REGION, operation, body).await
}

/// [`failure`], in `region`.
pub(crate) async fn failure_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, String) {
    let (status, payload) = call(router, region, operation, body).await;
    assert_ne!(
        status, 200,
        "case {operation} {body}: expected a failure, got HTTP 200 with {payload}"
    );
    (status, error_code(&payload))
}

/// The wire error code (`__type`) of an error body, without its namespace.
pub(crate) fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A blob as awsJson1_1 carries it.
pub(crate) fn b64(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
}

/// The bytes behind a blob member.
pub(crate) fn unb64(case: &str, payload: &Value, member: &str) -> Vec<u8> {
    let encoded = payload[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected {member} in {payload}"));
    aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case}: {member} is not base64: {error}"))
}

/// Create a key and return its `KeyMetadata`.
pub(crate) async fn create_key(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "CreateKey", request).await["KeyMetadata"].clone()
}

/// The `KeyMetadata` `DescribeKey` reports for an identifier.
pub(crate) async fn describe(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "DescribeKey", &json!({ "KeyId": identifier })).await["KeyMetadata"].clone()
}

/// A member of a metadata object, as a string.
pub(crate) fn text(case: &str, value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {member} in {value}"))
        .to_string()
}

/// Whether `id` has the shape of a KMS key id: a version-4 UUID (8-4-4-4-12
/// lowercase hex with the version nibble `4`) for a single-Region key, and `mrk-`
/// followed by 32 lowercase hex characters for a multi-Region one.
pub(crate) fn is_key_id(id: &str, multi_region: bool) -> bool {
    if multi_region {
        return id
            .strip_prefix("mrk-")
            .is_some_and(|hex| hex.len() == 32 && is_lower_hex(hex));
    }
    let groups: Vec<&str> = id.split('-').collect();
    let lengths = [8usize, 4, 4, 4, 12];
    groups.len() == lengths.len()
        && groups
            .iter()
            .zip(lengths)
            .all(|(group, len)| group.len() == len && is_lower_hex(group))
        && groups[2].starts_with('4')
}

/// Whether every character is a lowercase hexadecimal digit.
pub(crate) fn is_lower_hex(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Whether `haystack` contains `needle` as a contiguous run — how a test checks
/// that a ciphertext does not simply carry its plaintext.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|window| window == needle)
}

/// A symmetric encryption key in the `Enabled` state.
pub(crate) async fn enabled_key(router: &MultiServiceRouter) -> Value {
    create_key(router, &json!({"Description": "round trip"})).await
}

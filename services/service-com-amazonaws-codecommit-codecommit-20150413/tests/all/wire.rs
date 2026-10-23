// SPDX-License-Identifier: CC0-1.0

//! Calling the emulator AS A NAMED PRINCIPAL.
//!
//! The suites' default harness — building the emulator, sending a call, reading
//! an answer and the small value helpers — lives in
//! [`crate::repository_lifecycle`] and signs every request with
//! [`DEFAULT_ACCESS_KEY_ID`]. This module is the one place a request's SigV4
//! access key id is chosen, because a service that reads a caller's identity from
//! it (a comment's `authorArn`, the owner of an emoji reaction) can only be
//! tested by calling as two different principals. That default harness builds its
//! requests through [`call_as`] too, so there is exactly one request builder.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::REGION;

/// The access key id a request is signed with unless a suite is checking that the
/// service tells two callers apart.
pub(crate) const DEFAULT_ACCESS_KEY_ID: &str = "AKIDTESTKEY";

/// The ARN the service reports for a caller signing with `access_key_id` — what a
/// comment's `authorArn` and a reaction's owner are read back as.
pub(crate) fn caller_arn(access_key_id: &str) -> String {
    format!(
        "arn:aws:iam::{}:user/{access_key_id}",
        engine::emulator_account_id()
    )
}

/// Send one awsJson1_1 request: `POST /` with `X-Amz-Target:
/// CodeCommit_20150413.<Operation>` (the crate's OPERATIONS.md "Wire protocol"),
/// signed with `access_key_id`, which is what the service reads a caller's
/// identity from.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://codecommit.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeCommit_20150413.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/codecommit/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "operation {operation}: response body is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// A successful call by a named caller, with the status asserted.
pub(crate) async fn ok_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, payload) = call_as(router, access_key_id, operation, body).await;
    assert_eq!(
        status, 200,
        "operation {operation} as {access_key_id}: expected 200, got {status} with {payload}"
    );
    payload
}

/// The modeled error code of a failed call by a named caller, with the status
/// asserted. awsJson carries it in `__type`, optionally namespace-qualified.
pub(crate) async fn error_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
) -> String {
    refusal_as(router, access_key_id, operation, body, expected_status)
        .await
        .0
}

/// The modeled error code AND message of a failed call by a named caller, with
/// the status asserted: the same parse as [`error_as`], for a suite that asserts
/// what the error SAYS as well as which one it is. awsJson carries the code in
/// `__type` (optionally namespace-qualified) and the message in `message`.
pub(crate) async fn refusal_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
) -> (String, String) {
    let (status, payload) = call_as(router, access_key_id, operation, body).await;
    assert_eq!(
        status, expected_status,
        "operation {operation} as {access_key_id}: expected HTTP {expected_status}, got {status} \
         with {payload}"
    );
    let code = payload
        .get("__type")
        .and_then(Value::as_str)
        .map(|shape| shape.rsplit('#').next().unwrap_or(shape).to_string())
        .unwrap_or_else(|| panic!("operation {operation}: no __type in {payload}"));
    let message = payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("operation {operation}: no message in {payload}"))
        .to_string();
    (code, message)
}

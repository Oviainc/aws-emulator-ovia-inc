// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the quota on the SIZE of a request that carries an archive INLINE:
//! `CreateFunction` and `UpdateFunctionCode` (a deployment package in `ZipFile`) and
//! `PublishLayerVersion` (a layer archive in `Content.ZipFile`).
//!
//! The rule under test: a request whose body exceeds the quota AWS publishes for the
//! operation it addresses is refused HTTP 413 with the error code
//! `RequestEntityTooLargeException` — the modelled `RequestTooLargeException`'s status
//! (`smithy_query(query="shape", shape="RequestTooLargeException")`: `@error("client")` with
//! `@httpError(413)`) under the code AWS's answer names on the wire — stating the quota and
//! the operation: `Request must be smaller than <quota> bytes for the <operation>
//! operation`. A refusal on the request's SIZE is not a refusal about what the request would
//! have stored, so it carries no modelled 400 and happens before the body is read as an
//! input at all.
//!
//! The rows differ along what the rule generalises over: which operation is addressed —
//! each has its own published quota, and the sentence names both the quota and the operation
//! that was addressed — and the size of the request, at that operation's quota and one byte
//! past it, so the boundary is asserted rather than one recorded size.

use std::sync::Arc;

use crate::function_lifecycle::{router_on, send, Answer};

/// The error code AWS's answer to such a request names on the wire.
const TOO_LARGE_CODE: &str = "RequestEntityTooLargeException";

/// One addressed operation: how the request reaches it, the name its refusal states, and
/// the quota that refusal states.
struct Addressed {
    /// The operation, as the refusal names it.
    operation: &'static str,
    /// The method the operation is bound to.
    method: &'static str,
    /// The URI it is bound to, with every label filled in.
    uri: &'static str,
    /// The quota AWS publishes for its request, in bytes, which its refusal states. It is a
    /// quota on the whole REQUEST rather than on the archive — an archive travels inline as
    /// base64 — so it is neither the account's `CodeSizeZipped` limit nor a fixed multiple
    /// of it, and the operations do not share one number.
    quota: usize,
}

/// Every operation of this service that carries an archive inline
/// (`smithy_query(query="operation", operation="CreateFunction,UpdateFunctionCode,\
/// PublishLayerVersion")` for each binding).
const ADDRESSED: &[Addressed] = &[
    Addressed {
        operation: "CreateFunction",
        method: "POST",
        uri: "/2015-03-31/functions",
        quota: 69_905_067,
    },
    Addressed {
        operation: "UpdateFunctionCode",
        method: "PUT",
        uri: "/2015-03-31/functions/quota-probe/code",
        quota: 70_167_211,
    },
    Addressed {
        operation: "PublishLayerVersion",
        method: "POST",
        uri: "/2018-10-31/layers/quota-probe/versions",
        quota: 70_167_211,
    },
];

/// A JSON request body of exactly `total` bytes, padded with a member nothing reads: the
/// rule is about the SIZE of the request, so the padding carries no value of the operation's
/// own.
fn body_of_size(total: usize) -> Vec<u8> {
    let envelope = br#"{"Padding":""}"#.len();
    assert!(
        total > envelope,
        "a body of {total} bytes cannot hold the padding envelope"
    );
    let mut body = Vec::with_capacity(total);
    body.extend_from_slice(br#"{"Padding":""#);
    body.resize(total - 2, b'A');
    body.extend_from_slice(br#""}"#);
    assert_eq!(
        body.len(),
        total,
        "the test needs a request body of exactly {total} bytes"
    );
    body
}

/// One request of `bytes` bytes to `addressed`.
async fn sent(
    router: &smithy_server_runtime::MultiServiceRouter,
    addressed: &Addressed,
    bytes: usize,
) -> Answer {
    send(
        router,
        addressed.method,
        addressed.uri,
        "application/json",
        body_of_size(bytes),
        &[],
    )
    .await
}

/// A request past the quota published for the operation it addresses is refused `413`,
/// naming that quota and that operation — and a request of exactly the quota is not refused
/// on its size at all.
#[tokio::test]
async fn an_inline_archive_request_is_refused_past_the_quota_of_the_operation_it_addresses() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for addressed in ADDRESSED {
        let operation = addressed.operation;
        let quota = addressed.quota;

        let refused = sent(&router, addressed, quota + 1).await;
        assert_eq!(
            refused.status, 413,
            "case {operation}, one byte over the quota: expected HTTP 413, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.header("x-amzn-errortype").as_deref(),
            Some(TOO_LARGE_CODE),
            "case {operation}, one byte over the quota: expected the too-large error code, \
             got {:?}",
            refused.header("x-amzn-errortype")
        );
        let expected = format!(
            "Request must be smaller than {quota} bytes for the {operation} operation"
        );
        assert_eq!(
            refused.string("Message"),
            Some(expected.clone()),
            "case {operation}, one byte over the quota: expected {expected:?}, got {}",
            String::from_utf8_lossy(&refused.body)
        );

        // Exactly the quota is within it: whatever the request is then refused for, it is
        // not refused for its size.
        let served = sent(&router, addressed, quota).await;
        assert_ne!(
            served.status, 413,
            "case {operation}, exactly the quota: expected no size refusal, got {} {}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_ne!(
            served.error_code(),
            TOO_LARGE_CODE,
            "case {operation}, exactly the quota: expected no size refusal, got {}",
            served.error_code()
        );
    }
}

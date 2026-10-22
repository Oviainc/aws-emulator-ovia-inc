// SPDX-License-Identifier: CC0-1.0

//! A probe for the operations this slice did NOT implement.
//!
//! The domain -> space lifecycle is the first buildable slice of this service; every
//! other operation is still a scaffold. The contract for those is an honest HTTP 501
//! `NotImplemented` — never an empty 200 that a caller cannot tell from a real
//! implementation — so this suite pins it over the wire for an operation outside the
//! slice, and pins that the implemented ones have left the 501 behind.
//!
//! Keep this suite as the slice grows: when an operation here is implemented, move it
//! from the 501 row to the implemented row.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

/// Dispatch `operation` with an empty body — which the rpcv2Cbor protocol reads as an
/// empty input map, so an operation whose input is all-optional is fully specified.
async fn status_of(operation: &str) -> (u16, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(omni::EmulatedService::new(engine));
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "/service/CloudWatchOmniFrontend/operation/{operation}"
        ))
        .header("content-type", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/us-west-2/cloudwatch/aws4_request, \
             SignedHeaders=host, Signature=emulator",
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed rpcv2Cbor request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

#[tokio::test]
async fn an_operation_outside_this_slice_still_answers_501() {
    // Both rows take an all-optional input, so an empty body is a complete request
    // and the only thing that can decide the answer is whether the operation is
    // implemented.
    for operation in ["ListDomains", "ListIntegrations"] {
        let (status, body) = status_of(operation).await;
        assert_eq!(
            status, 501,
            "case {operation:?}: a scaffolded operation must answer 501, got {status}: {body:?}"
        );
        assert!(
            body.contains("NotImplemented"),
            "case {operation:?}: the 501 body must name NotImplemented, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_operation_in_this_slice_no_longer_answers_501() {
    // `ListSpaces` is in the slice and also takes an all-optional input, so the same
    // empty-body request that proves the rows above are stubs proves this one is not.
    let (status, body) = status_of("ListSpaces").await;
    assert_eq!(
        status, 200,
        "ListSpaces is implemented and must serve an empty store, got {status}: {body:?}"
    );
    assert!(
        !body.contains("NotImplemented"),
        "ListSpaces must not answer the scaffold signal, got {body:?}"
    );
}

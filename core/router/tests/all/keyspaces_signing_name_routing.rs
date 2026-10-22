// SPDX-License-Identifier: CC0-1.0

//! The `cassandra` signing-name split, on the PRODUCTION router.
//!
//! Amazon Keyspaces (`KeyspacesService.<Op>`, endpoint `cassandra.<region>.…`) and
//! KeyspacesStreams (`KeyspacesStreams.<Op>`, endpoint `cassandra-streams.<region>.…`)
//! share one SigV4 signing name, so the signing name alone cannot say which service a
//! signed request is for — and a request that TWO services claim is answered by
//! NEITHER: the router refuses an ambiguous claim with a 500. Every signed AWS SDK
//! request reaches this emulator through `build_router`, with every other service
//! mounted beside these two, so this suite pins the claim where it actually has to
//! hold: on the assembled router.
//!
//! The split's other rules (each service's own target, a request signed for a third
//! service, and the target-before-host precedence) are covered over a two-service
//! router in
//! `services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/tests/all/signing_name_routing.rs`,
//! which needs no full assembly.
//!
//! Assembling the router costs stack, so the case runs on an explicitly sized thread,
//! exactly as `boot_serve` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

/// Stack for assembling the router, matching `boot_serve`'s.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 `Authorization` header whose credential scope names `cassandra` — what an
/// AWS SDK sends for BOTH of these services, and the signal that makes the signing
/// name ambiguous. Signature verification is off in this test process, so only the
/// credential scope matters here.
const CASSANDRA_SIGV4: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/cassandra/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A signed KeyspacesStreams request is claimed by exactly one of the services the
/// production router mounts, and that service is KeyspacesStreams: it answers 200 with
/// a stream list, not 500 `AmbiguousService`.
#[test]
fn a_signed_keyspaces_streams_request_reaches_the_streams_service() {
    let (status, body) = std::thread::Builder::new()
        .name("keyspaces-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let router = build_router(engine);
            let request = http::Request::builder()
                .method("POST")
                .uri("https://localhost/")
                .header("authorization", CASSANDRA_SIGV4)
                .header("content-type", "application/x-amz-json-1.0")
                .header("x-amz-target", "KeyspacesStreams.ListStreams")
                .body(bytes::Bytes::from_static(b"{}"))
                .expect("build the request");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a runtime");
            let response = runtime.block_on(router.dispatch(request));
            (
                response.status().as_u16(),
                String::from_utf8_lossy(response.body()).to_string(),
            )
        })
        .expect("spawn the routing thread")
        .join()
        .expect("the routing thread must not panic");

    assert_eq!(
        status, 200,
        "case KeyspacesStreams.ListStreams: exactly one mounted service must claim it, got \
         {status} {body}"
    );
    assert!(
        body.contains("\"streams\""),
        "case KeyspacesStreams.ListStreams: the streams service must answer it, got {body}"
    );
}

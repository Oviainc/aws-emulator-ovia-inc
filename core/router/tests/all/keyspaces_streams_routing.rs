// SPDX-License-Identifier: CC0-1.0

//! Amazon Keyspaces and Keyspaces Streams are TWO services behind ONE SigV4 signing
//! name (`cassandra` — both models' `aws.api#service` trait gives them that
//! `sigv4Name`, with the distinct `cassandra` and `cassandra-streams` endpoint
//! prefixes), which no Smithy trait expresses. With both mounted, the default
//! signing-name `can_handle` makes both claim every `cassandra`-signed request and the
//! router answers `500 AmbiguousService` for all of it — so each service's traffic is
//! told apart by the `X-Amz-Target` prefix the awsJson1_0 request carries.
//!
//! This suite needs the WHOLE router: the ambiguity only exists when both services are
//! mounted, so a single-service test cannot see it.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming `cassandra` — what BOTH services' requests are
/// signed with, and therefore the whole problem.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cassandra/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

#[test]
fn keyspaces_and_keyspaces_streams_route_apart_under_one_signing_name() {
    std::thread::Builder::new()
        .name("keyspaces-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Each row: the `X-Amz-Target` a request carries, its body, and the status
            // the OWNING service answers when that operation is implemented. What this
            // suite is about is that neither service answers 500 (`AmbiguousService`)
            // or 404 (`UnknownService`) — every request must reach exactly one of them
            // — so the streams row carries `None`: pinning its scaffold 501 would fail
            // the day `ListStreams` is implemented, for a reason that has nothing to do
            // with routing.
            let cases: [(&str, &str, &[u8], Option<u16>); 4] = [
                (
                    "the keyspaces control plane",
                    "KeyspacesService.CreateKeyspace",
                    br#"{"keyspaceName":"routing_probe"}"#,
                    Some(200),
                ),
                (
                    "a keyspaces operation that reads an absent table",
                    "KeyspacesService.GetTable",
                    br#"{"keyspaceName":"routing_probe","tableName":"absent"}"#,
                    // Keyspaces answers every client-side exception on its awsJson1_0
                    // wire with the protocol's own 400 (the error TYPE in the body is
                    // what names the failure), so this row's 400 is the OWNING service
                    // answering a modelled `ResourceNotFoundException` rather than a
                    // routing failure.
                    Some(400),
                ),
                (
                    "a keyspaces operation that reads the keyspace just created",
                    "KeyspacesService.ListTables",
                    br#"{"keyspaceName":"routing_probe"}"#,
                    Some(200),
                ),
                (
                    "the streams consumer surface",
                    "KeyspacesStreams.ListStreams",
                    b"{}",
                    None,
                ),
            ];

            for (case, target, body, expected_status) in cases {
                let request = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("authorization", AUTHORIZATION)
                    .header("x-amz-target", target)
                    .header("content-type", "application/x-amz-json-1.0")
                    .body(bytes::Bytes::copy_from_slice(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_ne!(
                    status, 500,
                    "case {case} ({target}): the request was claimed by BOTH services \
                     (AmbiguousService), got {status} with {rendered}"
                );
                if expected_status != Some(404) {
                    assert_ne!(
                        status, 404,
                        "case {case} ({target}): the request was claimed by NEITHER \
                         service (UnknownService), got {status} with {rendered}"
                    );
                }
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {case} ({target}): routing failed, got {status} with {rendered}"
                );
                if let Some(expected_status) = expected_status {
                    assert_eq!(
                        status, expected_status,
                        "case {case} ({target}): the owning service must answer HTTP \
                         {expected_status}, got {status} with {rendered}"
                    );
                }
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("Amazon Keyspaces and Keyspaces Streams must route apart");
}

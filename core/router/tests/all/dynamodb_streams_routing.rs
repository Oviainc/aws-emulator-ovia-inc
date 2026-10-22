// SPDX-License-Identifier: CC0-1.0

//! DynamoDB and DynamoDB Streams are TWO services behind ONE SigV4 signing name
//! (`dynamodb`), which no Smithy trait expresses. With both mounted, the default
//! signing-name `can_handle` makes both claim every DynamoDB-signed request and the
//! router answers `500 AmbiguousService` for all of it — so each service's traffic
//! is told apart by the `X-Amz-Target` prefix the awsJson1_0 request carries.
//!
//! This suite needs the WHOLE router: the ambiguity only exists when both services
//! are mounted, so a single-service test cannot see it.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming `dynamodb` — what BOTH services' requests are
/// signed with, and therefore the whole problem.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/dynamodb/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

#[test]
fn dynamodb_and_dynamodb_streams_route_apart_under_one_signing_name() {
    std::thread::Builder::new()
        .name("dynamodb-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Each row: the `X-Amz-Target` a request carries, its body, and the
            // status the OWNING service answers when that operation is implemented.
            // What this suite is about is that neither service answers 500
            // (`AmbiguousService`) or 404 (`UnknownService`) — every request must
            // reach exactly one of them — so the DynamoDB row carries `None`: pinning
            // its scaffold 501 would fail the day `ListTables` is implemented, for a
            // reason that has nothing to do with routing.
            let cases: [(&str, &str, &[u8], Option<u16>); 3] = [
                (
                    "the streams consumer surface",
                    "DynamoDBStreams_20120810.ListStreams",
                    b"{}",
                    Some(200),
                ),
                (
                    "a streams operation that reads an absent stream",
                    "DynamoDBStreams_20120810.DescribeStream",
                    br#"{"StreamArn":"arn:aws:dynamodb:us-east-1:000000000000:table/absent/stream/2020-01-01T00:00:00.000"}"#,
                    Some(400),
                ),
                (
                    "a DynamoDB control-plane operation",
                    "DynamoDB_20120810.ListTables",
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
                    .body(bytes::Bytes::from_static(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_ne!(
                    status, 500,
                    "case {case} ({target}): the request was claimed by BOTH services \
                     (AmbiguousService), got {status} with {rendered}"
                );
                assert_ne!(
                    status, 404,
                    "case {case} ({target}): the request was claimed by NEITHER service \
                     (UnknownService), got {status} with {rendered}"
                );
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
        .expect("DynamoDB and DynamoDB Streams must route apart");
}

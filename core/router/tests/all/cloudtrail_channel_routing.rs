// SPDX-License-Identifier: CC0-1.0

//! The CloudTrail Lake CHANNEL operations, through the WHOLE router.
//!
//! A service-level suite mounts one service, so it cannot see a request being claimed
//! by another service mounted beside it — and CloudTrail Data sits behind this
//! account's mounted services too. This suite drives the channel calls an SDK makes,
//! under the dotted target prefix an SDK sends
//! (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.<Operation>`, the prefix
//! `cloudtrail_sdk_target_routing.rs` pins the reading of), and asserts each reaches
//! the CloudTrail handler and answers the operation's own modelled refusal — never the
//! dispatcher's `NotImplemented`, `AmbiguousService` or `UnknownService`.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming `cloudtrail`, which is how an SDK signs every
/// CloudTrail request.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/cloudtrail/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The target prefix an AWS SDK sends for CloudTrail (botocore's `targetPrefix`).
const SDK_TARGET_PREFIX: &str = "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.";

/// A well-formed channel ARN naming a channel no call has created.
const ABSENT_CHANNEL: &str =
    "arn:aws:cloudtrail:us-west-2:000000000000:channel/39032a83-30fe-4581-8bd3-351632c2be72";

#[test]
fn cloudtrail_channel_operations_reach_their_handler_in_the_whole_router() {
    std::thread::Builder::new()
        .name("cloudtrail-channel-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Each row: the operation, its body, and the status and error code the
            // CloudTrail handler answers with. Every row names a channel that does not
            // exist, which is the one answer that needs no setup — and the recorded
            // status for it is 400, not the `@httpError(404)` the shape declares.
            let cases: [(&str, String, u16, &str); 3] = [
                (
                    "UpdateChannel",
                    format!(r#"{{"Channel":"{ABSENT_CHANNEL}","Name":"renamed"}}"#),
                    400,
                    "ChannelNotFoundException",
                ),
                (
                    "GetChannel",
                    format!(r#"{{"Channel":"{ABSENT_CHANNEL}"}}"#),
                    400,
                    "ChannelNotFoundException",
                ),
                (
                    "UpdateChannel",
                    r#"{"Channel":"arn:aws:cloudtrail:us-west-2:000000000000:badchannel"}"#
                        .to_string(),
                    400,
                    "ChannelARNInvalidException",
                ),
            ];

            for (operation, body, expected_status, expected_code) in cases {
                let target = format!("{SDK_TARGET_PREFIX}{operation}");
                let request = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("authorization", AUTHORIZATION)
                    .header("x-amz-target", target.as_str())
                    .header("content-type", "application/x-amz-json-1.1")
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {operation}: the request reached no single service, got {status} \
                     with {rendered}"
                );
                assert!(
                    !rendered.contains("NotImplemented"),
                    "case {operation}: the operation did not route, got {status} with \
                     {rendered}"
                );
                assert_eq!(
                    status, expected_status,
                    "case {operation}: expected HTTP {expected_status}, got {status} with \
                     {rendered}"
                );
                assert!(
                    rendered.contains(expected_code),
                    "case {operation}: expected the error code {expected_code}, got {status} \
                     with {rendered}"
                );
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("every CloudTrail channel operation must reach its handler");
}

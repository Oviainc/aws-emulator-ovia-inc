// SPDX-License-Identifier: CC0-1.0

//! CloudTrail's `awsJson1_1` `X-Amz-Target` carries a DOTTED target prefix
//! (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.<Operation>`), not the
//! bare service shape name every other awsJson service sends. The framework's
//! routing key is everything after the FIRST dot of that header, so the generated
//! routing table — keyed by the operation's own name — matched nothing and EVERY
//! CloudTrail operation answered 501 `NotImplemented` from the dispatcher, including
//! the ones whose handlers are complete. The service resolves the operation from the
//! target's last dot-separated segment instead (`Service::override_operation`).
//!
//! This suite needs the WHOLE router: CloudTrail and CloudTrail Data both sit behind
//! this account's mounted services, so only the assembled router shows that each
//! request reaches exactly one of them.
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
     Credential=AKIDEXAMPLE/20260101/us-east-1/cloudtrail/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The target prefix an AWS SDK sends for CloudTrail (botocore's `targetPrefix`).
const SDK_TARGET_PREFIX: &str = "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.";

/// The target prefix the model's service shape alone gives.
const MODEL_TARGET_PREFIX: &str = "CloudTrail_20131101.";

#[test]
fn cloudtrail_operations_route_under_the_target_prefix_an_sdk_sends() {
    std::thread::Builder::new()
        .name("cloudtrail-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Each row: the case, the `X-Amz-Target`, the body, and the status the
            // owning service answers. Both prefixes name the same operations, and an
            // implemented one must answer its own modelled status — never the
            // dispatcher's 501.
            let cases: [(&str, String, &[u8], u16); 6] = [
                (
                    "CreateDashboard under the SDK's prefix",
                    format!("{SDK_TARGET_PREFIX}CreateDashboard"),
                    br#"{"Name":"router-dashboard"}"#,
                    200,
                ),
                (
                    "GetDashboard reads it back under the SDK's prefix",
                    format!("{SDK_TARGET_PREFIX}GetDashboard"),
                    br#"{"DashboardId":"router-dashboard"}"#,
                    200,
                ),
                (
                    "GetDashboard for a dashboard nobody created",
                    format!("{SDK_TARGET_PREFIX}GetDashboard"),
                    br#"{"DashboardId":"absent-dashboard"}"#,
                    400,
                ),
                (
                    "GetDashboard under the model's own prefix",
                    format!("{MODEL_TARGET_PREFIX}GetDashboard"),
                    br#"{"DashboardId":"router-dashboard"}"#,
                    200,
                ),
                (
                    "a trail read under the SDK's prefix",
                    format!("{SDK_TARGET_PREFIX}GetTrail"),
                    br#"{"Name":"absent-trail"}"#,
                    // `TrailNotFoundException` carries `@httpError(404)` in the model
                    // (`smithy_query(query="operation", operation="GetTrail")`), so a
                    // routed read of an absent trail answers 404 — not the
                    // dispatcher's 501.
                    404,
                ),
                (
                    "a trail listing under the SDK's prefix",
                    format!("{SDK_TARGET_PREFIX}ListTrails"),
                    b"{}",
                    200,
                ),
            ];

            for (case, target, body, expected_status) in cases {
                let request = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("authorization", AUTHORIZATION)
                    .header("x-amz-target", target.as_str())
                    .header("content-type", "application/x-amz-json-1.1")
                    .body(bytes::Bytes::copy_from_slice(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {case} ({target}): the request reached no single service, got \
                     {status} with {rendered}"
                );
                assert!(
                    !rendered.contains("NotImplemented"),
                    "case {case} ({target}): the operation did not route, got {status} with \
                     {rendered}"
                );
                assert_eq!(
                    status, expected_status,
                    "case {case} ({target}): expected HTTP {expected_status}, got {status} \
                     with {rendered}"
                );
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("every CloudTrail operation must route under the SDK's target prefix");
}

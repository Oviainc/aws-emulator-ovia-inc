// SPDX-License-Identifier: CC0-1.0

//! `GetResourceSyncStatus` is reachable under every form of this service's RPC target
//! prefix.
//!
//! The rule: an awsJson request names its operation in
//! `x-amz-target: <target prefix>.<OperationName>`; an operation name is a Smithy
//! identifier and carries no `.`, while the target prefix is opaque text an SDK takes
//! from its model — so the operation is the LAST dot-separated segment, whatever the
//! prefix. `sync_blocker_summary.rs` pins that rule service-wide (two operations and a
//! target naming no modeled operation); this suite pins it for the operation
//! implemented here, and `target_probe.rs` runs the census over the whole surface.
//!
//! The helpers below (`call_target`, the two prefix forms) are what those suites share.

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{REGION, router};
use super::git_sync::error_code;

/// The bare service shape name, the prefix the Smithy model's `x-amz-target` binding
/// states (`smithy_query(query="service_info")` names the service shape
/// `CodeConnections_20231201`).
const SHAPE_NAME_PREFIX: &str = "CodeConnections_20231201";

/// The service's ABSOLUTE shape id as a target prefix
/// (`com.amazonaws.codeconnections#CodeConnections_20231201` with `#` as `.`): the form
/// an AWS SDK client built from this service's model sends.
pub(crate) const SHAPE_ID_PREFIX: &str = "com.amazonaws.codeconnections.CodeConnections_20231201";

/// Both prefix forms a request may carry.
const TARGET_PREFIXES: &[&str] = &[SHAPE_NAME_PREFIX, SHAPE_ID_PREFIX];

/// Call one operation with an explicit `x-amz-target`, through the real router.
pub(crate) async fn call_target(
    router: &MultiServiceRouter,
    target: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTCODECONN/20261021/{REGION}/codeconnections/aws4_request, \
                 SignedHeaders=content-type;host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "target {target}: body is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// `GetResourceSyncStatus` answers from its own handler under either prefix: the same
/// modeled refusal for the same request, never the framework's 501 for a target that
/// named no operation. One row per prefix, and one per refusal the handler can give, so
/// routing is not keyed to one shape of request.
#[tokio::test]
async fn get_resource_sync_status_is_reachable_under_either_target_prefix() {
    // (case, body, expected status, expected error code)
    let rows = [
        (
            "a resource with no sync configuration",
            serde_json::json!({ "SyncType": "CFN_STACK_SYNC", "ResourceName": "AbsentStack" }),
            400,
            "InvalidInputException",
        ),
        (
            "a sync type outside the modeled enum",
            serde_json::json!({ "SyncType": "NOT_A_SYNC_TYPE", "ResourceName": "AbsentStack" }),
            400,
            "ValidationException",
        ),
    ];

    for prefix in TARGET_PREFIXES {
        let router = router();
        for (case, body, expected_status, expected_code) in &rows {
            let target = format!("{prefix}.GetResourceSyncStatus");
            let (status, got) = call_target(&router, &target, body.clone()).await;
            assert_eq!(
                status, *expected_status,
                "case {case:?} under {prefix:?}: expected {expected_status}, got {status} {got}"
            );
            assert_eq!(
                error_code(&got),
                *expected_code,
                "case {case:?} under {prefix:?}: expected {expected_code}, got {got}"
            );
        }
    }
}

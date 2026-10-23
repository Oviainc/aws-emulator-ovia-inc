// SPDX-License-Identifier: CC0-1.0

//! The target census: EVERY operation this service binds is reachable under the target
//! prefix an AWS SDK client of it sends.
//!
//! The rule (pinned per-operation in `wire_target_routing.rs` and
//! `sync_blocker_summary.rs`): a request naming a modeled operation in `x-amz-target`
//! reaches that operation whatever the target prefix in front of the name — so the
//! answer is the operation's own (its response, its modeled error, or, for one still
//! scaffolded, the scaffold's own 501 naming that operation), never the framework's
//! "operation <routing key> is not implemented in this emulator", which means the target
//! named no operation at all.
//!
//! This suite is the census over the whole surface: one row per operation, driven under
//! the dotted prefix, so a later operation added to the service cannot quietly become
//! unroutable.

use super::connection_lifecycle::router;
use super::git_sync::error_code;
use super::wire_target_routing::{SHAPE_ID_PREFIX, call_target};

/// Every operation this service binds, from `smithy_query(query="list_operations")`
/// (27 operations, one handler file each under `src/ops/`).
const OPERATIONS: &[&str] = &[
    "CreateConnection",
    "CreateHost",
    "CreateRepositoryLink",
    "CreateSyncConfiguration",
    "DeleteConnection",
    "DeleteHost",
    "DeleteRepositoryLink",
    "DeleteSyncConfiguration",
    "GetConnection",
    "GetHost",
    "GetRepositoryLink",
    "GetRepositorySyncStatus",
    "GetResourceSyncStatus",
    "GetSyncBlockerSummary",
    "GetSyncConfiguration",
    "ListConnections",
    "ListHosts",
    "ListRepositoryLinks",
    "ListRepositorySyncDefinitions",
    "ListSyncConfigurations",
    "ListTagsForResource",
    "TagResource",
    "UntagResource",
    "UpdateHost",
    "UpdateRepositoryLink",
    "UpdateSyncBlocker",
    "UpdateSyncConfiguration",
];

/// The routing key the framework resolves for a dotted target: everything after the
/// FIRST `.` of the header. It appears in the framework's unrouted 501 message and
/// nowhere else, so it is the marker of a request that reached no operation.
fn unrouted_key(operation: &str) -> String {
    let (_, rest) = SHAPE_ID_PREFIX
        .split_once('.')
        .expect("the dotted prefix carries a dot");
    format!("{rest}.{operation}")
}

/// No operation answers the framework's unrouted 501, so every one of them is reached
/// under the prefix an SDK client sends. The body is empty, so an implemented operation
/// answers its modeled refusal and one still scaffolded answers the scaffold channel's
/// own 501 — a 501 is accepted here ONLY when it is that channel, which names the
/// operation by its short name and not the routing key.
#[tokio::test]
async fn every_modeled_operation_is_reachable_under_the_sdk_target_prefix() {
    let router = router();

    for operation in OPERATIONS {
        let target = format!("{SHAPE_ID_PREFIX}.{operation}");
        let (status, body) = call_target(&router, &target, serde_json::json!({})).await;
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert!(
            !message.contains(&unrouted_key(operation)),
            "case {operation:?}: the target must reach the operation, got {status} {body}"
        );
        if error_code(&body) == "NotImplemented" {
            // The only 501 a routed request may get is the scaffold channel's, for an
            // operation nobody has implemented yet; it names the operation itself.
            assert!(
                message.contains(&format!("operation {operation} is scaffolded")),
                "case {operation:?}: a 501 here may only be the scaffold channel, got {body}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! How long a `CreateNode` `RequestId` (`@idempotencyToken`) keeps replaying the
//! node it minted — end to end over the restJson1 wire.
//!
//! RULE: a create's idempotency token answers a retry with the resource it
//! minted only while that resource is still one the caller may use; a node's
//! delete is a DEREGISTRATION whose terminal `DEREGISTERED` state keeps the
//! record readable while refusing every mutation of it, so once the clock has
//! settled the node into that state the token's window is over and the retried
//! create MINTS a node of its own, out of its own request.
//!
//! This suite goes past the one node row of
//! `idempotency_token_after_terminal_delete::a_token_stops_replaying_a_resource_whose_delete_has_settled`
//! (which pins the rule across the kinds of resource this service tombstones):
//! what these rows add is the node side of it — that the node the retry minted
//! is one the caller may really USE while the tombstone it replaced is not, that
//! both of them are reported by the cluster's own listing, that the token then
//! replays the NEW node, that one token survives several
//! deregister-then-retry rounds, and that the window closing for one cluster
//! leaves another cluster's identical token alone.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.
//!
//! The suite is a TABLE: the reported sequence — one token, one node, one
//! deregistration, one retry — is ONE row of it.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{already_deleted_message, node_state};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::node_lifecycle::{
    cluster, create_node, created, delete_node, described, node_id, node_uri, ROLES,
};
use crate::node_listing::{list, summaries};
use crate::wire::{call, error_code, error_message, medialive};

/// The `RequestId` every row spends, and the name the first node of a row
/// carries.
const TOKEN: &str = "token-a";
const FIRST_NAME: &str = "node-a";

/// `UpdateNodeState` — the mutation that tells a node a caller may still change
/// from a tombstone it may not — asking for `state`
/// (`smithy_query(query="operation", operation="UpdateNodeState")`: `PUT
/// /prod/clusters/{ClusterId}/nodes/{NodeId}/state`).
async fn update_node_state(
    router: &MultiServiceRouter,
    cluster_id: &str,
    node: &str,
    state: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("{}/state", node_uri(cluster_id, node)),
        Some(json!({ "state": state })),
    )
    .await
}

/// Every node id one cluster reports through `ListNodes`, sorted — what the
/// cluster HOLDS, as a client reads it.
async fn held_ids(router: &MultiServiceRouter, case: &str, cluster_id: &str) -> Vec<String> {
    let (status, body) = list(router, cluster_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListNodes({cluster_id}): expected 200, got {status}: {body}"
    );
    let mut ids: Vec<String> = summaries(case, &body)
        .iter()
        .map(|summary| node_id(case, summary))
        .collect();
    ids.sort();
    ids
}

/// One row: how it names itself, the name and the role the retried create asks
/// for, how many deregister-then-retry rounds the one token runs, and whether
/// another cluster of the account spent the same token on a node of its own.
type Row = (&'static str, &'static str, Option<&'static str>, usize, bool);

/// Rule: a create's `RequestId` replays the node it minted only while that node
/// is still one a caller may use, so once the clock has settled that node's
/// deregistration into the terminal `DEREGISTERED` state the retried create
/// mints a node of its own — carrying the retry's own `Name` and `Role`, in the
/// `CREATED` state a create starts in, usable through `UpdateNodeState` which the
/// deregistered node refuses, and held by the cluster beside the tombstone the
/// delete kept.
///
/// The rows differ along what the rule generalises over: the `Name` the retry
/// asks for (the deregistered node's own name, free again, and a new one), the
/// `Role` it asks for (both values of the enum and the row that names none), how
/// many deregister-then-retry rounds the one token runs, and whether another
/// cluster of the account spent the very same token — a token is idempotent for
/// the cluster endpoint it was presented to, so one cluster's window closing says
/// nothing about another's.
#[tokio::test]
async fn a_node_token_stops_replaying_once_the_deregistration_has_settled() {
    // (case, the retry's name, the retry's role, rounds, a twin token elsewhere)
    let rows: [Row; 5] = [
        (
            "the retry reuses the deregistered node's own name",
            FIRST_NAME,
            Some(ROLES[0]),
            1,
            false,
        ),
        (
            "the retry asks for a new name and the other role",
            "node-b",
            Some(ROLES[1]),
            1,
            false,
        ),
        ("the retry names no role at all", "node-b", None, 1, false),
        (
            "one token over two deregister-then-retry rounds",
            "node-b",
            Some(ROLES[0]),
            2,
            false,
        ),
        (
            "another cluster spent the same token on a live node",
            "node-b",
            Some(ROLES[1]),
            1,
            true,
        ),
    ];

    for (case, retried_name, retried_role, rounds, twin_elsewhere) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "token-cluster").await;
        let retry = || {
            let mut body = json!({ "name": retried_name, "requestId": TOKEN });
            if let Some(role) = retried_role {
                body["role"] = json!(role);
            }
            body
        };

        // The node another cluster of the account minted with the SAME token,
        // which nothing this row does may disturb.
        let elsewhere = if twin_elsewhere {
            let other_cluster = cluster(&router, "token-cluster-elsewhere").await;
            let node = created(
                &router,
                case,
                &other_cluster,
                json!({ "name": FIRST_NAME, "requestId": TOKEN }),
            )
            .await;
            Some((other_cluster, node))
        } else {
            None
        };

        let mut live = created(
            &router,
            case,
            &cluster_id,
            json!({ "name": FIRST_NAME, "requestId": TOKEN, "role": ROLES[0] }),
        )
        .await;
        let mut live_id = node_id(case, &live);
        let mut held = vec![live_id.clone()];

        for round in 1..=rounds {
            let case = format!("{case}, round {round}");

            // While the node is live the token IS a replay: the retry answers
            // with the stored node, whatever name and role it now asks for, and
            // the cluster gains nothing.
            let replayed = created(&router, &case, &cluster_id, retry()).await;
            assert_eq!(
                replayed, live,
                "case {case:?}: expected a live node's token to replay {live_id:?}, got {replayed}"
            );
            assert_eq!(
                held_ids(&router, &case, &cluster_id).await,
                held,
                "case {case:?}: expected the replay to have created nothing"
            );

            // The delete is accepted and the clock settles it into the terminal
            // state the record then keeps.
            let (status, deleting) = delete_node(&router, &cluster_id, &live_id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteNode({live_id}): expected 202, got {status}: {deleting}"
            );
            wait_for_the_clock();
            let tombstone = described(&router, &case, &cluster_id, &live_id).await;
            assert_eq!(
                tombstone["state"],
                json!(node_state::DEREGISTERED),
                "case {case:?}: expected the settled deregistration to report {}, got {:?}",
                node_state::DEREGISTERED,
                tombstone["state"]
            );

            // The window is over: the same create carrying the same token mints
            // a node of its own, out of its own request.
            let retried = created(&router, &case, &cluster_id, retry()).await;
            let retried_id = node_id(&case, &retried);
            assert!(
                !held.contains(&retried_id),
                "case {case:?}: expected a freshly minted node, got the already held \
                 {retried_id:?}"
            );
            assert_eq!(
                retried["state"],
                json!(node_state::CREATED),
                "case {case:?}: expected the retry to answer with a live node, got {retried}"
            );
            assert_eq!(
                retried["name"],
                json!(retried_name),
                "case {case:?}: expected the name the retry asked for, got {:?}",
                retried["name"]
            );
            assert_eq!(
                retried["role"],
                retried_role.map_or(Value::Null, |role| json!(role)),
                "case {case:?}: expected the role the retry asked for, got {:?}",
                retried["role"]
            );
            assert_eq!(
                retried["clusterId"],
                json!(cluster_id),
                "case {case:?}: expected the node in the cluster the retry was addressed to, got \
                 {:?}",
                retried["clusterId"]
            );

            // The retry really STORED it, and left the tombstone exactly as the
            // settled delete had: the cluster now holds both.
            assert_eq!(
                described(&router, &case, &cluster_id, &retried_id).await,
                retried,
                "case {case:?}: expected DescribeNode to report the node the retry minted"
            );
            assert_eq!(
                described(&router, &case, &cluster_id, &live_id).await,
                tombstone,
                "case {case:?}: expected the deregistered node to stay as it was"
            );
            held.push(retried_id.clone());
            held.sort();
            assert_eq!(
                held_ids(&router, &case, &cluster_id).await,
                held,
                "case {case:?}: expected the cluster to hold the tombstone and the new node"
            );

            // The node the retry answered with is one the caller may CHANGE,
            // which the tombstone the token used to replay is not.
            let (status, commissioned) =
                update_node_state(&router, &cluster_id, &retried_id, node_state::ACTIVE).await;
            assert_eq!(
                status, 201,
                "case {case:?}: UpdateNodeState({retried_id}): expected 201, got {status}: \
                 {commissioned}"
            );
            assert_eq!(
                commissioned["state"],
                json!(node_state::ACTIVE),
                "case {case:?}: expected the commissioned node to report {}, got {:?}",
                node_state::ACTIVE,
                commissioned["state"]
            );
            let (status, refused) =
                update_node_state(&router, &cluster_id, &live_id, node_state::ACTIVE).await;
            assert_eq!(
                status, 404,
                "case {case:?}: UpdateNodeState({live_id}): expected 404, got {status}: {refused}"
            );
            assert_eq!(
                error_code(status, &refused),
                "NotFoundException",
                "case {case:?}: expected the modelled NotFoundException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                already_deleted_message(&format!("Node {live_id}")),
                "case {case:?}: expected the refusal to name the node whose delete was taken, got \
                 {refused}"
            );

            // The token now replays the node it last minted, as the store holds
            // it — commissioned, not as the create left it — and creates nothing.
            live = described(&router, &case, &cluster_id, &retried_id).await;
            let again = created(&router, &case, &cluster_id, retry()).await;
            assert_eq!(
                again, live,
                "case {case:?}: expected the token to replay the live node {retried_id:?}, got \
                 {again}"
            );
            assert_eq!(
                held_ids(&router, &case, &cluster_id).await,
                held,
                "case {case:?}: expected the second replay to have created nothing"
            );
            live_id = retried_id;
        }

        // The other cluster's identical token is untouched: its own node is
        // still live, and the token still replays it.
        if let Some((other_cluster, node)) = elsewhere {
            let other_id = node_id(case, &node);
            assert_eq!(
                described(&router, case, &other_cluster, &other_id).await,
                node,
                "case {case:?}: expected the other cluster's node to be untouched"
            );
            let (status, replayed) = create_node(
                &router,
                &other_cluster,
                json!({ "name": FIRST_NAME, "requestId": TOKEN }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateNode({other_cluster}): expected 201, got {status}: \
                 {replayed}"
            );
            assert_eq!(
                replayed, node,
                "case {case:?}: expected the other cluster's token to still replay {other_id:?}, \
                 got {replayed}"
            );
            assert_eq!(
                held_ids(&router, case, &other_cluster).await,
                vec![other_id],
                "case {case:?}: expected the other cluster to hold only its own node"
            );
        }
    }
}

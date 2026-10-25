// SPDX-License-Identifier: CC0-1.0

//! The MediaLive Anywhere node lifecycle — `CreateNode`, `DescribeNode`,
//! `UpdateNodeState` and `DeleteNode` — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the stub issue's single `DeleteNode` call is ONE row.
//!
//! A node lives IN a cluster, so every suite starts by creating one through
//! `CreateCluster` and addresses the node operations under the id it minted:
//! that is how a client reaches a node, and a cluster the account does not hold
//! takes no node at all (asserted below).

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    node_state, MAX_NODES, MAX_NODE_INTERFACE_MAPPINGS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::percent_encoded;
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT};

/// Every value `NodeRole` accepts, from
/// `smithy_query(query="shape", shape="NodeRole")`.
pub(crate) const ROLES: [&str; 2] = ["ACTIVE", "BACKUP"];

/// Every value `NetworkInterfaceMode` accepts, from
/// `smithy_query(query="shape", shape="NetworkInterfaceMode")`.
pub(crate) const INTERFACE_MODES: [&str; 2] = ["NAT", "BRIDGE"];

/// Every value `UpdateNodeState`'s `State` accepts, from
/// `smithy_query(query="shape", shape="UpdateNodeStateShape")` — named through
/// the service's own constants, so the suite and the store read one source.
const REQUESTABLE_STATES: [&str; 2] = [node_state::ACTIVE, node_state::DRAINING];

/// The `NodeState` a created node starts in, from
/// `smithy_query(query="shape", shape="NodeState")`.
const CREATED_STATE: &str = node_state::CREATED;

/// One cluster created through `CreateCluster`, answering the `Id` it minted.
///
/// `ClusterType` is the member `CreateCluster` needs (see the cluster suite);
/// what matters here is only that the cluster EXISTS, because that is what a
/// node is created in.
pub(crate) async fn cluster(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/clusters",
        Some(json!({ "name": name, "clusterType": "ON_PREMISES" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: CreateCluster({name}): expected 201, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateCluster({name}) reported no id: {created}"))
        .to_string()
}

/// The node collection of one cluster, which `CreateNode` and `ListNodes` are
/// bound to (`smithy_query(query="operation", operation="CreateNode")`: `POST
/// /prod/clusters/{ClusterId}/nodes`).
pub(crate) fn nodes_uri(cluster: &str) -> String {
    format!("/prod/clusters/{cluster}/nodes")
}

/// One node of one cluster, which `DescribeNode`, `DeleteNode` and `UpdateNode`
/// are bound to (`GET`/`DELETE`/`PUT
/// /prod/clusters/{ClusterId}/nodes/{NodeId}`).
pub(crate) fn node_uri(cluster: &str, node: &str) -> String {
    format!("/prod/clusters/{cluster}/nodes/{node}")
}

async fn create_node(router: &MultiServiceRouter, cluster: &str, body: Value) -> (u16, Value) {
    call(router, "POST", &nodes_uri(cluster), Some(body)).await
}

/// A created node, with the create itself asserted to have succeeded.
///
/// `201 Created` is the status the model gives `CreateNode`
/// (`smithy_query(query="operation", operation="CreateNode")`: code 201), and
/// the one every other MediaLive create answers with.
pub(crate) async fn created(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    body: Value,
) -> Value {
    let (status, node) = create_node(router, cluster, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateNode: expected 201, got {status}: {node}"
    );
    node
}

async fn describe_node(router: &MultiServiceRouter, cluster: &str, node: &str) -> (u16, Value) {
    call(router, "GET", &node_uri(cluster, node), None).await
}

pub(crate) async fn delete_node(
    router: &MultiServiceRouter,
    cluster: &str,
    node: &str,
) -> (u16, Value) {
    call(router, "DELETE", &node_uri(cluster, node), None).await
}

/// The state sub-resource of one node, which `UpdateNodeState` is bound to
/// (`smithy_query(query="operation", operation="UpdateNodeState")`: `PUT
/// /prod/clusters/{ClusterId}/nodes/{NodeId}/state`).
fn node_state_uri(cluster: &str, node: &str) -> String {
    format!("/prod/clusters/{cluster}/nodes/{node}/state")
}

/// `UpdateNodeState` asking for `state` \u2014 or, when it is `None`, a request that
/// names no `State` member at all, which the model allows.
async fn update_node_state(
    router: &MultiServiceRouter,
    cluster: &str,
    node: &str,
    state: Option<&str>,
) -> (u16, Value) {
    let body = match state {
        Some(state) => json!({ "state": state }),
        None => json!({}),
    };
    call(router, "PUT", &node_state_uri(cluster, node), Some(body)).await
}

/// One node, put into `state` through `UpdateNodeState` with the call asserted
/// to have succeeded.
///
/// `201 Created` is the status the model gives this operation
/// (`smithy_query(query="operation", operation="UpdateNodeState")`: code 201).
async fn state_updated(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    node: &str,
    state: Option<&str>,
) -> Value {
    let (status, body) = update_node_state(router, cluster, node, state).await;
    assert_eq!(
        status, 201,
        "case {case:?}: UpdateNodeState({state:?}): expected 201, got {status}: {body}"
    );
    body
}

/// One node, read back through `DescribeNode` with the read asserted to have
/// succeeded: a node whose delete was accepted stays readable.
pub(crate) async fn described(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    node: &str,
) -> Value {
    let (status, body) = describe_node(router, cluster, node).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeNode({cluster}/{node}): expected 200, got {status}: {body}"
    );
    body
}

/// The `id` member of a node-shaped response.
pub(crate) fn node_id(case: &str, node: &Value) -> String {
    node["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a node id, got {node}"))
        .to_string()
}

/// A `nodeInterfaceMappings` entry pairing `logical` with `physical` in `mode`.
pub(crate) fn mapping(logical: &str, physical: &str, mode: &str) -> Value {
    json!({
        "logicalInterfaceName": logical,
        "physicalInterfaceName": physical,
        "networkInterfaceMode": mode,
    })
}

/// Rule: `CreateNode` mints a numeric `Id`, builds the `Arn` from the request's
/// region and that id, records the cluster the request was addressed to together
/// with the `Name`, `Role` and `NodeInterfaceMappings` it carried, and reports
/// the node as `CREATED`, `DISCONNECTED`, in no channel placement group and with
/// no SDI source mapping — and `DescribeNode` reads that same stored node back.
///
/// The rows span the `NodeRole` enum and the row that names none ([`ROLES`]),
/// the `NetworkInterfaceMode` enum and the mapping counts 0, 1 and 2
/// ([`INTERFACE_MODES`]), the row that names no name, and two different clusters
/// of the same account, because that is what the rule generalises over.
#[tokio::test]
async fn a_created_node_is_read_back_by_describe() {
    // (case, which of the two clusters the create is addressed to, request body)
    let mut rows: Vec<(String, usize, Value)> = Vec::new();
    for cluster_index in 0..2usize {
        for role in [Some(ROLES[0]), Some(ROLES[1]), None] {
            for mappings in [
                Value::Null,
                json!([mapping("my-inputs", "eth1", INTERFACE_MODES[0])]),
                json!([
                    mapping("my-inputs", "eth1", INTERFACE_MODES[0]),
                    mapping("my-outputs", "ENO1234EXAMPLE", INTERFACE_MODES[1]),
                ]),
            ] {
                let count = mappings.as_array().map_or(0, Vec::len);
                let mut body = json!({ "name": format!("node-{cluster_index}-{count}") });
                if let Some(role) = role {
                    body["role"] = json!(role);
                }
                if !mappings.is_null() {
                    body["nodeInterfaceMappings"] = mappings.clone();
                }
                rows.push((
                    format!("cluster {cluster_index} role {role:?} with {count} mapping(s)"),
                    cluster_index,
                    body,
                ));
            }
        }
        // A create that names nothing at all but the cluster it is addressed to.
        rows.push((
            format!("cluster {cluster_index} with no member at all"),
            cluster_index,
            json!({}),
        ));
    }

    for (case, cluster_index, request) in rows {
        let router = medialive();
        // Two clusters, so the cluster a node reports is the one its create was
        // addressed to rather than the only one that exists.
        let clusters = [
            cluster(&router, "first-cluster").await,
            cluster(&router, "second-cluster").await,
        ];
        let cluster_id = clusters[cluster_index].clone();
        let node = created(&router, &case, &cluster_id, request.clone()).await;

        let id = node_id(&case, &node);
        assert!(
            id.chars().all(|character| character.is_ascii_digit()) && !id.is_empty(),
            "case {case:?}: expected a numeric node id, got {:?}",
            node["id"]
        );
        assert_eq!(
            node["arn"],
            json!(format!("arn:aws:medialive:us-west-2:{ACCOUNT}:node:{id}")),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            node["arn"]
        );
        assert_eq!(
            node["clusterId"],
            json!(cluster_id),
            "case {case:?}: expected the cluster the create was addressed to, got {:?}",
            node["clusterId"]
        );
        assert_eq!(
            node["name"],
            request.get("name").cloned().unwrap_or(Value::Null),
            "case {case:?}: expected the requested name, got {:?}",
            node["name"]
        );
        assert_eq!(
            node["role"],
            request.get("role").cloned().unwrap_or(Value::Null),
            "case {case:?}: expected the requested role, got {:?}",
            node["role"]
        );
        // The three members the create request shape carries
        // (`NodeInterfaceMappingCreateRequest`) come back unchanged, and no
        // `PhysicalInterfaceIpAddresses`: that member reports addresses the real
        // service learns from the node's hardware, which this emulator does not
        // run.
        assert_eq!(
            node["nodeInterfaceMappings"],
            request
                .get("nodeInterfaceMappings")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: expected the requested interface mappings, got {:?}",
            node["nodeInterfaceMappings"]
        );
        // `NodeState` starts at `CREATED` (`smithy_query(query="shape",
        // shape="NodeState")`): what moves a node on from there is the
        // registration script run on its own hardware.
        assert_eq!(
            node["state"],
            json!("CREATED"),
            "case {case:?}: expected a created node to be CREATED, got {:?}",
            node["state"]
        );
        // `NodeConnectionState`: nothing has connected, because this emulator
        // runs no node hardware.
        assert_eq!(
            node["connectionState"],
            json!("DISCONNECTED"),
            "case {case:?}: expected a created node to be DISCONNECTED, got {:?}",
            node["connectionState"]
        );
        // "Empty if the Node is not yet associated with any groups."
        assert_eq!(
            node["channelPlacementGroups"],
            json!([]),
            "case {case:?}: expected a new node to be in no placement group, got {:?}",
            node["channelPlacementGroups"]
        );
        assert_eq!(
            node["sdiSourceMappings"],
            json!([]),
            "case {case:?}: expected a new node to carry no SDI source mapping, got {:?}",
            node["sdiSourceMappings"]
        );
        // The EC2 instance hosting the node is infrastructure this emulator does
        // not provision, so it is not claimed.
        assert_eq!(
            node["instanceArn"],
            Value::Null,
            "case {case:?}: expected no instance ARN to be claimed, got {:?}",
            node["instanceArn"]
        );

        // The round trip: the reader answers with the stored node, member for
        // member, not with anything derived from its own request.
        let read_back = described(&router, &case, &cluster_id, &id).await;
        assert_eq!(
            read_back, node,
            "case {case:?}: expected DescribeNode to report the created node, got {read_back}"
        );

        // And reading does not change what the next reader sees.
        assert_eq!(
            described(&router, &case, &cluster_id, &id).await,
            node,
            "case {case:?}: expected a second DescribeNode to report the same node"
        );
    }
}

/// Rule: a node exists only in a cluster the account holds and its `Id` is
/// unique in THAT cluster, so `CreateNode` addressed to a cluster id nothing
/// holds is refused with `BadRequestException` (400, the client error this
/// create models — it declares no `NotFoundException`) naming that cluster, and
/// a node id addressed under a cluster it does not belong to names no node —
/// refused with `NotFoundException` (404), exactly as an id no node ever had is
/// — leaving the node itself alone.
///
/// The rows differ along which cluster the second request names (another cluster
/// of the account, one the account does not hold at all) and along which
/// operation does the asking, because the rule is about how a node is addressed.
#[tokio::test]
async fn a_node_is_only_found_under_its_own_cluster() {
    // (case, whether the "other" cluster exists in the account)
    let rows: [(&str, bool); 2] = [
        ("another cluster of the same account", true),
        ("a cluster id the account does not hold", false),
    ];

    for (case, other_exists) in rows {
        let router = medialive();
        let home = cluster(&router, "home-cluster").await;
        let other = if other_exists {
            cluster(&router, "other-cluster").await
        } else {
            // A well-formed cluster id no `CreateCluster` minted.
            "7777777".to_string()
        };

        let node = created(&router, case, &home, json!({ "name": "homed" })).await;
        let id = node_id(case, &node);

        // Reading it under the other cluster finds nothing...
        let (status, body) = describe_node(&router, &other, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DescribeNode under {other:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Node {id} not found"),
            "case {case:?}: expected the refusal to name the node id it was given, got {body}"
        );

        // ... and neither does deleting it there.
        let (status, body) = delete_node(&router, &other, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DeleteNode under {other:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );

        // An id no node ever had gets the same answer in the node's own cluster.
        let unknown = "9999999";
        let (status, body) = describe_node(&router, &home, unknown).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DescribeNode({unknown}): expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        let (status, body) = delete_node(&router, &home, unknown).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DeleteNode({unknown}): expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );

        // A node cannot be created in a cluster the account does not hold.
        if !other_exists {
            let (status, body) = create_node(&router, &other, json!({ "name": "homeless" })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: CreateNode in {other:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("Cluster {other} not found"),
                "case {case:?}: expected the refusal to name the cluster it was given, got {body}"
            );
        }

        // None of those refusals touched the node.
        assert_eq!(
            described(&router, case, &home, &id).await,
            node,
            "case {case:?}: expected the refused calls to leave the node exactly as it was"
        );
    }
}

/// Rule: `DeleteNode` DEREGISTERS the node it names — it answers `202 Accepted`
/// with that node in `DEREGISTERING` (the state the `NodeState` enum gives a
/// deletion, which models no `DELETED`), the clock settles it into the terminal
/// `DEREGISTERED` which `DescribeNode` then reports, and the record stays in the
/// account in that state; a repeated delete is accepted, answers the state the
/// node is already in and restarts nothing; no other node is touched.
///
/// The rows differ along what the node carries (its name, its role, its
/// interface mappings) and along how many nodes the cluster holds, because the
/// rule is about the node the request names.
#[tokio::test]
async fn a_deleted_node_deregisters_and_settles_into_deregistered() {
    // (case, the body of the node that gets deleted, how many bystanders)
    let rows: [(&str, Value, usize); 3] = [
        ("a bare node", json!({}), 0),
        (
            "a named ACTIVE node with one mapping",
            json!({
                "name": "doomed",
                "role": ROLES[0],
                "nodeInterfaceMappings": [mapping("my-inputs", "eth1", INTERFACE_MODES[0])],
            }),
            1,
        ),
        (
            "a BACKUP node beside two others",
            json!({ "name": "spare", "role": ROLES[1] }),
            2,
        ),
    ];

    for (case, body, bystanders) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "deleting-cluster").await;
        let node = created(&router, case, &cluster_id, body).await;
        let id = node_id(case, &node);

        let mut others = Vec::new();
        for index in 0..bystanders {
            let other = created(
                &router,
                case,
                &cluster_id,
                json!({ "name": format!("bystander-{index}") }),
            )
            .await;
            others.push(other);
        }

        // The delete is ACCEPTED: 202, with the node reported in the transient
        // `DEREGISTERING` state and every other member as the delete found it.
        let (status, deleting) = delete_node(&router, &cluster_id, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteNode: expected 202, got {status}: {deleting}"
        );
        assert_eq!(
            deleting["state"],
            json!("DEREGISTERING"),
            "case {case:?}: expected the delete to report DEREGISTERING, got {:?}",
            deleting["state"]
        );
        for member in [
            "id",
            "arn",
            "clusterId",
            "name",
            "role",
            "nodeInterfaceMappings",
            "sdiSourceMappings",
            "channelPlacementGroups",
            "connectionState",
        ] {
            assert_eq!(
                deleting[member], node[member],
                "case {case:?}: expected the delete to report the node it deleted at {member:?}, got {:?}",
                deleting[member]
            );
        }

        // The write landed in the store, not just in the response.
        assert_eq!(
            described(&router, case, &cluster_id, &id).await,
            deleting,
            "case {case:?}: expected DescribeNode to report the deregistering node"
        );

        // A repeat inside the transient window is accepted and changes nothing.
        let (status, again) = delete_node(&router, &cluster_id, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a repeated DeleteNode: expected 202, got {status}: {again}"
        );
        assert_eq!(
            again, deleting,
            "case {case:?}: expected the repeat to answer with the same deregistering node, got {again}"
        );

        // The clock settles the deregistration into the terminal state, which
        // the node then keeps instead of vanishing.
        wait_for_the_clock();
        let settled = described(&router, case, &cluster_id, &id).await;
        assert_eq!(
            settled["state"],
            json!("DEREGISTERED"),
            "case {case:?}: expected the clock to settle the node into DEREGISTERED, got {:?}",
            settled["state"]
        );
        for (member, value) in deleting.as_object().expect("the delete body is an object") {
            if member == "state" {
                continue;
            }
            assert_eq!(
                &settled[member], value,
                "case {case:?}: expected {member:?} to be unchanged by the clock, got {:?}",
                settled[member]
            );
        }

        // A delete repeated on a node already DEREGISTERED answers that terminal
        // state and does NOT put it back into the transient one: the acceptance
        // keeps the instant the first call set.
        let (status, terminal) = delete_node(&router, &cluster_id, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteNode on a DEREGISTERED node: expected 202, got {status}: {terminal}"
        );
        assert_eq!(
            terminal, settled,
            "case {case:?}: expected the repeat to answer with the same DEREGISTERED node, got {terminal}"
        );
        assert_eq!(
            described(&router, case, &cluster_id, &id).await,
            settled,
            "case {case:?}: expected the repeated delete to leave the terminal node alone"
        );

        // Every other node of the cluster is exactly as it was.
        for other in others {
            let other_id = node_id(case, &other);
            assert_eq!(
                described(&router, case, &cluster_id, &other_id).await,
                other,
                "case {case:?}: expected the bystander node {other_id} to be untouched"
            );
        }
    }
}

/// One row of the idempotency table: the case name, which cluster each of the
/// two creates is addressed to and the `RequestId` it carries, and whether they
/// must answer with the same node.
type RetryRow<'a> = (&'a str, (usize, Option<&'a str>), (usize, Option<&'a str>), bool);

/// Rule: a create retried with the `RequestId` (`@idempotencyToken`) of an
/// earlier one against the SAME cluster answers with the node that one created;
/// any other pair of creates mints two nodes.
///
/// The rows differ along the tokens and along the clusters the two creates are
/// addressed to, because a node's id is unique in its cluster and a token is
/// idempotent for the endpoint it was presented to.
#[tokio::test]
async fn a_retried_create_returns_the_node_the_first_one_created() {
    // (case, first (cluster, token), second (cluster, token), the same node?)
    let rows: [RetryRow<'_>; 5] = [
        (
            "the same token, the same cluster",
            (0, Some("token-a")),
            (0, Some("token-a")),
            true,
        ),
        (
            "two tokens, the same cluster",
            (0, Some("token-a")),
            (0, Some("token-b")),
            false,
        ),
        ("no token at all", (0, None), (0, None), false),
        (
            "only the second names a token",
            (0, None),
            (0, Some("token-b")),
            false,
        ),
        (
            "the same token, two clusters",
            (0, Some("token-a")),
            (1, Some("token-a")),
            false,
        ),
    ];

    for (case, (first_cluster, first_token), (second_cluster, second_token), same) in rows {
        let router = medialive();
        let clusters = [
            cluster(&router, "retry-cluster-a").await,
            cluster(&router, "retry-cluster-b").await,
        ];
        let mut first = json!({ "name": "first", "role": ROLES[0] });
        if let Some(token) = first_token {
            first["requestId"] = json!(token);
        }
        let mut second = json!({ "name": "second", "role": ROLES[1] });
        if let Some(token) = second_token {
            second["requestId"] = json!(token);
        }

        let one = created(&router, case, &clusters[first_cluster], first).await;
        let two = created(&router, case, &clusters[second_cluster], second).await;

        if same {
            assert_eq!(
                two, one,
                "case {case:?}: expected the retry to answer with the first node, got {two}"
            );
        } else {
            assert_ne!(
                two["id"], one["id"],
                "case {case:?}: expected two distinct nodes, got {:?} twice",
                one["id"]
            );
            // The second create's own values are what the second node carries.
            assert_eq!(
                two["name"],
                json!("second"),
                "case {case:?}: expected the second node to carry its own name, got {:?}",
                two["name"]
            );
            assert_eq!(
                two["clusterId"],
                json!(clusters[second_cluster]),
                "case {case:?}: expected the second node in its own cluster, got {:?}",
                two["clusterId"]
            );
        }
    }
}

/// Rule: the `NodeInterfaceMappings` list a create carries is bounded — at most
/// [`MAX_NODE_INTERFACE_MAPPINGS`] entries, the ceiling this emulator stores for
/// one node — so a longer list is refused with `BadRequestException` (400) and
/// no node is created.
///
/// The rows sit below, exactly ON and one PAST the ceiling, which is what a bound
/// generalises over.
#[tokio::test]
async fn a_create_past_the_interface_mapping_ceiling_is_refused() {
    // (case, how many mappings the request carries, the status it gets)
    let rows: [(&str, usize, u16); 4] = [
        ("no mapping", 0, 201),
        ("one mapping", 1, 201),
        ("at the ceiling", MAX_NODE_INTERFACE_MAPPINGS, 201),
        ("one past the ceiling", MAX_NODE_INTERFACE_MAPPINGS + 1, 400),
    ];

    for (case, count, expected) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "mapping-cluster").await;
        let mappings: Vec<Value> = (0..count)
            .map(|index| {
                mapping(
                    &format!("logical-{index}"),
                    &format!("eth{index}"),
                    INTERFACE_MODES[index % INTERFACE_MODES.len()],
                )
            })
            .collect();

        let (status, body) = create_node(
            &router,
            &cluster_id,
            json!({ "name": "mapped", "nodeInterfaceMappings": mappings.clone() }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: CreateNode with {count} mapping(s): expected {expected}, got {status}: {body}"
        );

        if expected == 201 {
            let id = node_id(case, &body);
            assert_eq!(
                body["nodeInterfaceMappings"],
                json!(mappings),
                "case {case:?}: expected every requested mapping to be stored, got {:?}",
                body["nodeInterfaceMappings"]
            );
            assert_eq!(
                described(&router, case, &cluster_id, &id).await,
                body,
                "case {case:?}: expected DescribeNode to report the created node"
            );
        } else {
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            // The refused create stored nothing: the next node the cluster takes
            // carries no mapping of this request's.
            let next = created(&router, case, &cluster_id, json!({ "name": "after" })).await;
            assert_eq!(
                next["nodeInterfaceMappings"],
                json!([]),
                "case {case:?}: expected the next node to carry no mapping, got {:?}",
                next["nodeInterfaceMappings"]
            );
        }
    }
}

/// Rule: a node's tags live on the node (no node-shaped response models a
/// `Tags` member, so the tag operations are what report them), and once its
/// deregistration has been ACCEPTED the node is readable but no longer
/// writable: a tag write on its ARN answers `NotFoundException` (404) and
/// changes nothing, while `ListTagsForResource` goes on reporting the tags it
/// carried.
///
/// This is the node's row of the rule the `deleted_resource_mutation` suite
/// asserts for every other resource kind; it lives here because a node is
/// addressed through the cluster that holds it.
///
/// The rows differ along the phase the write is attempted in — inside the
/// transient `DEREGISTERING` window and after the clock has settled the terminal
/// `DEREGISTERED` — because the delete is accepted at the start of it.
#[tokio::test]
async fn a_tag_write_on_a_deregistered_node_is_not_found() {
    // (case, whether to let the clock settle the deregistration first)
    let rows: [(&str, bool); 2] = [
        ("inside the deregistering window", false),
        ("once deregistered", true),
    ];

    for (case, settle) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "tagged-cluster").await;
        let node = created(
            &router,
            case,
            &cluster_id,
            json!({ "name": "tagged", "tags": { "team": "video" } }),
        )
        .await;
        let id = node_id(case, &node);
        let arn = node["arn"].as_str().unwrap_or_default().to_string();
        let tags_uri = format!("/prod/tags/{}", percent_encoded(&arn));

        // The create's own tags are the node's tags.
        let (status, listed) = call(&router, "GET", &tags_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            listed["tags"],
            json!({ "team": "video" }),
            "case {case:?}: expected the create's tags on the node, got {listed}"
        );

        // A tag write a LIVE node accepts, so the refusal below is about the
        // deregistration and nothing else.
        let (status, body) = call(
            &router,
            "POST",
            &tags_uri,
            Some(json!({ "tags": { "phase": "before" } })),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: CreateTags on a live node: expected 204, got {status}: {body}"
        );

        let (status, body) = delete_node(&router, &cluster_id, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteNode: expected 202, got {status}: {body}"
        );
        if settle {
            wait_for_the_clock();
        }

        for (operation, method, request) in [
            (
                "CreateTags",
                "POST",
                Some(json!({ "tags": { "phase": "after" } })),
            ),
            ("DeleteTags", "DELETE", None),
        ] {
            let uri = if method == "DELETE" {
                format!("{tags_uri}?tagKeys=team")
            } else {
                tags_uri.clone()
            };
            let (status, refusal) = call(&router, method, &uri, request).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} on a deregistered node: expected 404, got {status}: {refusal}"
            );
            assert_eq!(
                error_code(status, &refusal),
                "NotFoundException",
                "case {case:?}: {operation}: expected NotFoundException, got {refusal}"
            );
            assert!(
                error_message(&refusal).contains(&arn),
                "case {case:?}: {operation}: expected the refusal to name the ARN, got {refusal}"
            );
        }

        // The tags are exactly what they were, and the node is still readable.
        let (status, listed) = call(&router, "GET", &tags_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource after the delete: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            listed["tags"],
            json!({ "team": "video", "phase": "before" }),
            "case {case:?}: expected the refused tag writes to change nothing, got {listed}"
        );
        let after = described(&router, case, &cluster_id, &id).await;
        assert_eq!(
            after["state"],
            json!(if settle { "DEREGISTERED" } else { "DEREGISTERING" }),
            "case {case:?}: expected the node to be readable in its own state, got {:?}",
            after["state"]
        );
    }
}

/// Rule: the node store is bounded at [`MAX_NODES`] nodes across every cluster
/// — the emulator's own ceiling, so a create loop cannot grow its heap without
/// bound — and the create past it answers `TooManyRequestsException` (429, the
/// throttle `CreateNode` models) without storing a node; a deregistered node
/// still occupies its slot, because its record stays in the account.
///
/// The rows differ along how the nodes are spread over the clusters, because the
/// ceiling is on the store rather than on one cluster.
#[tokio::test]
async fn a_create_past_the_node_ceiling_is_refused() {
    // (case, how many clusters the nodes are spread over)
    let rows: [(&str, usize); 2] = [("one cluster", 1), ("three clusters", 3)];

    for (case, cluster_count) in rows {
        let router = medialive();
        let mut clusters = Vec::new();
        for index in 0..cluster_count {
            clusters.push(cluster(&router, &format!("full-cluster-{index}")).await);
        }

        // Fill the store to exactly the ceiling.
        let mut last = Value::Null;
        for index in 0..MAX_NODES {
            last = created(
                &router,
                case,
                &clusters[index % cluster_count],
                json!({ "name": format!("node-{index}") }),
            )
            .await;
        }

        // One more, in every cluster, is refused.
        for (index, cluster_id) in clusters.iter().enumerate() {
            let (status, body) =
                create_node(&router, cluster_id, json!({ "name": "one-too-many" })).await;
            assert_eq!(
                status, 429,
                "case {case:?}: CreateNode in cluster {index} past the ceiling: expected 429, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "TooManyRequestsException",
                "case {case:?}: expected TooManyRequestsException, got {body}"
            );
        }

        // A deregistered node keeps its slot: its record stays in the account,
        // so the store is still full.
        let id = node_id(case, &last);
        let home = clusters[(MAX_NODES - 1) % cluster_count].clone();
        let (status, body) = delete_node(&router, &home, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteNode: expected 202, got {status}: {body}"
        );
        wait_for_the_clock();
        let (status, body) = create_node(&router, &home, json!({ "name": "after-the-delete" })).await;
        assert_eq!(
            status, 429,
            "case {case:?}: expected a deregistered node to keep its slot, got {status}: {body}"
        );

        // And the refusals stored nothing: the node that was there is still the
        // one the reader reports.
        let after = described(&router, case, &home, &id).await;
        assert_eq!(
            after["state"],
            json!("DEREGISTERED"),
            "case {case:?}: expected the deregistered node to be readable, got {:?}",
            after["state"]
        );
    }
}

/// Rule: a node's `ChannelPlacementGroups` is every live channel placement group
/// of the node's own cluster whose `Nodes` names it ("An array of IDs. Each ID is
/// one ChannelPlacementGroup that is associated with this Node. Empty if the Node
/// is not yet associated with any groups"), derived on every read — so a group
/// created after the node shows up on it, a group that names another node does
/// not, and a group in another cluster does not.
///
/// The rows differ along which node and which cluster the group names, because
/// that is what the rule generalises over.
#[tokio::test]
async fn a_nodes_placement_groups_are_the_groups_that_name_it() {
    let router = medialive();
    let home = cluster(&router, "placement-cluster").await;
    let elsewhere = cluster(&router, "other-placement-cluster").await;

    let node = created(&router, "placement", &home, json!({ "name": "placed" })).await;
    let other_node = created(&router, "placement", &home, json!({ "name": "unplaced" })).await;
    let far_node = created(&router, "placement", &elsewhere, json!({ "name": "far" })).await;
    let id = node_id("placement", &node);
    let other_id = node_id("placement", &other_node);
    let far_id = node_id("placement", &far_node);

    // Before any group exists, the node is in none.
    assert_eq!(
        described(&router, "placement", &home, &id).await["channelPlacementGroups"],
        json!([]),
        "expected a node no group names to be in no placement group"
    );

    // One group of the node's cluster naming it, one naming its neighbour, and
    // one in the other cluster naming the far node.
    let mut group_ids = Vec::new();
    for (case, cluster_id, nodes) in [
        ("names the node", home.clone(), vec![id.clone()]),
        ("names the neighbour", home.clone(), vec![other_id.clone()]),
        ("another cluster", elsewhere.clone(), vec![far_id.clone()]),
    ] {
        let (status, group) = call(
            &router,
            "POST",
            &format!("/prod/clusters/{cluster_id}/channelplacementgroups"),
            Some(json!({ "name": format!("group-{case}").replace(' ', "-"), "nodes": nodes })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateChannelPlacementGroup: expected 201, got {status}: {group}"
        );
        group_ids.push(group["id"].as_str().unwrap_or_default().to_string());
    }

    // The node reports exactly the group that names it.
    assert_eq!(
        described(&router, "placement", &home, &id).await["channelPlacementGroups"],
        json!([group_ids[0].clone()]),
        "expected the node to report the group that names it"
    );
    assert_eq!(
        described(&router, "placement", &home, &other_id).await["channelPlacementGroups"],
        json!([group_ids[1].clone()]),
        "expected the neighbour to report its own group"
    );
    assert_eq!(
        described(&router, "placement", &elsewhere, &far_id).await["channelPlacementGroups"],
        json!([group_ids[2].clone()]),
        "expected the far node to report the group of its own cluster"
    );

    // A delete accepted for the group would drop it off the node again (a group
    // on its way out of the account associates nothing), which the derivation
    // filters on — `DeleteChannelPlacementGroup` is still a scaffold with its
    // own issue, so there is no wire call that reaches that state yet and this
    // suite does not claim one.

    // The node's own deregistration leaves the other nodes' lists alone.
    let (status, body) = delete_node(&router, &home, &id).await;
    assert_eq!(status, 202, "DeleteNode: expected 202, got {status}: {body}");
    assert_eq!(
        described(&router, "placement", &home, &other_id).await["channelPlacementGroups"],
        json!([group_ids[1].clone()]),
        "expected the neighbour's group list to be untouched"
    );
    // And a deregistering node still reports the group that names it: the
    // association is the group's, and the group still names the node.
    assert_eq!(
        described(&router, "placement", &home, &id).await["channelPlacementGroups"],
        json!([group_ids[0].clone()]),
        "expected the deregistering node to still report its group"
    );
}

/// The members of a node-shaped response that `UpdateNodeState` must leave
/// exactly as it found them: everything the node carries except its `State`.
const MEMBERS_BESIDE_STATE: [&str; 9] = [
    "id",
    "arn",
    "clusterId",
    "name",
    "role",
    "nodeInterfaceMappings",
    "sdiSourceMappings",
    "channelPlacementGroups",
    "connectionState",
];

/// Rule: `UpdateNodeState` puts the node its path names into the `NodeState` its
/// `State` member asks for — either of the two values the member accepts
/// ([`REQUESTABLE_STATES`]) — stores that state and reports the stored node, so
/// `DescribeNode` reports the same state from then on and the clock owes nothing
/// after the call; a request that names no `State` changes nothing; and no other
/// member of that node, and no other node, moves.
///
/// The rows differ along the state asked for, along how many states are asked
/// for in a row (so the node moves back and forth, and is asked for the state it
/// is already in), along the request that names no state at all, and along what
/// the node carries, because that is what the rule generalises over.
#[tokio::test]
async fn a_node_is_put_into_the_state_update_node_state_asks_for() {
    // (case, the body the node is created with, the states asked for in order)
    let rows: Vec<(&str, Value, Vec<Option<&str>>)> = vec![
        (
            "commission a bare node",
            json!({}),
            vec![Some(REQUESTABLE_STATES[0])],
        ),
        (
            "isolate a bare node",
            json!({}),
            vec![Some(REQUESTABLE_STATES[1])],
        ),
        (
            "commission, isolate, then commission again",
            json!({
                "name": "worker",
                "role": ROLES[0],
                "nodeInterfaceMappings": [mapping("my-inputs", "eth1", INTERFACE_MODES[0])],
            }),
            vec![
                Some(REQUESTABLE_STATES[0]),
                Some(REQUESTABLE_STATES[1]),
                Some(REQUESTABLE_STATES[0]),
            ],
        ),
        (
            "the state the node is already in, twice",
            json!({ "name": "idle", "role": ROLES[1] }),
            vec![Some(REQUESTABLE_STATES[1]), Some(REQUESTABLE_STATES[1])],
        ),
        ("no state member at all", json!({ "name": "unasked" }), vec![None]),
        (
            "a state, then a request that names none",
            json!({
                "name": "two-interfaces",
                "nodeInterfaceMappings": [
                    mapping("my-inputs", "eth1", INTERFACE_MODES[0]),
                    mapping("my-outputs", "ENO1234EXAMPLE", INTERFACE_MODES[1]),
                ],
            }),
            vec![Some(REQUESTABLE_STATES[0]), None],
        ),
    ];

    for (case, body, sequence) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "commissioning-cluster").await;
        let node = created(&router, case, &cluster_id, body).await;
        let id = node_id(case, &node);
        // A bystander, so what the update touches is the node it names.
        let bystander = created(&router, case, &cluster_id, json!({ "name": "bystander" })).await;
        let bystander_id = node_id(case, &bystander);

        let mut expected = CREATED_STATE.to_string();
        for requested in sequence {
            let updated = state_updated(&router, case, &cluster_id, &id, requested).await;
            // A request that names no state asks for no change.
            expected = requested.map_or(expected, str::to_string);

            assert_eq!(
                updated["state"],
                json!(expected),
                "case {case:?}: asked for {requested:?}: expected state {expected:?}, got {:?}",
                updated["state"]
            );
            for member in MEMBERS_BESIDE_STATE {
                assert_eq!(
                    updated[member], node[member],
                    "case {case:?}: asked for {requested:?}: expected {member:?} untouched, got {:?}",
                    updated[member]
                );
            }

            // The write landed in the STORE, not just in the response.
            assert_eq!(
                described(&router, case, &cluster_id, &id).await,
                updated,
                "case {case:?}: asked for {requested:?}: expected DescribeNode to report the stored node"
            );

            // And the state the caller asked for is the node's current one: the
            // clock owes no step after this call.
            wait_for_the_clock();
            assert_eq!(
                described(&router, case, &cluster_id, &id).await,
                updated,
                "case {case:?}: asked for {requested:?}: expected the clock to owe nothing"
            );
        }

        assert_eq!(
            described(&router, case, &cluster_id, &bystander_id).await,
            bystander,
            "case {case:?}: expected the bystander node {bystander_id} to be untouched"
        );
    }
}

/// Rule: `UpdateNodeState` changes a node that EXISTS and that a caller may
/// still change, so a cluster/node pair naming no node of the account — and a
/// node whose deregistration `DeleteNode` has already accepted, which names no
/// node a caller may still change — is answered with `NotFoundException` (404,
/// the error `DescribeNode` and `DeleteNode` model for the same node) and writes
/// nothing.
///
/// The rows differ along why the pair resolves to nothing — an id no node ever
/// had, a live node addressed under another cluster, a node inside its
/// deregistering window and one the clock has settled — and along the state
/// asked for, because the refusal is about the node and not about the state.
#[tokio::test]
async fn update_node_state_refuses_a_node_it_cannot_change() {
    // (case, whether the id names a live node of another cluster, how far the
    // node's own deregistration has got: none / accepted / settled)
    let rows: [(&str, bool, Option<bool>); 4] = [
        ("an id no node ever had", false, None),
        ("a node of another cluster", true, None),
        ("inside the deregistering window", false, Some(false)),
        ("once deregistered", false, Some(true)),
    ];

    for (case, under_another_cluster, deregistration) in rows {
        for requested in [Some(REQUESTABLE_STATES[0]), Some(REQUESTABLE_STATES[1]), None] {
            let router = medialive();
            let home = cluster(&router, "refusing-cluster").await;
            let elsewhere = cluster(&router, "other-refusing-cluster").await;
            let node = created(&router, case, &home, json!({ "name": "homed" })).await;
            let id = node_id(case, &node);

            // The pair the refused request names, and the message it must carry.
            let (cluster_id, node_id_asked, expected_message) = if under_another_cluster {
                (
                    elsewhere.clone(),
                    id.clone(),
                    format!("Node {id} not found"),
                )
            } else if deregistration.is_some() {
                (home.clone(), id.clone(), format!("Node {id} already deleted"))
            } else {
                // A well-formed node id no `CreateNode` minted.
                let unknown = "9999999".to_string();
                (
                    home.clone(),
                    unknown.clone(),
                    format!("Node {unknown} not found"),
                )
            };

            if let Some(settle) = deregistration {
                let (status, body) = delete_node(&router, &home, &id).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: DeleteNode: expected 202, got {status}: {body}"
                );
                if settle {
                    wait_for_the_clock();
                }
            }

            let before = described(&router, case, &home, &id).await;

            let (status, refusal) =
                update_node_state(&router, &cluster_id, &node_id_asked, requested).await;
            assert_eq!(
                status, 404,
                "case {case:?}: UpdateNodeState({requested:?}): expected 404, got {status}: {refusal}"
            );
            assert_eq!(
                error_code(status, &refusal),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {refusal}"
            );
            assert_eq!(
                error_message(&refusal),
                expected_message,
                "case {case:?}: expected the refusal to name the node it was given, got {refusal}"
            );

            // The refusal wrote nothing: the node is exactly as it was.
            assert_eq!(
                described(&router, case, &home, &id).await,
                before,
                "case {case:?}: expected the refused update to leave the node as it was"
            );
        }
    }
}

/// Rule: `State` targets an `enum` shape, so only the values that shape declares
/// ([`REQUESTABLE_STATES`]) reach the node: any other string — including a
/// `NodeState` value only the node's own registration could reach — is refused
/// as a client error and the node keeps the state it had.
///
/// The rows span a `NodeState` value the member does not accept, a lower-case
/// spelling of one it does, and the empty string, because that is the class the
/// enum's membership rules out.
#[tokio::test]
async fn a_state_outside_the_enum_is_refused_and_writes_nothing() {
    // Values `UpdateNodeStateShape` does not declare; the first three are
    // `NodeState` values a node can be in but no caller can ask for
    // (`smithy_query(query="shape", shape="NodeState")`).
    let strays = [
        "READY",
        "IN_USE",
        node_state::DEREGISTERED,
        "active",
        "",
        "NOT_A_MODELLED_VALUE",
    ];

    for stray in strays {
        let case = format!("state = {stray:?}");
        let router = medialive();
        let cluster_id = cluster(&router, "stray-state-cluster").await;
        let node = created(&router, &case, &cluster_id, json!({ "name": "strayed" })).await;
        let id = node_id(&case, &node);

        let (status, body) = update_node_state(&router, &cluster_id, &id, Some(stray)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected an unmodelled state to be refused, got {status}: {body}"
        );
        let code = error_code(status, &body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {case:?}: expected a client error, got {code} ({body})"
        );

        // The refused request wrote nothing.
        let after = described(&router, &case, &cluster_id, &id).await;
        assert_eq!(
            after, node,
            "case {case:?}: expected the refused update to leave the node as it was, got {after}"
        );

        // And a value the enum DOES declare still reaches the same node.
        let updated = state_updated(
            &router,
            &case,
            &cluster_id,
            &id,
            Some(REQUESTABLE_STATES[0]),
        )
        .await;
        assert_eq!(
            updated["state"],
            json!(REQUESTABLE_STATES[0]),
            "case {case:?}: expected the modelled value to be accepted, got {:?}",
            updated["state"]
        );
    }
}

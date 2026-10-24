// SPDX-License-Identifier: CC0-1.0

//! MediaLive channel placement groups, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The clusters the rows place groups in are created through `CreateCluster`, so
//! every `ClusterId` in this suite is an id the store really minted.
//!
//! The suites are tables: one cluster, one name and one node id is ONE row of
//! each. Every node id a row names is one `CreateNode` really minted in the
//! row's own cluster, because that is the only kind of id a group's `Nodes` may
//! carry.
//!
//! The group helpers here are `pub(crate)` because the UPDATE suite
//! ([`crate::channel_placement_group_update`]) drives the same resource through
//! the same two URIs: one harness for the resource, so the create, the read and
//! the update suites cannot disagree about how a group is reached.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_CHANNEL_PLACEMENT_GROUPS;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::node_lifecycle::node_uri;
use crate::support::percent_encoded;
use crate::wire::{call, error_code, medialive, ACCOUNT};

/// The cluster collection URI
/// (`smithy_query(query="operation", operation="CreateCluster")`:
/// `POST /prod/clusters`).
const CLUSTERS: &str = "/prod/clusters";

/// The collection URI `CreateChannelPlacementGroup` and
/// `ListChannelPlacementGroups` are bound to
/// (`smithy_query(query="operation",
/// operation="CreateChannelPlacementGroup,ListChannelPlacementGroups")`:
/// `POST` / `GET /prod/clusters/{ClusterId}/channelplacementgroups`).
pub(crate) fn groups_uri(cluster: &str) -> String {
    format!("{CLUSTERS}/{cluster}/channelplacementgroups")
}

/// The instance URI `DescribeChannelPlacementGroup` is bound to
/// (`GET /prod/clusters/{ClusterId}/channelplacementgroups/{ChannelPlacementGroupId}`).
pub(crate) fn group_uri(cluster: &str, id: &str) -> String {
    format!("{}/{id}", groups_uri(cluster))
}

/// One cluster to place groups in, created through `CreateCluster`; answers the
/// id it minted.
pub(crate) async fn create_cluster(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        CLUSTERS,
        Some(json!({ "name": name, "clusterType": "ON_PREMISES" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateCluster: expected 201, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

pub(crate) async fn create_group(
    router: &MultiServiceRouter,
    cluster: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "POST", &groups_uri(cluster), Some(body)).await
}

/// A created channel placement group, with the create itself asserted to have
/// succeeded. `201 Created` is the status the model declares for this create.
pub(crate) async fn created(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    body: Value,
) -> Value {
    let (status, group) = create_group(router, cluster, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannelPlacementGroup: expected 201, got {status}: {group}"
    );
    group
}

pub(crate) async fn describe_group(
    router: &MultiServiceRouter,
    cluster: &str,
    id: &str,
) -> (u16, Value) {
    call(router, "GET", &group_uri(cluster, id), None).await
}

/// The group a describe reports, with the read itself asserted to have
/// succeeded.
pub(crate) async fn described(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    id: &str,
) -> Value {
    let (status, group) = describe_group(router, cluster, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannelPlacementGroup: expected 200, got {status}: {group}"
    );
    group
}

/// One group description with its `State` dropped, so two reads can be compared
/// on every member the clock does not move.
pub(crate) fn without_state(group: &Value) -> Value {
    let mut copy = group.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("state");
    }
    copy
}

/// The `id` of a reported group.
pub(crate) fn id_of(group: &Value) -> String {
    group["id"].as_str().unwrap_or_default().to_string()
}

/// The `channels` a reported group carries, in the order it reported them.
fn channels_of(group: &Value) -> Vec<String> {
    group["channels"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|channel| channel.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Create a channel, optionally placed in a cluster's channel placement group,
/// and answer its id. `pub(crate)` for the same reason as the group helpers
/// above: the LISTING suite ([`crate::channel_placement_group_listing`]) and the
/// DELETION suite ([`crate::channel_placement_group_deletion`]) assert the same
/// derived `Channels` member from the same kind of channel.
pub(crate) async fn create_channel(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    placement: Option<(&str, &str)>,
) -> String {
    let mut request = json!({ "name": name });
    if let Some((cluster, group)) = placement {
        request["anywhereSettings"] =
            json!({ "clusterId": cluster, "channelPlacementGroupId": group });
    }
    let (status, response) = call(router, "POST", "/prod/channels", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {response}"
    );
    response["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `count` nodes created in `cluster` through `CreateNode`, answering the ids
/// the store minted, in the order they were created.
///
/// A group's `Nodes` is a REFERENCE to a node of the group's own cluster, so
/// every row that associates a group with a node names an id the store really
/// minted rather than a literal. The CreateNode call itself is the NODE suite's
/// ([`crate::node_lifecycle::created`]), so the node collection and the status
/// it answers are spelled once in this harness. `pub(crate)` for the same
/// reason as the group helpers above: the UPDATE, LISTING and DELETION suites
/// associate the same kind of node with the same kind of group.
pub(crate) async fn create_nodes(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let node = crate::node_lifecycle::created(
            router,
            case,
            cluster,
            json!({ "name": format!("node-{index}") }),
        )
        .await;
        ids.push(node["id"].as_str().unwrap_or_default().to_string());
    }
    ids
}

/// One node of one cluster, deleted through the NODE suite's `DeleteNode` call;
/// the delete is asserted to have been accepted (`202`, the status the model
/// declares for it).
pub(crate) async fn deregister_node(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    node: &str,
) {
    let (status, body) = crate::node_lifecycle::delete_node(router, cluster, node).await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteNode: expected 202, got {status}: {body}"
    );
}

/// How a row sends the `Nodes` member: not at all, or naming `n` nodes the
/// row's own cluster really has (`Named(0)` is the empty list).
#[derive(Clone, Copy, Debug)]
pub(crate) enum NodesSent {
    Absent,
    Named(usize),
}

/// Rule: a created channel placement group is stored under a minted `Id` whose
/// `Arn` is built from the signed region and that id, carries the `ClusterId`
/// the request's path named and the `Name` and `Nodes` the request supplied, is
/// `ASSIGNING` while the association with a node is in flight and `UNASSIGNED`
/// when it names no node — and `DescribeChannelPlacementGroup` reads that same
/// stored group back.
///
/// The rows differ along which members the create supplies (name, nodes, tags,
/// none at all) and along which of the account's clusters the group belongs to,
/// because that is what the rule generalises over; the states are the model's
/// own (`smithy_query(query="shape", shape="ChannelPlacementGroupState")`). The
/// node ids the rows name are minted by `CreateNode` in the row's own cluster,
/// since that is the only kind of id a group may be associated with.
#[tokio::test]
async fn a_created_group_is_read_back_by_describe() {
    // (case, which cluster, request besides `Nodes`, how `Nodes` is sent,
    //  expected name, state now, state after the clock)
    let rows: [(&str, usize, Value, NodesSent, Value, &str, &str); 6] = [
        (
            "nothing at all",
            0,
            json!({}),
            NodesSent::Absent,
            Value::Null,
            "UNASSIGNED",
            "UNASSIGNED",
        ),
        (
            "name only",
            0,
            json!({ "name": "studio-a" }),
            NodesSent::Absent,
            json!("studio-a"),
            "UNASSIGNED",
            "UNASSIGNED",
        ),
        (
            "name and one node",
            0,
            json!({ "name": "studio-b" }),
            NodesSent::Named(1),
            json!("studio-b"),
            "ASSIGNING",
            "ASSIGNED",
        ),
        (
            "node only, second cluster",
            1,
            json!({}),
            NodesSent::Named(1),
            Value::Null,
            "ASSIGNING",
            "ASSIGNED",
        ),
        (
            "tags too, second cluster",
            1,
            json!({ "name": "tagged", "tags": { "team": "news" } }),
            NodesSent::Absent,
            json!("tagged"),
            "UNASSIGNED",
            "UNASSIGNED",
        ),
        (
            "an empty node list",
            0,
            json!({ "name": "no-nodes" }),
            NodesSent::Named(0),
            json!("no-nodes"),
            "UNASSIGNED",
            "UNASSIGNED",
        ),
    ];

    for (case, which, request, sent, name, state, settled_state) in rows {
        let router = medialive();
        let clusters = [
            create_cluster(&router, case, "primary").await,
            create_cluster(&router, case, "secondary").await,
        ];
        let cluster = clusters[which].clone();

        let mut request = request;
        // A node named by the request is one the cluster really has, and an
        // absent member reports the empty list just as `Named(0)` does.
        let nodes = match sent {
            NodesSent::Absent => json!([]),
            NodesSent::Named(count) => {
                let minted = create_nodes(&router, case, &cluster, count).await;
                request["nodes"] = json!(minted);
                json!(minted)
            }
        };

        let group = created(&router, case, &cluster, request).await;
        let id = id_of(&group);

        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a numeric channel placement group id, got {:?}",
            group["id"]
        );
        assert_ne!(
            id, cluster,
            "case {case:?}: expected the group's id to come from its own id space, not the cluster's"
        );
        assert_eq!(
            group["arn"],
            json!(format!(
                "arn:aws:medialive:us-west-2:{ACCOUNT}:channelPlacementGroup:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            group["arn"]
        );
        assert_eq!(
            group["clusterId"],
            json!(cluster),
            "case {case:?}: expected the cluster the path named, got {:?}",
            group["clusterId"]
        );
        assert_eq!(
            group["name"], name,
            "case {case:?}: expected name {name}, got {:?}",
            group["name"]
        );
        assert_eq!(
            group["nodes"], nodes,
            "case {case:?}: expected nodes {nodes}, got {:?}",
            group["nodes"]
        );
        assert_eq!(
            group["channels"],
            json!([]),
            "case {case:?}: expected no channels placed in a new group, got {:?}",
            group["channels"]
        );
        assert_eq!(
            group["state"],
            json!(state),
            "case {case:?}: expected state {state}, got {:?}",
            group["state"]
        );

        // The round trip: the reader answers with the stored group, member for
        // member, not with anything derived from its own request. Only `State`
        // is left out of the comparison, because the clock may legitimately
        // have moved it between the two calls.
        let read = described(&router, case, &cluster, &id).await;
        assert_eq!(
            without_state(&read),
            without_state(&group),
            "case {case:?}: expected the describe to report the created group, got {read}"
        );

        // And once the clock has run, a group with a node has settled into
        // `ASSIGNED` while a group with none stays where it was: the clock owes
        // a transition only to a resource in a non-terminal state.
        wait_for_the_clock();
        let after = described(&router, case, &cluster, &id).await;
        assert_eq!(
            after["state"],
            json!(settled_state),
            "case {case:?}: expected the group to settle into {settled_state}, got {:?}",
            after["state"]
        );
        assert_eq!(
            without_state(&after),
            without_state(&group),
            "case {case:?}: expected only the state to have moved, got {after}"
        );
    }
}

/// Rule: a group is named by BOTH path labels — it belongs to the cluster it was
/// created in — so a `ChannelPlacementGroupId` that names no group of the
/// requested `ClusterId` is `NotFoundException` (HTTP 404), whichever of the two
/// is wrong.
///
/// The rows differ along which label misses, because that is what the rule
/// generalises over.
#[tokio::test]
async fn a_group_is_only_visible_through_its_own_cluster() {
    let router = medialive();
    let cluster_one = create_cluster(&router, "scoping", "primary").await;
    let cluster_two = create_cluster(&router, "scoping", "secondary").await;
    let one = created(
        &router,
        "scoping",
        &cluster_one,
        json!({ "name": "in-cluster-one" }),
    )
    .await;
    let two = created(
        &router,
        "scoping",
        &cluster_two,
        json!({ "name": "in-cluster-two" }),
    )
    .await;
    let first = id_of(&one);
    let second = id_of(&two);

    // (case, cluster asked for, id asked for)
    let misses: [(&str, &str, &str); 4] = [
        ("another cluster's group", &cluster_one, &second),
        (
            "the group's own id under the other cluster",
            &cluster_two,
            &first,
        ),
        ("an id no group has", &cluster_one, "9999999"),
        ("a cluster that does not exist", "7654321", &first),
    ];

    for (case, cluster, id) in misses {
        let (status, body) = describe_group(&router, cluster, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains(id),
            "case {case:?}: expected the refusal to name the id asked for, got {body}"
        );
    }

    // Each group is still readable through its own cluster, so none of those
    // refusals removed anything.
    for (case, cluster, id, expected) in [
        ("first cluster", &cluster_one, &first, &one),
        ("second cluster", &cluster_two, &second, &two),
    ] {
        let read = described(&router, case, cluster, id).await;
        assert_eq!(
            read, *expected,
            "case {case:?}: expected the group to be unchanged, got {read}"
        );
    }
}

/// Rule: a group is created INSIDE one cluster, so a `ClusterId` the account
/// holds no cluster for is refused — `UnprocessableEntityException` (HTTP 422),
/// the refusal this create models for a request it cannot process, naming the
/// member at fault — and nothing is stored under it.
///
/// The rows differ along the shape of the unknown id (a plausible MediaLive
/// numeric id, one from another resource's id space, and a non-numeric one),
/// because that is what the rule generalises over.
#[tokio::test]
async fn a_group_needs_a_cluster_that_exists() {
    let router = medialive();
    let cluster = create_cluster(&router, "unknown cluster", "primary").await;
    // A channel's id: it exists as a resource id, but it is not a cluster's.
    let channel = create_channel(&router, "unknown cluster", "unplaced", None).await;

    for (case, unknown) in [
        ("a numeric id no cluster has", "7654321".to_string()),
        ("a channel's id", channel),
        ("a non-numeric id", "not-a-cluster".to_string()),
    ] {
        let (status, body) = create_group(&router, &unknown, json!({ "name": "orphan" })).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {body}"
        );
        assert_eq!(
            body["validationErrors"][0]["elementPath"],
            json!("clusterId"),
            "case {case:?}: expected the validation error to name the member, got {body}"
        );
    }

    // The same request against the cluster that does exist is accepted, so the
    // refusals above were about the cluster and not about the group.
    let group = created(
        &router,
        "the cluster that exists",
        &cluster,
        json!({ "name": "orphan" }),
    )
    .await;
    assert_eq!(
        group["clusterId"],
        json!(cluster),
        "expected the group to belong to the cluster that exists, got {:?}",
        group["clusterId"]
    );
}

/// Rule: `Nodes` accepts at most one node id — "An array of one ID for the Node
/// that you want to associate with the ChannelPlacementGroup. (You can't
/// associate more than one Node with the ChannelPlacementGroup.)" — so a create
/// naming more is refused with `UnprocessableEntityException` (HTTP 422) naming
/// the member, and stores nothing.
///
/// The rows span the published bound (one node), below it (none) and past it
/// (two and three), because that is what the rule generalises over. Every id
/// named is a node the cluster really has, so the COUNT is the only thing each
/// row varies.
#[tokio::test]
async fn nodes_accepts_at_most_one_node_id() {
    // (case, how many of the cluster's nodes the create names, accepted)
    let rows: [(&str, usize, bool); 4] = [
        ("no node", 0, true),
        ("one node", 1, true),
        ("two nodes", 2, false),
        ("three nodes", 3, false),
    ];

    for (case, count, accepted) in rows {
        let router = medialive();
        let cluster = create_cluster(&router, case, "primary").await;
        // A group that already exists, so a refusal can be shown to have left
        // the store exactly as it was.
        let existing = created(&router, case, &cluster, json!({ "name": "existing" })).await;
        // The nodes this row names, all of them nodes the cluster really has, so
        // the COUNT is the only thing it varies. At least one is minted even for
        // the zero-node row, because the retry at the end of a refused row needs
        // a node it can legitimately name.
        let minted = create_nodes(&router, case, &cluster, count.max(1)).await;
        let nodes = json!(minted[..count]);

        let (status, body) = create_group(
            &router,
            &cluster,
            json!({ "name": "under-test", "nodes": nodes.clone() }),
        )
        .await;

        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected the create to be accepted, got {status}: {body}"
            );
            assert_eq!(
                body["nodes"], nodes,
                "case {case:?}: expected the nodes the request named, got {:?}",
                body["nodes"]
            );
            continue;
        }

        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {body}"
        );
        assert_eq!(
            body["validationErrors"][0]["elementPath"],
            json!("nodes"),
            "case {case:?}: expected the validation error to name the member, got {body}"
        );

        // Nothing was stored: the group that existed before is untouched, and
        // the refused name is still free.
        let read = described(&router, case, &cluster, &id_of(&existing)).await;
        assert_eq!(
            read, existing,
            "case {case:?}: expected the refused create to change nothing, got {read}"
        );
        let retried = created(
            &router,
            case,
            &cluster,
            json!({ "name": "under-test", "nodes": [minted[0].clone()] }),
        )
        .await;
        assert_eq!(
            retried["nodes"],
            json!([minted[0].clone()]),
            "case {case:?}: expected the name the refused create asked for to still be free, got {retried}"
        );
    }
}

/// Rule: `Nodes` is a REFERENCE — "An array of one ID for the Node that you want
/// to associate with the ChannelPlacementGroup", where "The Node and the
/// ChannelPlacementGroup must be in the same Cluster" — so a create naming an id
/// that is not a node of that cluster the account can still associate is refused
/// with `UnprocessableEntityException` (HTTP 422) naming the member, and stores
/// nothing.
///
/// The rows differ along the shape of the id that does not resolve (an id no
/// node has, a node of another cluster, ids from two other resource id spaces, a
/// non-numeric id, and a node whose deregistration has been accepted), because
/// that is what the rule generalises over. The accepted control at the end
/// reports the association from BOTH ends, which is what a dangling reference
/// would break.
#[tokio::test]
async fn nodes_must_name_a_node_of_the_groups_own_cluster() {
    let router = medialive();
    let setup = "node resolution";
    let cluster = create_cluster(&router, setup, "primary").await;
    let elsewhere = create_cluster(&router, setup, "secondary").await;
    // Two nodes of the group's own cluster — one to associate, one to
    // deregister — and one node of another cluster.
    let mut mine = create_nodes(&router, setup, &cluster, 2).await;
    let doomed = mine.remove(1);
    let node = mine.remove(0);
    let far = create_nodes(&router, setup, &elsewhere, 1)
        .await
        .remove(0);
    // A resource of another type, to show that an id of another of this
    // service's id spaces resolves no better than one nothing holds.
    let channel = create_channel(&router, setup, "unplaced", None).await;
    // A group that already exists, so each refusal can be shown to have left the
    // store exactly as it was.
    let existing = created(&router, setup, &cluster, json!({ "name": "existing" })).await;
    // A node on its way out of the account is nothing a new association can be
    // made with.
    deregister_node(&router, setup, &cluster, &doomed).await;

    let rows: [(&str, String); 6] = [
        ("a numeric id no node has", "9999999".to_string()),
        ("a node of another cluster", far.clone()),
        ("the cluster's own id", cluster.clone()),
        ("a channel's id", channel.clone()),
        ("a non-numeric id", "not-a-node".to_string()),
        (
            "a node whose deregistration was accepted",
            doomed.clone(),
        ),
    ];

    for (case, unresolvable) in rows {
        let (status, body) = create_group(
            &router,
            &cluster,
            json!({ "name": "under-test", "nodes": [unresolvable.clone()] }),
        )
        .await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422 for node {unresolvable:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {body}"
        );
        assert_eq!(
            body["validationErrors"][0]["elementPath"],
            json!("nodes"),
            "case {case:?}: expected the validation error to name the member, got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&unresolvable),
            "case {case:?}: expected the refusal to name the id it could not resolve, got {body}"
        );

        // Nothing was stored: the group that existed before is untouched, and
        // the cluster still holds exactly that one group.
        let read = described(&router, case, &cluster, &id_of(&existing)).await;
        assert_eq!(
            without_state(&read),
            without_state(&existing),
            "case {case:?}: expected the refused create to change nothing, got {read}"
        );
    }

    // The same request naming a node the cluster really has is accepted, so the
    // refusals were about the reference rather than about the rest of the
    // request — and the association is then reported from both ends: the group
    // names the node, and the node names the group.
    let case = "a node of this cluster";
    let group = created(
        &router,
        case,
        &cluster,
        json!({ "name": "under-test", "nodes": [node.clone()] }),
    )
    .await;
    assert_eq!(
        group["nodes"],
        json!([node.clone()]),
        "case {case:?}: expected the group to name the node it was created with, got {:?}",
        group["nodes"]
    );
    let (status, reported_node) = call(&router, "GET", &node_uri(&cluster, &node), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeNode: expected 200, got {status}: {reported_node}"
    );
    assert_eq!(
        reported_node["channelPlacementGroups"],
        json!([id_of(&group)]),
        "case {case:?}: expected the node to report the group that names it, got {:?}",
        reported_node["channelPlacementGroups"]
    );
}

/// Rule: a group's `Name` is unique in its CLUSTER and case-sensitive ("Specify
/// a name that is unique in the Cluster. You can't change the name. Names are
/// case-sensitive"), so a second create under the same name in the same cluster
/// is refused while the same name in another cluster, a different name, and a
/// name differing only in case are all accepted.
///
/// The rows differ along the cluster and the spelling of the second name,
/// because that is what the rule generalises over.
#[tokio::test]
async fn a_name_is_unique_within_its_cluster() {
    // (case, which cluster the second create names, second name, accepted)
    let rows: [(&str, usize, &str, bool); 4] = [
        ("same cluster, same name", 0, "studio-a", false),
        ("same cluster, another name", 0, "studio-b", true),
        ("same cluster, other case", 0, "Studio-A", true),
        ("another cluster, same name", 1, "studio-a", true),
    ];

    for (case, which, name, accepted) in rows {
        let router = medialive();
        let clusters = [
            create_cluster(&router, case, "primary").await,
            create_cluster(&router, case, "secondary").await,
        ];
        let first = created(&router, case, &clusters[0], json!({ "name": "studio-a" })).await;

        let (status, body) = create_group(&router, &clusters[which], json!({ "name": name })).await;
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected the create to be accepted, got {status}: {body}"
            );
            assert_ne!(
                id_of(&body),
                id_of(&first),
                "case {case:?}: expected a second group, got the first one again"
            );
            assert_eq!(
                body["name"],
                json!(name),
                "case {case:?}: expected the name the request asked for, got {:?}",
                body["name"]
            );
            continue;
        }

        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {body}"
        );
        assert_eq!(
            body["validationErrors"][0]["elementPath"],
            json!("name"),
            "case {case:?}: expected the validation error to name the member, got {body}"
        );

        // And the group that holds the name is unchanged.
        let read = described(&router, case, &clusters[0], &id_of(&first)).await;
        assert_eq!(
            read, first,
            "case {case:?}: expected the refused create to change nothing, got {read}"
        );
    }
}

/// One row of the idempotency table: the case, which cluster the second create
/// names, the `RequestId` each of the two creates presents, and whether the two
/// answer with one group.
type RetryRow = (
    &'static str,
    usize,
    Option<&'static str>,
    Option<&'static str>,
    bool,
);

/// Rule: a create retried with the `RequestId` (`@idempotencyToken`) of an
/// earlier one for the SAME cluster answers with the group that one created; two
/// creates that do not share a token — or that present the same token to
/// different clusters — create two groups.
#[tokio::test]
async fn a_retried_create_returns_the_group_the_first_one_created() {
    let rows: [RetryRow; 5] = [
        ("the same token", 0, Some("token-a"), Some("token-a"), true),
        ("two tokens", 0, Some("token-a"), Some("token-b"), false),
        ("no token at all", 0, None, None, false),
        ("only the second names one", 0, None, Some("token-b"), false),
        (
            "the same token, another cluster",
            1,
            Some("token-a"),
            Some("token-a"),
            false,
        ),
    ];

    for (case, which, first_token, second_token, same) in rows {
        let router = medialive();
        let clusters = [
            create_cluster(&router, case, "primary").await,
            create_cluster(&router, case, "secondary").await,
        ];
        let second_cluster = clusters[which].clone();
        // The node the FIRST create associates its group with: a node of that
        // group's own cluster, which is the only kind a group may name.
        let node = create_nodes(&router, case, &clusters[0], 1).await.remove(0);

        let mut first = json!({ "name": "first", "nodes": [node.clone()] });
        if let Some(token) = first_token {
            first["requestId"] = json!(token);
        }
        let mut second = json!({ "name": "second" });
        if let Some(token) = second_token {
            second["requestId"] = json!(token);
        }

        let one = created(&router, case, &clusters[0], first).await;
        let two = created(&router, case, &second_cluster, second).await;

        if same {
            assert_eq!(
                without_state(&two),
                without_state(&one),
                "case {case:?}: expected the retry to answer with the first group, got {two}"
            );
            // The retry created no second group: the one group is what the
            // reader reports, under the name and node the FIRST create gave it.
            let read = described(&router, case, &clusters[0], &id_of(&one)).await;
            assert_eq!(
                read["name"],
                json!("first"),
                "case {case:?}: expected the retry to leave the stored name alone, got {:?}",
                read["name"]
            );
            assert_eq!(
                read["nodes"],
                json!([node.clone()]),
                "case {case:?}: expected the retry to leave the stored nodes alone, got {:?}",
                read["nodes"]
            );
            continue;
        }

        assert_ne!(
            id_of(&two),
            id_of(&one),
            "case {case:?}: expected two distinct groups, got the same id twice"
        );
        // Each group carries its own create's values, read back out of the
        // store.
        let read = described(&router, case, &second_cluster, &id_of(&two)).await;
        assert_eq!(
            read["name"],
            json!("second"),
            "case {case:?}: expected the second group to carry its own name, got {:?}",
            read["name"]
        );
        assert_eq!(
            read["clusterId"],
            json!(second_cluster),
            "case {case:?}: expected the second group to belong to its own cluster, got {:?}",
            read["clusterId"]
        );
    }
}

/// Rule: the store is bounded — once it holds `MAX_CHANNEL_PLACEMENT_GROUPS`
/// groups a create is refused with `TooManyRequestsException` (HTTP 429), the
/// refusal this operation models for a store that can take no more, and nothing
/// is minted.
///
/// The ceiling is the emulator's own (it keeps a create loop from growing the
/// heap without bound), so the bound is read from the crate rather than written
/// out here.
#[tokio::test]
async fn the_store_is_bounded() {
    let router = medialive();
    let cluster = create_cluster(&router, "the ceiling", "primary").await;

    let mut last = Value::Null;
    for index in 0..MAX_CHANNEL_PLACEMENT_GROUPS {
        last = created(
            &router,
            "the ceiling",
            &cluster,
            json!({ "name": format!("group-{index}") }),
        )
        .await;
    }

    let (status, body) = create_group(&router, &cluster, json!({ "name": "one-too-many" })).await;
    assert_eq!(
        status, 429,
        "expected the create past the ceiling to be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );

    // The refusal minted nothing: the last group that fit is still the newest
    // one, and it is unchanged.
    let read = described(&router, "the ceiling", &cluster, &id_of(&last)).await;
    assert_eq!(
        read, last,
        "expected the refused create to change nothing, got {read}"
    );
}

/// The URI all three tag operations are bound to
/// (`smithy_query(query="operation", operation="CreateTags,DeleteTags,ListTagsForResource")`:
/// `POST`/`DELETE`/`GET /prod/tags/{ResourceArn}`). `ResourceArn` is an
/// `@httpLabel`, so the ARN's `:` separators travel percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("/prod/tags/{}", percent_encoded(arn))
}

/// The tags `ListTagsForResource` reports for `arn`, with the read itself
/// asserted to have succeeded. An absent `Tags` member and an empty map both
/// mean "no tags", so both read here as the empty map.
async fn listed_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = call(router, "GET", &tags_uri(arn), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}): expected 200, got {status}: {body}"
    );
    match body.get("tags") {
        None | Some(Value::Null) => json!({}),
        Some(tags) => tags.clone(),
    }
}

/// Rule: a channel placement group is one of this service's taggable resources,
/// so the `Tags` its create stored are what `ListTagsForResource` reports for
/// its ARN, `CreateTags` adds to that same map and `DeleteTags` removes from it.
///
/// The rows differ along what the create stored, what the later write adds
/// (including a key it overwrites) and which keys are removed, because that is
/// what the rule generalises over.
#[tokio::test]
async fn a_groups_tags_round_trip_through_the_tag_operations() {
    // (case, tags the create stored, tags added later, keys removed, expected)
    let rows: [(&str, Value, Value, &[&str], Value); 4] = [
        (
            "created with tags",
            json!({ "team": "news" }),
            json!({ "tier": "gold" }),
            &["team"],
            json!({ "tier": "gold" }),
        ),
        (
            "created without tags",
            json!({}),
            json!({ "a": "1", "b": "2" }),
            &["b"],
            json!({ "a": "1" }),
        ),
        (
            "a key overwritten",
            json!({ "team": "news" }),
            json!({ "team": "sport" }),
            &[],
            json!({ "team": "sport" }),
        ),
        (
            "every key removed",
            json!({ "team": "news", "tier": "gold" }),
            json!({}),
            &["team", "tier"],
            json!({}),
        ),
    ];

    for (case, stored, added, removed, expected) in rows {
        let router = medialive();
        let cluster = create_cluster(&router, case, "primary").await;
        let mut request = json!({ "name": "tagged" });
        if stored.as_object().is_some_and(|tags| !tags.is_empty()) {
            request["tags"] = stored.clone();
        }
        let group = created(&router, case, &cluster, request).await;
        let arn = group["arn"].as_str().unwrap_or_default().to_string();

        // What the create stored is what the tag reader reports: the group's own
        // map, not a second copy.
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            stored,
            "case {case:?}: expected the tags the create stored"
        );

        if added.as_object().is_some_and(|tags| !tags.is_empty()) {
            let (status, body) = call(
                &router,
                "POST",
                &tags_uri(&arn),
                Some(json!({ "tags": added })),
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?}: CreateTags: expected 204, got {status}: {body}"
            );
        }
        if !removed.is_empty() {
            let query = removed
                .iter()
                .map(|key| format!("tagKeys={}", percent_encoded(key)))
                .collect::<Vec<_>>()
                .join("&");
            let (status, body) = call(
                &router,
                "DELETE",
                &format!("{}?{query}", tags_uri(&arn)),
                None,
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?}: DeleteTags: expected 204, got {status}: {body}"
            );
        }

        assert_eq!(
            listed_tags(&router, case, &arn).await,
            expected,
            "case {case:?}: expected the tag writes to land on the group's own map"
        );

        // The tag writes changed nothing else about the group.
        let read = described(&router, case, &cluster, &id_of(&group)).await;
        assert_eq!(
            read, group,
            "case {case:?}: expected the group itself to be unchanged, got {read}"
        );
    }
}

/// Rule: a group's `Channels` is every channel currently placed in it — a
/// channel names its group in `AnywhereSettings$ChannelPlacementGroupId` — in id
/// order, derived from the channel store on every read rather than recorded on
/// the group.
///
/// The rows differ along how many channels are placed in the group, and every
/// row also has a channel placed nowhere and one placed in another group of the
/// same cluster, so a non-empty answer is not just "every channel".
#[tokio::test]
async fn channels_placed_in_the_group_are_reported() {
    for placed in [0_usize, 1, 2, 3] {
        let case = format!("{placed} channel(s) placed");
        let router = medialive();
        let cluster = create_cluster(&router, &case, "primary").await;
        let group = created(&router, &case, &cluster, json!({ "name": "studio-a" })).await;
        let id = id_of(&group);
        let other = created(&router, &case, &cluster, json!({ "name": "studio-b" })).await;
        let other_id = id_of(&other);

        // A channel in no group at all, and one in the other group: neither
        // belongs in this group's list.
        create_channel(&router, &case, "unplaced", None).await;
        create_channel(&router, &case, "elsewhere", Some((&cluster, &other_id))).await;

        let mut expected = Vec::new();
        for index in 0..placed {
            expected.push(
                create_channel(
                    &router,
                    &case,
                    &format!("placed-{index}"),
                    Some((&cluster, &id)),
                )
                .await,
            );
        }
        expected.sort();

        // A channel that names this group id while running on ANOTHER cluster is
        // not placed here: both halves of its `anywhereSettings` are matched.
        let elsewhere = create_cluster(&router, &case, "secondary").await;
        create_channel(
            &router,
            &case,
            "other-cluster",
            Some((&elsewhere, &id)),
        )
        .await;

        let read = described(&router, &case, &cluster, &id).await;
        assert_eq!(
            channels_of(&read),
            expected,
            "case {case:?}: expected the channels placed in the group, in id order, got {:?}",
            read["channels"]
        );

        // The other group reports only its own channel, so the derivation is
        // per group rather than per cluster.
        let sibling = described(&router, &case, &cluster, &other_id).await;
        assert_eq!(
            channels_of(&sibling).len(),
            1,
            "case {case:?}: expected the other group to report its one channel, got {:?}",
            sibling["channels"]
        );

        // A channel that has been deleted is no longer placed anywhere: the
        // member says *currently*.
        if let Some(doomed) = expected.first().cloned() {
            // A channel is deletable once its create has settled into IDLE, and
            // the delete itself settles into the terminal DELETED.
            wait_for_the_clock();
            let (status, body) =
                call(&router, "DELETE", &format!("/prod/channels/{doomed}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteChannel: expected 200, got {status}: {body}"
            );
            wait_for_the_clock();

            let after = described(&router, &case, &cluster, &id).await;
            assert_eq!(
                channels_of(&after),
                expected[1..].to_vec(),
                "case {case:?}: expected the deleted channel to leave the group, got {:?}",
                after["channels"]
            );
        }
    }
}

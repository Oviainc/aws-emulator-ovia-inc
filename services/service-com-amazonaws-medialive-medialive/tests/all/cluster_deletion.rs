// SPDX-License-Identifier: CC0-1.0

//! `DeleteCluster` — the deletion half of the MediaLive Anywhere cluster's
//! lifecycle — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour, not an internal function's return value.
//!
//! The rules these suites assert:
//!
//! 1. A delete of a cluster the store holds is ACCEPTED: it answers the modelled
//!    202 with that cluster in `DELETING`, the clock owes it the terminal
//!    `DELETED` it then keeps, and every other member it reports is the stored
//!    cluster read back — which `DescribeCluster` and `ListClusters` go on
//!    reporting rather than losing. A repeat is accepted on the same terms.
//! 2. "The Cluster must be idle": a cluster that still holds any of the resources
//!    that live inside one — the channels that run on it, the channel placement
//!    groups created in it, the nodes registered with it — is refused with the
//!    modelled `ConflictException` (409) and left exactly as it was.
//! 3. A `ClusterId` the store holds no cluster for is the modelled
//!    `NotFoundException` (404) naming that id, and deletes nothing.
//! 4. A cluster whose delete has been accepted stays READABLE but takes no
//!    further write, and the `Name` it held — "unique in the AWS account" — is
//!    free for the next create.
//! 5. A cluster in the terminal `DELETED` state is no longer *using* anything, so
//!    it leaves the `AssociatedClusterIds` a network reports and stops blocking
//!    that network's own delete.
//!
//! The create and read side of the resource lives in `cluster_creation`,
//! `cluster_description` and `cluster_listing`; the helpers for the resources a
//! cluster holds are the ones the sibling suites already own
//! ([`crate::channel_placement_groups`], [`crate::node_lifecycle`],
//! [`crate::networks`]), so this suite spells no second way of creating one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_groups::{
    create_channel, create_channel_in_cluster, create_cluster, create_nodes, deregister_node,
    group_uri, groups_uri,
};
use crate::support::{percent_encoded, resolved_network_ids};
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT};

/// The cluster collection URI `CreateCluster` and `ListClusters` are bound to
/// (`smithy_query(query="operation",operation="CreateCluster,DeleteCluster")`:
/// POST /prod/clusters, DELETE /prod/clusters/{ClusterId}).
const CLUSTERS: &str = "/prod/clusters";

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape",shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// An id in no MediaLive resource range this emulator mints from, so the store
/// holds no cluster for it.
const STRAY_CLUSTER_ID: &str = "8009999";

/// `CreateCluster` with `body`, answering the created cluster with the modelled
/// 201 asserted.
async fn created(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, cluster) = call(router, "POST", CLUSTERS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateCluster to answer 201, got {status}: {cluster}"
    );
    cluster
}

/// `DeleteCluster` on `id`.
async fn delete(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("{CLUSTERS}/{id}"), None).await
}

/// `DeleteCluster` on `id`, answering the cluster it reports, with the call
/// asserted to have answered the modelled 202 and to report the cluster in
/// `state`.
async fn accepted(router: &MultiServiceRouter, case: &str, id: &str, state: &str) -> Value {
    let (status, cluster) = delete(router, id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected DeleteCluster to answer 202, got {status}: {cluster}"
    );
    assert_eq!(
        cluster["state"],
        json!(state),
        "case {case:?}: expected the delete to report {state}, got {:?}",
        cluster["state"]
    );
    cluster
}

/// `DescribeCluster` on `id`, asserted to be readable: a deleted cluster's record
/// stays in the account.
async fn describe(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, cluster) = call(router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeCluster to answer 200, got {status}: {cluster}"
    );
    cluster
}

/// The `id` of a cluster-shaped response.
fn id_of(case: &str, cluster: &Value) -> String {
    cluster["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted cluster id, got {cluster}"))
        .to_string()
}

/// Every cluster `ListClusters` reports right now, in the order it reports them.
async fn listed(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let (status, body) = call(router, "GET", CLUSTERS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListClusters to answer 200, got {status}: {body}"
    );
    match body.get("clusters") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a clusters list, got {other}"),
    }
}

/// The one cluster `ListClusters` reports, asserted to be the only one stored.
async fn only_listed(router: &MultiServiceRouter, case: &str) -> Value {
    let mut page = listed(router, case).await;
    assert_eq!(
        page.len(),
        1,
        "case {case:?}: expected the deleted cluster to still be listed, got {page:?}"
    );
    page.remove(0)
}

/// Every member a cluster-shaped response carries beside the `State` the
/// lifecycle owns (`smithy_query(query="operation", operation="DeleteCluster")`).
const CONFIGURATION: [&str; 7] = [
    "arn",
    "id",
    "name",
    "clusterType",
    "instanceRoleArn",
    "networkSettings",
    "channelIds",
];

/// Assert every member but `state` reads back in `after` exactly as it did in
/// `before`: a deletion moves the lifecycle member and nothing else.
fn configuration_is_unchanged(case: &str, step: &str, before: &Value, after: &Value) {
    for name in CONFIGURATION {
        assert_eq!(
            after.get(name).cloned().unwrap_or(Value::Null),
            before.get(name).cloned().unwrap_or(Value::Null),
            "case {case:?}: {step}: expected {name} to be unchanged, got {:?}",
            after.get(name)
        );
    }
}

/// One row of the deletion table: the create the row deletes, when the delete is
/// called and how many times.
struct Deletion {
    case: &'static str,
    /// The `CreateCluster` body the row stores.
    body: fn() -> Value,
    /// Whether the create's transient `CREATING` is allowed to settle into
    /// `ACTIVE` before the delete is called.
    settle_before_delete: bool,
    /// How many `DeleteCluster` calls the row makes inside the `DELETING`
    /// window. A row with more than one asserts the delete is idempotent.
    calls: usize,
}

/// Rule: `DeleteCluster` accepts the deletion of a stored cluster — it answers
/// 202 with that cluster in `DELETING`, the clock owes it the terminal `DELETED`
/// it then keeps, and every other member it reports is the stored cluster read
/// back, which `DescribeCluster` and `ListClusters` go on reporting rather than
/// losing. A repeated delete is accepted on the same terms: it neither restarts
/// the clock nor revives the cluster.
///
/// The rows differ along WHAT the create stored (nothing but a type, a name, a
/// role ARN, network settings of several mapping sizes), along WHEN the delete is
/// called (inside the transient `CREATING` window and after it has settled) and
/// along HOW MANY deletes are called, because that is what the rule generalises
/// over.
#[tokio::test]
async fn a_deleted_cluster_moves_through_deleting_to_the_terminal_deleted() {
    let table = [
        Deletion {
            case: "a name, deleted while still creating",
            body: || json!({ "name": "doomed", "clusterType": ON_PREMISES }),
            settle_before_delete: false,
            calls: 1,
        },
        Deletion {
            case: "no name at all, deleted once active",
            body: || json!({ "clusterType": ON_PREMISES }),
            settle_before_delete: true,
            calls: 1,
        },
        Deletion {
            case: "a role ARN and one interface mapping, deleted once active",
            body: || {
                json!({
                    "name": "one-mapping",
                    "clusterType": ON_PREMISES,
                    "instanceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaLiveAnywhere"),
                    "networkSettings": {
                        "defaultRoute": "primary",
                        "interfaceMappings": [
                            { "logicalInterfaceName": "primary", "networkId": "8000000" }
                        ]
                    }
                })
            },
            settle_before_delete: true,
            calls: 1,
        },
        Deletion {
            case: "three interface mappings, deleted three times over",
            body: || {
                json!({
                    "name": "three-mappings",
                    "clusterType": ON_PREMISES,
                    "networkSettings": {
                        "interfaceMappings": [
                            { "logicalInterfaceName": "primary", "networkId": "8000000" },
                            { "logicalInterfaceName": "backup", "networkId": "8000001" },
                            { "logicalInterfaceName": "management", "networkId": "8000002" }
                        ]
                    }
                })
            },
            settle_before_delete: true,
            calls: 3,
        },
        Deletion {
            case: "an empty mapping list, deleted twice while still creating",
            body: || {
                json!({
                    "name": "empty-mappings",
                    "clusterType": ON_PREMISES,
                    "networkSettings": { "interfaceMappings": [] }
                })
            },
            settle_before_delete: false,
            calls: 2,
        },
    ];

    for Deletion {
        case,
        body,
        settle_before_delete,
        calls,
    } in table
    {
        let router = medialive();
        // The networks the row's mappings name are put in the account first: a
        // mapping may only name a network this account holds.
        let body = resolved_network_ids(&router, body()).await;
        let cluster = created(&router, case, body).await;
        let id = id_of(case, &cluster);

        if settle_before_delete {
            wait_for_the_clock();
        }

        // What the cluster looks like one instant before the delete, read back
        // out of the store through its own reader.
        let before = describe(&router, case, &id).await;
        let state_before = if settle_before_delete {
            "ACTIVE"
        } else {
            "CREATING"
        };
        assert_eq!(
            before["state"],
            json!(state_before),
            "case {case:?}: expected the cluster to be {state_before} before the delete, got {:?}",
            before["state"]
        );

        for index in 0..calls {
            let step = format!("delete {index}");
            // The delete reports the cluster in the transient state the clock
            // owes the terminal `DELETED`, and changes nothing else about it.
            let answered = accepted(&router, case, &id, "DELETING").await;
            configuration_is_unchanged(case, &step, &before, &answered);

            // Both readers answer from the one stored record, so each reports
            // exactly what the delete just answered.
            let described = describe(&router, case, &id).await;
            assert_eq!(
                described, answered,
                "case {case:?}: {step}: expected DescribeCluster to report what the delete did"
            );
            assert_eq!(
                only_listed(&router, case).await,
                answered,
                "case {case:?}: {step}: expected the listing to report what the delete did"
            );
        }

        wait_for_the_clock();

        // The deletion has come due: the record is still there, in the terminal
        // state it now keeps, with every other member as it was.
        let after = describe(&router, case, &id).await;
        assert_eq!(
            after["state"],
            json!("DELETED"),
            "case {case:?}: expected the deletion to have settled, got {:?}",
            after["state"]
        );
        configuration_is_unchanged(case, "after the deletion came due", &before, &after);
        assert_eq!(
            only_listed(&router, case).await,
            after,
            "case {case:?}: expected the listing to agree with DescribeCluster"
        );

        // A delete of a cluster already in the terminal state is accepted and
        // revives nothing: the state the caller is asking for is the stored one.
        let again = accepted(&router, case, &id, "DELETED").await;
        configuration_is_unchanged(case, "a delete after the terminal state", &before, &again);
        let reread = describe(&router, case, &id).await;
        assert_eq!(
            reread["state"],
            json!("DELETED"),
            "case {case:?}: expected the repeat not to restart the clock, got {:?}",
            reread["state"]
        );
    }
}

/// One row of the idle table: how many of each resource the cluster under test
/// holds when its delete is called.
struct Held {
    case: &'static str,
    /// Channels whose `AnywhereSettings$ClusterId` names the cluster.
    channels: usize,
    /// Channel placement groups created in the cluster.
    groups: usize,
    /// Nodes registered with the cluster.
    nodes: usize,
}

/// One channel placement group created in `cluster`, answering its minted id.
async fn create_group(router: &MultiServiceRouter, case: &str, cluster: &str, name: &str) -> String {
    let group = crate::channel_placement_groups::created(
        router,
        case,
        cluster,
        json!({ "name": name }),
    )
    .await;
    crate::channel_placement_groups::id_of(&group)
}

/// Rule: a cluster is deleted only once it is idle — "Delete a Cluster. The
/// Cluster must be idle" — so a cluster that still holds any of the resources
/// that live inside one (the channels that run on it, the channel placement
/// groups created in it, the nodes registered with it) is refused with the
/// operation's modelled `ConflictException` (409) and left exactly as it was,
/// while the same cluster is accepted once each of those resources has reached
/// its own terminal state.
///
/// The rows differ along WHICH kind of resource is still in the cluster and HOW
/// MANY of them (none at all, one channel, two channels, a group, a node, a group
/// and a node, a channel and two nodes), because that is what the rule
/// generalises over. Every row also stands up a SECOND cluster holding one of
/// each, so what decides the refusal is what is in the cluster the request names
/// and not what the store holds.
#[tokio::test]
async fn a_cluster_a_resource_is_still_in_is_not_deleted() {
    let table = [
        Held { case: "nothing in it", channels: 0, groups: 0, nodes: 0 },
        Held { case: "one channel", channels: 1, groups: 0, nodes: 0 },
        Held { case: "two channels", channels: 2, groups: 0, nodes: 0 },
        Held { case: "one channel placement group", channels: 0, groups: 1, nodes: 0 },
        Held { case: "one node", channels: 0, groups: 0, nodes: 1 },
        Held { case: "a group and a node", channels: 0, groups: 1, nodes: 1 },
        Held { case: "a channel and two nodes", channels: 1, groups: 0, nodes: 2 },
    ];

    for Held {
        case,
        channels,
        groups,
        nodes,
    } in table
    {
        let router = medialive();
        let id = create_cluster(&router, case, "cluster-under-test").await;

        // The bystander holds one of each for every row, so a refusal (or an
        // acceptance) has to be decided on the cluster the request names.
        let bystander = create_cluster(&router, case, "cluster-bystander").await;
        create_channel_in_cluster(&router, case, "bystander-channel", &bystander).await;
        create_group(&router, case, &bystander, "bystander-group").await;
        create_nodes(&router, case, &bystander, 1).await;

        // What the row puts in the cluster under test, each id minted by the
        // create that put it there.
        let mut held: Vec<String> = Vec::new();
        let mut channel_ids: Vec<String> = Vec::new();
        for index in 0..channels {
            let name = format!("held-channel-{index}");
            // The channel runs in the cluster under test and in no group of it:
            // a group would be one more resource the delete has to refuse on,
            // and this row is about the channel.
            let channel = create_channel_in_cluster(&router, case, &name, &id).await;
            channel_ids.push(channel.clone());
            held.push(channel);
        }
        let mut group_ids: Vec<String> = Vec::new();
        for index in 0..groups {
            let group = create_group(&router, case, &id, &format!("held-group-{index}")).await;
            group_ids.push(group.clone());
            held.push(group);
        }
        let node_ids = create_nodes(&router, case, &id, nodes).await;
        held.extend(node_ids.iter().cloned());

        // The creates have come due, so every channel is deletable (`IDLE`) and
        // the clusters are `ACTIVE`: the delete below is refused on what is in
        // the cluster rather than on anything still in flight.
        wait_for_the_clock();
        let before = describe(&router, case, &id).await;

        if held.is_empty() {
            let answered = accepted(&router, case, &id, "DELETING").await;
            configuration_is_unchanged(case, "an idle cluster", &before, &answered);
        } else {
            let (status, refusal) = delete(&router, &id).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected a cluster holding {} resource(s) to be refused, \
                 got {status}: {refusal}",
                held.len()
            );
            assert_eq!(
                error_code(status, &refusal),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {refusal}"
            );
            let message = error_message(&refusal);
            assert!(
                message.contains(&id),
                "case {case:?}: expected the refusal to name the cluster, got {message:?}"
            );
            assert!(
                message.contains(&held.len().to_string()),
                "case {case:?}: expected the refusal to count the {} resource(s) still in the \
                 cluster, got {message:?}",
                held.len()
            );
            for resource in &held {
                assert!(
                    message.contains(resource.as_str()),
                    "case {case:?}: expected the refusal to name {resource}, got {message:?}"
                );
            }

            // The refusal wrote nothing: the cluster is still exactly what it
            // was, the refusal having been taken before any state was touched.
            let after = describe(&router, case, &id).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused delete to leave the cluster alone"
            );

            // Empty the cluster: each resource is deleted through its own
            // operation and left to reach its own terminal state.
            for channel in &channel_ids {
                let (status, body) =
                    call(&router, "DELETE", &format!("/prod/channels/{channel}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteChannel({channel}): expected 200, got {status}: {body}"
                );
            }
            for group in &group_ids {
                let (status, body) = call(&router, "DELETE", &group_uri(&id, group), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteChannelPlacementGroup({group}): expected 200, \
                     got {status}: {body}"
                );
            }
            for node in &node_ids {
                deregister_node(&router, case, &id, node).await;
            }
            wait_for_the_clock();

            // Idle at last, so the same delete is accepted and reports the
            // cluster in `DELETING`.
            let answered = accepted(&router, case, &id, "DELETING").await;
            assert_eq!(
                answered["channelIds"].as_array().map(Vec::len).unwrap_or_default(),
                0,
                "case {case:?}: expected no channel left on the cluster, got {:?}",
                answered["channelIds"]
            );
        }

        // The bystander still holds one of each, so its own delete is still
        // refused: the rule is about the cluster the request names.
        let (status, refusal) = delete(&router, &bystander).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the bystander's delete to be refused, got {status}: {refusal}"
        );
    }
}

/// Rule: a `DeleteCluster` whose `ClusterId` names no cluster this store holds
/// answers the operation's modelled `NotFoundException` (404) naming that id, and
/// deletes nothing — each resource type's ids come from a range of its own, so
/// another type's id names no cluster either.
///
/// The rows differ along the FORM of the id: one of the cluster id range nothing
/// minted, a channel's id, a node's id, a non-numeric label, the cluster's own
/// NAME, and the spellings a client can still put in the path.
#[tokio::test]
async fn a_delete_of_a_cluster_id_nothing_minted_is_not_found() {
    let case = "the bystander";
    let router = medialive();
    let bystander = create_cluster(&router, case, "cluster-present").await;
    let channel = create_channel(&router, case, "neighbour", None).await;
    let node = create_nodes(&router, case, &bystander, 1)
        .await
        .remove(0);

    let misses: [(&str, String); 7] = [
        ("a never-minted id of the same form", STRAY_CLUSTER_ID.to_string()),
        ("a channel's id", channel),
        ("a node's id", node),
        ("a non-numeric label", "not-a-cluster".to_string()),
        ("the cluster's name, not its id", "cluster-present".to_string()),
        ("a longer numeric id", "8000000000".to_string()),
        ("a negative number", "-1".to_string()),
    ];

    for (miss, id) in &misses {
        assert_ne!(
            id, &bystander,
            "case {miss:?}: the row must name an id the store does NOT hold a cluster for"
        );
        let (status, body) = delete(&router, id).await;
        assert_eq!(
            status, 404,
            "case {miss:?}: expected 404 for {id:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {miss:?}: expected NotFoundException, got {body}"
        );
        assert!(
            error_message(&body).contains(id.as_str()),
            "case {miss:?}: expected the refusal to name the id asked for, got {:?}",
            error_message(&body)
        );
    }

    // Every refusal above deleted nothing: the cluster the store does hold still
    // has the lifecycle of a cluster nobody deleted.
    let described = describe(&router, "after the misses", &bystander).await;
    for state in ["DELETING", "DELETED"] {
        assert_ne!(
            described["state"],
            json!(state),
            "expected the refused deletes to have started no deletion, got {described}"
        );
    }
}

/// One mutation addressed to a cluster: the operation, the request it sends for a
/// cluster id, and the refusal a cluster whose delete has been accepted answers
/// it with.
struct Mutation {
    /// The operation, as the model names it.
    op: &'static str,
    /// Its request: `(method, uri, body)` built from the cluster id.
    request: fn(&str) -> (&'static str, String, Option<Value>),
    /// The status the refusal travels with, and the error code the operation's
    /// model gives it.
    status: u16,
    code: &'static str,
}

/// Every mutation this rule covers, with the request each sends. The error each
/// one's model lets it answer a `ClusterId` it cannot write through differs
/// (`UpdateCluster` and `CreateNode` model no 404 and answer their
/// `BadRequestException`; `CreateChannelPlacementGroup` answers the 422 it
/// refuses an unprocessable path with), which is why the row carries it.
const MUTATIONS: &[Mutation] = &[
    Mutation {
        op: "UpdateCluster",
        request: |id| {
            (
                "PUT",
                format!("/prod/clusters/{id}"),
                Some(json!({ "name": "renamed" })),
            )
        },
        status: 400,
        code: "BadRequestException",
    },
    Mutation {
        op: "CreateNode",
        request: |id| {
            (
                "POST",
                format!("/prod/clusters/{id}/nodes"),
                Some(json!({ "name": "late-node" })),
            )
        },
        status: 400,
        code: "BadRequestException",
    },
    Mutation {
        op: "CreateChannelPlacementGroup",
        request: |id| ("POST", groups_uri(id), Some(json!({ "name": "late-group" }))),
        status: 422,
        code: "UnprocessableEntityException",
    },
    Mutation {
        op: "CreateNodeRegistrationScript",
        request: |id| {
            (
                "POST",
                format!("/prod/clusters/{id}/nodeRegistrationScript"),
                Some(json!({})),
            )
        },
        status: 400,
        code: "BadRequestException",
    },
];

/// Rule: a cluster whose delete this service has accepted stays READABLE but
/// takes no further write — every mutation addressed to it resolves its
/// `ClusterId` through the live record first and is refused with the error its own
/// model gives an id it cannot write through, never with a conflict — and the
/// `Name` it held ("unique in the AWS account") is free for the next create.
///
/// The rows span the OPERATION doing the mutating and the PHASE the accepted
/// delete is in: the transient `DELETING` the clock still owes the terminal state,
/// and the terminal `DELETED` the record then keeps.
#[tokio::test]
async fn a_deleted_cluster_takes_no_write_and_releases_its_name() {
    for (case, settle_after_delete) in [
        ("inside the DELETING window", false),
        ("after the terminal DELETED", true),
    ] {
        let router = medialive();
        let first = created(
            &router,
            case,
            json!({ "name": "shared", "clusterType": ON_PREMISES, "requestId": "token-1" }),
        )
        .await;
        let id = id_of(case, &first);
        let arn = first["arn"].as_str().unwrap_or_default().to_string();

        accepted(&router, case, &id, "DELETING").await;
        if settle_after_delete {
            wait_for_the_clock();
        }
        let phase = if settle_after_delete {
            "DELETED"
        } else {
            "DELETING"
        };
        let before = describe(&router, case, &id).await;
        assert_eq!(
            before["state"],
            json!(phase),
            "case {case:?}: the test did not reach {phase}, it reached {:?}",
            before["state"]
        );

        for mutation in MUTATIONS {
            let (method, uri, body) = (mutation.request)(&id);
            let (status, refusal) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, mutation.status,
                "case {case:?}/{}: expected {}, got {status}: {refusal}",
                mutation.op, mutation.status
            );
            assert_eq!(
                error_code(status, &refusal),
                mutation.code,
                "case {case:?}/{}: expected {}, got {refusal}",
                mutation.op,
                mutation.code
            );
            assert_ne!(
                status, 409,
                "case {case:?}/{}: a tombstone must not earn the conflict a live cluster gets",
                mutation.op
            );
        }

        // A tag write reaches the same resolution: the ARN of a cluster on its
        // way out names no resource a caller may still tag.
        let (status, tagged) = call(
            &router,
            "POST",
            &format!("/prod/tags/{}", percent_encoded(&arn)),
            Some(json!({ "tags": { "phase": "after" } })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected CreateTags on the tombstone to be refused, \
             got {status}: {tagged}"
        );

        // None of those refusals changed the record: it still reads back exactly
        // as it did before them.
        let after = describe(&router, case, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused writes to leave the cluster alone"
        );

        // The name is free again, so a second cluster takes it rather than being
        // refused the conflict a live namesake earns — and the deleted record
        // still reports the name it was created under.
        let second = created(
            &router,
            case,
            json!({ "name": "shared", "clusterType": ON_PREMISES }),
        )
        .await;
        let second_id = id_of(case, &second);
        assert_ne!(
            second_id, id,
            "case {case:?}: expected the reused name to mint a second cluster"
        );
        assert_eq!(
            after["name"],
            json!("shared"),
            "case {case:?}: expected the deleted record to keep its name, got {:?}",
            after["name"]
        );

        // The token's idempotency window is over exactly when the deletion has
        // come due: before that a retry still answers with the cluster it
        // created, and afterwards it mints a new one.
        let retried = created(
            &router,
            case,
            json!({ "name": "token-retry", "clusterType": ON_PREMISES, "requestId": "token-1" }),
        )
        .await;
        let retried_id = id_of(case, &retried);
        if settle_after_delete {
            assert!(
                retried_id != id && retried_id != second_id,
                "case {case:?}: expected a retry past the deletion to mint a new cluster, \
                 got {retried_id}"
            );
        } else {
            assert_eq!(
                retried_id, id,
                "case {case:?}: expected a retry inside the window to answer with the cluster \
                 the token created"
            );
        }
    }
}

/// Rule: a cluster whose delete this service has ACCEPTED is no longer *using*
/// anything, so it leaves the `AssociatedClusterIds` a network reports and stops
/// blocking that network's own delete — in the transient `DELETING` step the
/// clock owes as much as in the terminal `DELETED` state.
///
/// The rows differ along the PHASE the cluster's accepted delete is in, because
/// the rule has to hold the same way in both.
#[tokio::test]
async fn a_cluster_leaves_a_network_once_its_delete_is_accepted() {
    for (case, settle_after_delete) in [
        ("inside the DELETING window", false),
        ("after the terminal DELETED", true),
    ] {
        let router = medialive();
        let network = crate::networks::created(&router, case, json!({ "name": "net" })).await;
        let network_id = crate::networks::id_of(case, &network);

        let cluster = created(
            &router,
            case,
            json!({
                "name": "mapped",
                "clusterType": ON_PREMISES,
                "networkSettings": {
                    "interfaceMappings": [
                        { "logicalInterfaceName": "primary", "networkId": network_id }
                    ]
                }
            }),
        )
        .await;
        let cluster_id = id_of(case, &cluster);
        wait_for_the_clock();

        // The association the network's own delete is decided on.
        let (status, described) = crate::networks::describe(&router, &network_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNetwork to answer 200, got {status}: {described}"
        );
        assert_eq!(
            crate::networks::list_member(&described, "associatedClusterIds"),
            json!([cluster_id.clone()]),
            "case {case:?}: expected the mapped cluster to be reported, got {:?}",
            described["associatedClusterIds"]
        );

        accepted(&router, case, &cluster_id, "DELETING").await;
        if settle_after_delete {
            wait_for_the_clock();
        }

        let (status, described) = crate::networks::describe(&router, &network_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNetwork to answer 200, got {status}: {described}"
        );
        assert_eq!(
            crate::networks::list_member(&described, "associatedClusterIds"),
            json!([]),
            "case {case:?}: expected the cluster whose delete was accepted to be gone, got {:?}",
            described["associatedClusterIds"]
        );

        // And the network's own delete follows that association exactly.
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/prod/networks/{network_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected DeleteNetwork to answer 202, got {status}: {body}"
        );
    }
}

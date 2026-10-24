// SPDX-License-Identifier: CC0-1.0

//! `UpdateChannelPlacementGroup`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The clusters and the groups the rows change are created through
//! `CreateCluster` and `CreateChannelPlacementGroup`, through the one harness the
//! create and read suite owns ([`crate::channel_placement_groups`]), so every
//! id here is an id the store really minted.
//!
//! The rules these suites assert, each over a table that spans its class:
//!
//! 1. An update replaces exactly the members the request CARRIES and leaves the
//!    ones it omits ("Include this parameter only if you want to change the
//!    current ..." on both `Name` and `Nodes`), and what it answers is what a
//!    later `DescribeChannelPlacementGroup` reports.
//! 2. A node set that really changed starts the association the clock owes the
//!    group: `ASSIGNING` into `ASSIGNED` when the group now names a node,
//!    `UNASSIGNING` into `UNASSIGNED` when it no longer does. A node set the
//!    group already holds, or an omitted one, leaves the state it had reached.
//! 3. A request the service cannot apply is refused with the modelled error for
//!    its class and NOTHING is written: a pair naming no group of that cluster is
//!    404 `NotFoundException`, a `Name` another group of the same cluster holds
//!    is 409 `ConflictException`, and more nodes than the one `Nodes` documents
//!    is 422 `UnprocessableEntityException` naming the member.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_groups::{
    create_cluster, created, described, group_uri, id_of, without_state,
};
use crate::wire::{call, error_code, medialive};

/// One `UpdateChannelPlacementGroup` call
/// (`smithy_query(query="operation", operation="UpdateChannelPlacementGroup")`:
/// `PUT /prod/clusters/{ClusterId}/channelplacementgroups/{ChannelPlacementGroupId}`).
async fn update_group(
    router: &MultiServiceRouter,
    cluster: &str,
    id: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "PUT", &group_uri(cluster, id), Some(body)).await
}

/// The group an update reports, with the update itself asserted to have
/// succeeded. `200 OK` is the status the model declares for it.
async fn updated(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    id: &str,
    body: Value,
) -> Value {
    let (status, group) = update_group(router, cluster, id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateChannelPlacementGroup: expected 200, got {status}: {group}"
    );
    group
}

/// One row of the update table: the case, the nodes the group is created with,
/// the update's request body, the `name` and `nodes` expected afterwards, the
/// state the update itself reports and the state the clock settles it into.
type UpdateRow = (
    &'static str,
    Value,
    Value,
    Value,
    Value,
    &'static str,
    &'static str,
);

/// Rule: an update changes exactly the members the request carries, leaves the
/// ones it omits, and a node set that really changed starts the association the
/// clock owes the group — `ASSIGNING` into `ASSIGNED` when it now names a node,
/// `UNASSIGNING` into `UNASSIGNED` when it no longer does — while a node set the
/// group already holds leaves the state it had reached. What the update answers
/// is what the describe reports.
///
/// The rows differ along which members the request carries, along the node set
/// the group starts from and along the node set it ends with (none -> one, one ->
/// another, one -> none, one -> the same one), because that is what the rule
/// generalises over; the states are the model's own
/// (`smithy_query(query="shape", shape="ChannelPlacementGroupState")`).
#[tokio::test]
async fn an_update_changes_the_members_it_carries() {
    let rows: [UpdateRow; 7] = [
        (
            "a rename, nodes omitted",
            json!(["node-a"]),
            json!({ "name": "renamed" }),
            json!("renamed"),
            json!(["node-a"]),
            "ASSIGNED",
            "ASSIGNED",
        ),
        (
            "a node where there was none",
            json!([]),
            json!({ "nodes": ["node-b"] }),
            json!("created"),
            json!(["node-b"]),
            "ASSIGNING",
            "ASSIGNED",
        ),
        (
            "another node",
            json!(["node-a"]),
            json!({ "nodes": ["node-c"] }),
            json!("created"),
            json!(["node-c"]),
            "ASSIGNING",
            "ASSIGNED",
        ),
        (
            "the last node taken away",
            json!(["node-a"]),
            json!({ "nodes": [] }),
            json!("created"),
            json!([]),
            "UNASSIGNING",
            "UNASSIGNED",
        ),
        (
            "the node it already has, resent",
            json!(["node-a"]),
            json!({ "nodes": ["node-a"] }),
            json!("created"),
            json!(["node-a"]),
            "ASSIGNED",
            "ASSIGNED",
        ),
        (
            "a name and a node at once",
            json!([]),
            json!({ "name": "renamed-and-assigned", "nodes": ["node-d"] }),
            json!("renamed-and-assigned"),
            json!(["node-d"]),
            "ASSIGNING",
            "ASSIGNED",
        ),
        (
            "neither member",
            json!(["node-a"]),
            json!({}),
            json!("created"),
            json!(["node-a"]),
            "ASSIGNED",
            "ASSIGNED",
        ),
    ];

    for (case, start_nodes, request, name, nodes, state, settled_state) in rows {
        let router = medialive();
        let cluster = create_cluster(&router, case, "primary").await;
        let group = created(
            &router,
            case,
            &cluster,
            json!({ "name": "created", "nodes": start_nodes }),
        )
        .await;
        let id = id_of(&group);
        // Start from a settled group, so the state the update reports is the one
        // the update put it in rather than one left over from the create.
        wait_for_the_clock();

        let after = updated(&router, case, &cluster, &id, request).await;

        assert_eq!(
            after["name"], name,
            "case {case:?}: expected name {name}, got {:?}",
            after["name"]
        );
        assert_eq!(
            after["nodes"], nodes,
            "case {case:?}: expected nodes {nodes}, got {:?}",
            after["nodes"]
        );
        assert_eq!(
            after["state"],
            json!(state),
            "case {case:?}: expected state {state} right after the update, got {:?}",
            after["state"]
        );
        // An update neither mints a new group nor re-mints its ARN, and the
        // cluster it belongs to is not something an update can move.
        assert_eq!(
            after["id"],
            json!(id),
            "case {case:?}: expected the same group, got {:?}",
            after["id"]
        );
        assert_eq!(
            after["arn"], group["arn"],
            "case {case:?}: expected the ARN to be untouched, got {:?}",
            after["arn"]
        );
        assert_eq!(
            after["clusterId"],
            json!(cluster),
            "case {case:?}: expected the group to stay in its cluster, got {:?}",
            after["clusterId"]
        );

        // The round trip: what the update answered is what the store holds.
        let read = described(&router, case, &cluster, &id).await;
        assert_eq!(
            without_state(&read),
            without_state(&after),
            "case {case:?}: expected the describe to report what the update wrote, got {read}"
        );

        // And once the clock has run, the association the update started has
        // settled into the state the new node set implies.
        wait_for_the_clock();
        let settled = described(&router, case, &cluster, &id).await;
        assert_eq!(
            settled["state"],
            json!(settled_state),
            "case {case:?}: expected the group to settle into {settled_state}, got {:?}",
            settled["state"]
        );
        assert_eq!(
            without_state(&settled),
            without_state(&after),
            "case {case:?}: expected only the state to have moved, got {settled}"
        );
    }
}

/// Rule: an update is applied to the group the PAIR of path labels names, so a
/// `ChannelPlacementGroupId` that names no group of the requested `ClusterId` is
/// `NotFoundException` (HTTP 404) — whichever of the two is wrong — and nothing
/// in the store changes.
///
/// The rows differ along which label misses, because that is what the rule
/// generalises over.
#[tokio::test]
async fn an_update_only_reaches_a_group_of_the_cluster_the_path_names() {
    let router = medialive();
    let cluster_one = create_cluster(&router, "scoping", "primary").await;
    let cluster_two = create_cluster(&router, "scoping", "secondary").await;
    let one = created(
        &router,
        "scoping",
        &cluster_one,
        json!({ "name": "in-cluster-one", "nodes": ["node-a"] }),
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
        let (status, body) =
            update_group(&router, cluster, id, json!({ "name": "renamed" })).await;
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

    // Neither group was touched by any of those refusals: both still carry the
    // name and the nodes their own create gave them.
    for (case, cluster, id, expected) in [
        ("first cluster", &cluster_one, &first, &one),
        ("second cluster", &cluster_two, &second, &two),
    ] {
        let read = described(&router, case, cluster, id).await;
        assert_eq!(
            without_state(&read),
            without_state(expected),
            "case {case:?}: expected the group to be unchanged, got {read}"
        );
    }
}

/// Rule: the update holds a changed group to the same two documented bounds the
/// create does — `Nodes` accepts at most one node id ("You can't associate more
/// than one Node with the ChannelPlacementGroup"), and `Name` is unique in the
/// Cluster and case-sensitive — refusing with the modelled error for each class
/// and writing nothing.
///
/// The rows span the node counts around the published bound (none, one, two,
/// three) and the name clashes around uniqueness (another group's name, that
/// name in another case, the group's own name, a free name), because that is
/// what the rule generalises over.
#[tokio::test]
async fn an_update_is_held_to_the_documented_bounds() {
    // (case, nodes the update asks for, accepted)
    let node_rows: [(&str, Value, bool); 4] = [
        ("no node", json!([]), true),
        ("one node", json!(["node-x"]), true),
        ("two nodes", json!(["node-x", "node-y"]), false),
        ("three nodes", json!(["node-x", "node-y", "node-z"]), false),
    ];

    for (case, nodes, accepted) in node_rows {
        let router = medialive();
        let cluster = create_cluster(&router, case, "primary").await;
        let group = created(
            &router,
            case,
            &cluster,
            json!({ "name": "under-test", "nodes": ["node-a"] }),
        )
        .await;
        let id = id_of(&group);
        wait_for_the_clock();
        let before = described(&router, case, &cluster, &id).await;

        let (status, body) =
            update_group(&router, &cluster, &id, json!({ "nodes": nodes.clone() })).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status}: {body}"
            );
            assert_eq!(
                body["nodes"], nodes,
                "case {case:?}: expected the nodes the request named, got {:?}",
                body["nodes"]
            );
            // An accepted update is PERSISTED, not just reported: the describe
            // reads the same node set back out of the store.
            let read = described(&router, case, &cluster, &id).await;
            assert_eq!(
                read["nodes"], nodes,
                "case {case:?}: expected the accepted nodes to be stored, got {:?}",
                read["nodes"]
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
        let after = described(&router, case, &cluster, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused update to change nothing, got {after}"
        );
    }

    // (case, the name the update asks for, which cluster the OTHER group is in,
    //  accepted)
    let name_rows: [(&str, &str, usize, bool); 5] = [
        ("another group's name in this cluster", "taken", 0, false),
        ("that name in another case", "Taken", 0, true),
        ("a name only another cluster uses", "taken", 1, true),
        ("a name nobody holds", "free", 0, true),
        ("the group's own name", "under-test", 0, true),
    ];

    for (case, name, which, accepted) in name_rows {
        let router = medialive();
        let clusters = [
            create_cluster(&router, case, "primary").await,
            create_cluster(&router, case, "secondary").await,
        ];
        // The group that holds the name the update asks for, in whichever
        // cluster this row puts it.
        let holder = created(
            &router,
            case,
            &clusters[which],
            json!({ "name": "taken" }),
        )
        .await;
        let group = created(
            &router,
            case,
            &clusters[0],
            json!({ "name": "under-test" }),
        )
        .await;
        let id = id_of(&group);

        let (status, body) =
            update_group(&router, &clusters[0], &id, json!({ "name": name })).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status}: {body}"
            );
            assert_eq!(
                body["name"],
                json!(name),
                "case {case:?}: expected the name the request asked for, got {:?}",
                body["name"]
            );
            // And the accepted rename is what the store holds afterwards.
            let read = described(&router, case, &clusters[0], &id).await;
            assert_eq!(
                read["name"],
                json!(name),
                "case {case:?}: expected the accepted name to be stored, got {:?}",
                read["name"]
            );
            continue;
        }

        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
        // Neither the group being renamed nor the one holding the name changed.
        let read = described(&router, case, &clusters[0], &id).await;
        assert_eq!(
            read["name"],
            json!("under-test"),
            "case {case:?}: expected the refused update to leave the name alone, got {:?}",
            read["name"]
        );
        let other = described(&router, case, &clusters[which], &id_of(&holder)).await;
        assert_eq!(
            other, holder,
            "case {case:?}: expected the group holding the name to be unchanged, got {other}"
        );
    }
}

/// Rule: an update is a WRITE to the group it names and to nothing else — a
/// sibling group of the same cluster, a group of another cluster, and the
/// channels placed in the group are all left exactly as they were, and the
/// group's `Channels` stays derived from the channel store.
#[tokio::test]
async fn an_update_touches_only_the_group_it_names() {
    let router = medialive();
    let case = "one group at a time";
    let cluster = create_cluster(&router, case, "primary").await;
    let elsewhere = create_cluster(&router, case, "secondary").await;

    let target = created(
        &router,
        case,
        &cluster,
        json!({ "name": "target", "nodes": ["node-a"] }),
    )
    .await;
    let sibling = created(&router, case, &cluster, json!({ "name": "sibling" })).await;
    let distant = created(&router, case, &elsewhere, json!({ "name": "distant" })).await;
    let id = id_of(&target);

    // A channel placed in the target group, so `Channels` is non-empty and can
    // be shown to survive the update.
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "placed",
            "anywhereSettings": { "clusterId": cluster, "channelPlacementGroupId": id },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {channel}"
    );
    let placed = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    wait_for_the_clock();
    let after = updated(
        &router,
        case,
        &cluster,
        &id,
        json!({ "name": "target-renamed", "nodes": ["node-b"] }),
    )
    .await;
    assert_eq!(
        after["channels"],
        json!([placed]),
        "case {case:?}: expected the placed channel to still be reported, got {:?}",
        after["channels"]
    );

    for (what, cluster, group) in [
        ("the sibling group", &cluster, &sibling),
        ("the group of another cluster", &elsewhere, &distant),
    ] {
        let read = described(&router, case, cluster, &id_of(group)).await;
        assert_eq!(
            read, *group,
            "case {case:?}: expected {what} to be untouched, got {read}"
        );
    }
}

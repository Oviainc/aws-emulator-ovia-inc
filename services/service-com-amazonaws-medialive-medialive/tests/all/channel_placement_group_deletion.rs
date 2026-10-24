// SPDX-License-Identifier: CC0-1.0

//! `DeleteChannelPlacementGroup`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the stub issue's single `DeleteChannelPlacementGroup`
//! call is ONE row. The clusters, groups and channels every row works on are
//! created through the service's own creates, so every id here is one the store
//! really minted — the request helpers are the ones the create/read suite
//! beside this one already has ([`crate::channel_placement_groups`]).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_groups::{
    create_channel, create_cluster, create_group, create_nodes, created, describe_group, described,
    group_uri, id_of, without_state, NodesSent,
};
use crate::node_lifecycle::node_uri;
use crate::wire::{call, error_code, error_message, medialive};

/// `DeleteChannelPlacementGroup` is bound to the group instance
/// (`smithy_query(query="operation", operation="DeleteChannelPlacementGroup")`:
/// `DELETE /prod/clusters/{ClusterId}/channelplacementgroups/{ChannelPlacementGroupId}`).
async fn delete_group(router: &MultiServiceRouter, cluster: &str, id: &str) -> (u16, Value) {
    call(router, "DELETE", &group_uri(cluster, id), None).await
}

/// One accepted delete. `200 OK` is the status the model declares for this
/// delete, unlike the node's and the cluster's 202.
async fn deleted(router: &MultiServiceRouter, case: &str, cluster: &str, id: &str) -> Value {
    let (status, body) = delete_group(router, cluster, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteChannelPlacementGroup: expected 200, got {status}: {body}"
    );
    body
}

/// Delete a channel and wait for it to reach the terminal `DELETED`, so it is no
/// longer placed anywhere. A channel is deletable once its create has settled
/// into `IDLE`, which is why the clock runs on both sides of the call.
async fn delete_channel(router: &MultiServiceRouter, case: &str, id: &str) {
    wait_for_the_clock();
    let (status, body) = call(router, "DELETE", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteChannel({id}): expected 200, got {status}: {body}"
    );
    wait_for_the_clock();
}

/// Rule: an accepted delete puts the group into the transient `DELETING` state
/// and the clock owes it the terminal `DELETED`, which the record then keeps —
/// the group stays readable, with every member the create stored unchanged.
///
/// The rows differ along what the created group carried (nothing, a name, a
/// name and a node, tags) and along which of the account's clusters it belongs
/// to, because that is what the rule generalises over; the states are the
/// model's own (`smithy_query(query="shape",
/// shape="ChannelPlacementGroupState")`).
#[tokio::test]
async fn an_accepted_delete_moves_the_group_through_deleting_to_deleted() {
    // (case, which cluster, create request besides `Nodes`, how `Nodes` is sent)
    let rows: [(&str, usize, Value, NodesSent); 5] = [
        ("nothing at all", 0, json!({}), NodesSent::Absent),
        (
            "name only",
            0,
            json!({ "name": "studio-a" }),
            NodesSent::Absent,
        ),
        (
            "name and one node",
            0,
            json!({ "name": "studio-b" }),
            NodesSent::Named(1),
        ),
        ("node only, second cluster", 1, json!({}), NodesSent::Named(1)),
        (
            "tags too, second cluster",
            1,
            json!({ "name": "tagged", "tags": { "team": "news" } }),
            NodesSent::Absent,
        ),
    ];

    for (case, which, request, sent) in rows {
        let router = medialive();
        let clusters = [
            create_cluster(&router, case, "primary").await,
            create_cluster(&router, case, "secondary").await,
        ];
        let cluster = clusters[which].clone();
        let mut request = request;
        // A node a row names is one its own cluster really has: `Nodes` is a
        // reference to a node of the group's cluster.
        if let NodesSent::Named(count) = sent {
            request["nodes"] = json!(create_nodes(&router, case, &cluster, count).await);
        }
        let group = created(&router, case, &cluster, request).await;
        let id = id_of(&group);

        let accepted = deleted(&router, case, &cluster, &id).await;
        assert_eq!(
            accepted["state"],
            json!("DELETING"),
            "case {case:?}: expected the accepted delete to report DELETING, got {:?}",
            accepted["state"]
        );
        // The body is the STORED group, member for member: only the state the
        // acceptance wrote has moved.
        assert_eq!(
            without_state(&accepted),
            without_state(&group),
            "case {case:?}: expected the delete to report the stored group, got {accepted}"
        );

        // The group is still readable, in the state the delete left it: the
        // record is kept rather than dropped.
        let read = described(&router, case, &cluster, &id).await;
        assert_eq!(
            read, accepted,
            "case {case:?}: expected the reader to report the group the delete left, got {read}"
        );

        // Once the clock has run, the deletion has come due and the group is in
        // the terminal `DELETED` state it then keeps.
        wait_for_the_clock();
        let settled = described(&router, case, &cluster, &id).await;
        assert_eq!(
            settled["state"],
            json!("DELETED"),
            "case {case:?}: expected the group to settle into DELETED, got {:?}",
            settled["state"]
        );
        assert_eq!(
            without_state(&settled),
            without_state(&group),
            "case {case:?}: expected only the state to have moved, got {settled}"
        );

        // And it STAYS there: a terminal state owes the clock nothing.
        wait_for_the_clock();
        let again = described(&router, case, &cluster, &id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: expected the deleted group to stay as it was, got {again}"
        );
    }
}

/// Rule: a delete already accepted for a group is accepted again — the state the
/// caller asks for is the one the group is already heading for or in — and the
/// repeat neither restarts the clock nor revives the group.
///
/// The rows differ along how many repeats arrive and whether the clock has
/// already run between them, because that is what the rule generalises over.
#[tokio::test]
async fn a_repeated_delete_is_accepted_without_restarting_the_clock() {
    // (case, waits before each repeat, state each repeat must report)
    let rows: [(&str, &[bool], &[&str]); 4] = [
        ("one repeat, at once", &[false], &["DELETING"]),
        ("one repeat, after the clock", &[true], &["DELETED"]),
        (
            "two repeats, at once",
            &[false, false],
            &["DELETING", "DELETING"],
        ),
        (
            "a repeat on each side of the clock",
            &[false, true],
            &["DELETING", "DELETED"],
        ),
    ];

    for (case, waits, expected) in rows {
        let router = medialive();
        let cluster = create_cluster(&router, case, "primary").await;
        let node = create_nodes(&router, case, &cluster, 1).await.remove(0);
        let group = created(
            &router,
            case,
            &cluster,
            json!({ "name": "studio-a", "nodes": [node] }),
        )
        .await;
        let id = id_of(&group);

        let first = deleted(&router, case, &cluster, &id).await;
        assert_eq!(
            first["state"],
            json!("DELETING"),
            "case {case:?}: expected the first delete to report DELETING, got {:?}",
            first["state"]
        );

        for (index, (wait, state)) in waits.iter().zip(expected).enumerate() {
            if *wait {
                wait_for_the_clock();
            }
            let repeat = deleted(&router, case, &cluster, &id).await;
            assert_eq!(
                repeat["state"],
                json!(*state),
                "case {case:?}: repeat {index}: expected {state}, got {:?}",
                repeat["state"]
            );
            assert_eq!(
                without_state(&repeat),
                without_state(&group),
                "case {case:?}: repeat {index}: expected the stored group unchanged, got {repeat}"
            );
        }

        // Whatever the repeats did, the group ends in the terminal state the
        // FIRST call put it on course for: no repeat pushed the due instant out.
        wait_for_the_clock();
        let settled = described(&router, case, &cluster, &id).await;
        assert_eq!(
            settled["state"],
            json!("DELETED"),
            "case {case:?}: expected the group to be DELETED, got {:?}",
            settled["state"]
        );
    }
}

/// Rule: a group is named by BOTH path labels — it belongs to the cluster it was
/// created in — so a delete whose `ClusterId`/`ChannelPlacementGroupId` pair
/// names no group is `NotFoundException` (HTTP 404) naming the id asked for, and
/// deletes nothing.
///
/// The rows differ along which label misses, because that is what the rule
/// generalises over.
#[tokio::test]
async fn a_delete_needs_both_path_labels_to_name_a_group() {
    let router = medialive();
    let one = create_cluster(&router, "scoping", "primary").await;
    let two = create_cluster(&router, "scoping", "secondary").await;
    let first = id_of(&created(&router, "scoping", &one, json!({ "name": "in-one" })).await);
    let second = id_of(&created(&router, "scoping", &two, json!({ "name": "in-two" })).await);

    // (case, cluster asked for, id asked for)
    let misses: [(&str, &str, &str); 4] = [
        ("another cluster's group", &one, &second),
        ("the group's own id under the other cluster", &two, &first),
        ("an id no group has", &one, "9999999"),
        ("a cluster that does not exist", "7654321", &first),
    ];

    for (case, cluster, id) in misses {
        let (status, body) = delete_group(&router, cluster, id).await;
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
            error_message(&body).contains(id),
            "case {case:?}: expected the refusal to name the id asked for, got {body}"
        );
    }

    // Neither group was touched: each is still live in its own cluster.
    for (case, cluster, id) in [
        ("first cluster", &one, &first),
        ("second cluster", &two, &second),
    ] {
        let read = described(&router, case, cluster, id).await;
        assert_eq!(
            read["state"],
            json!("UNASSIGNED"),
            "case {case:?}: expected the group to be untouched, got {read}"
        );
    }
}

/// Rule: a group channels are still placed in is IN USE, so its delete is
/// refused with `ConflictException` (HTTP 409) — the refusal this delete models
/// and the read of the same group does not — and the group is left exactly as it
/// was; deleting the channels that hold it is what clears the refusal.
///
/// The rows differ along how many channels hold the group, and every row also
/// has a channel placed in another group of the same cluster and one placed
/// nowhere, so a refusal is about the holders of THIS group rather than about
/// the channel store being non-empty.
#[tokio::test]
async fn a_group_channels_are_placed_in_cannot_be_deleted() {
    for holders in [0_usize, 1, 2, 3] {
        let case = format!("{holders} channel(s) placed");
        let router = medialive();
        let cluster = create_cluster(&router, &case, "primary").await;
        let group = created(&router, &case, &cluster, json!({ "name": "studio-a" })).await;
        let id = id_of(&group);
        let other = created(&router, &case, &cluster, json!({ "name": "studio-b" })).await;
        let other_id = id_of(&other);
        // A third group of the same cluster that no channel is ever placed in.
        let spare_id = id_of(&created(&router, &case, &cluster, json!({ "name": "studio-c" })).await);

        create_channel(&router, &case, "unplaced", None).await;
        create_channel(&router, &case, "elsewhere", Some((&cluster, &other_id))).await;
        let mut placed = Vec::new();
        for index in 0..holders {
            placed.push(
                create_channel(
                    &router,
                    &case,
                    &format!("placed-{index}"),
                    Some((&cluster, &id)),
                )
                .await,
            );
        }

        if holders == 0 {
            // The control row: nothing holds the group, so the same delete is
            // accepted.
            let accepted = deleted(&router, &case, &cluster, &id).await;
            assert_eq!(
                accepted["state"],
                json!("DELETING"),
                "case {case:?}: expected the delete to be accepted, got {accepted}"
            );
            continue;
        }

        // The group as it stands with its holders placed in it, so the refusal
        // can be shown to have changed nothing at all.
        let before = described(&router, &case, &cluster, &id).await;

        let (status, body) = delete_group(&router, &cluster, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
        assert!(
            error_message(&body).contains(&id),
            "case {case:?}: expected the refusal to name the group asked for, got {body}"
        );

        // The refusal changed nothing: the group is still live and still
        // reports its holders.
        let read = described(&router, &case, &cluster, &id).await;
        assert_eq!(
            read, before,
            "case {case:?}: expected the refused delete to change nothing, got {read}"
        );

        // A group of the same cluster that nothing holds deletes fine, even
        // while this one is held — so the refusal is per group rather than per
        // cluster.
        let spare = deleted(&router, &case, &cluster, &spare_id).await;
        assert_eq!(
            spare["state"],
            json!("DELETING"),
            "case {case:?}: expected the unheld group's delete to be accepted, got {spare}"
        );

        // Delete every channel that holds the group and the same delete is
        // accepted: the precondition is the live holders, not a flag.
        for channel in &placed {
            delete_channel(&router, &case, channel).await;
        }
        let accepted = deleted(&router, &case, &cluster, &id).await;
        assert_eq!(
            accepted["state"],
            json!("DELETING"),
            "case {case:?}: expected the delete to be accepted once nothing holds the group, got {accepted}"
        );
    }
}

/// Rule: a group a delete has been accepted for is on its way out of the
/// account, so it holds its `Name` no longer — a new group may take it — and it
/// associates its `Nodes` no longer, which is what the node's own
/// `ChannelPlacementGroups` reports.
///
/// The rows differ along whether the clock has run (the group is `DELETING` or
/// the terminal `DELETED`) when the two are checked, because the rule holds from
/// the acceptance onwards rather than from the deletion coming due.
#[tokio::test]
async fn a_deleted_group_holds_neither_its_name_nor_its_node() {
    for settled in [false, true] {
        let case = if settled { "after the clock" } else { "at once" };
        let router = medialive();
        let cluster = create_cluster(&router, case, "primary").await;

        // A node of the same cluster, created through `CreateNode`, so the
        // association the group reports is with a node the store really minted.
        let node_id = create_nodes(&router, case, &cluster, 1).await.remove(0);

        let group = created(
            &router,
            case,
            &cluster,
            json!({ "name": "studio-a", "nodes": [node_id.clone()] }),
        )
        .await;
        let id = id_of(&group);

        // While the group is live the node reports the association.
        let (status, before) = call(&router, "GET", &node_uri(&cluster, &node_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeNode: expected 200, got {status}: {before}"
        );
        assert_eq!(
            before["channelPlacementGroups"],
            json!([id.clone()]),
            "case {case:?}: expected the node to report the group that names it, got {:?}",
            before["channelPlacementGroups"]
        );

        deleted(&router, case, &cluster, &id).await;
        if settled {
            wait_for_the_clock();
        }

        // The node associates the group no longer.
        let (status, after) = call(&router, "GET", &node_uri(&cluster, &node_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeNode: expected 200, got {status}: {after}"
        );
        assert_eq!(
            after["channelPlacementGroups"],
            json!([]),
            "case {case:?}: expected the deleted group to associate nothing, got {:?}",
            after["channelPlacementGroups"]
        );

        // And its name is free: a new group of the same cluster takes it, and
        // is a group of its own.
        let (status, replacement) = create_group(
            &router,
            &cluster,
            json!({ "name": "studio-a", "nodes": [node_id.clone()] }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the deleted group's name to be free, got {status}: {replacement}"
        );
        assert_ne!(
            id_of(&replacement),
            id,
            "case {case:?}: expected a new group rather than the deleted one"
        );

        // The deleted group is still readable, under the name it was created
        // with: a record that is kept, not one that was renamed.
        let read = described(&router, case, &cluster, &id).await;
        assert_eq!(
            read["name"],
            json!("studio-a"),
            "case {case:?}: expected the deleted group to keep its stored name, got {read}"
        );
        let (status, body) = describe_group(&router, &cluster, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deleted group to stay readable, got {status}: {body}"
        );
    }
}

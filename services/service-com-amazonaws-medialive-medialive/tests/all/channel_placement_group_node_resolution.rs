// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED `Nodes` reference on `UpdateChannelPlacementGroup` does to the
//! group's LIFECYCLE, end to end over the restJson1 wire.
//!
//! This suite carries only what
//! [`crate::channel_placement_group_update`]'s own node-resolution table cannot:
//! that table spans the SHAPES of an id that does not resolve, starting from one
//! group in one state, while the rule also has to hold whatever lifecycle the
//! group is in when the refused update arrives — associated with no node at all,
//! associated with one, or with an association still in flight — and for a
//! request that carries a second member alongside the bad one. The call, the
//! group and the node helpers are the ones those suites already own
//! ([`crate::channel_placement_groups`]), so every id here is an id the store
//! really minted and the operation is driven through one wrapper.
//!
//! Rule: `Nodes` on an update is a REFERENCE — "The Node and the
//! ChannelPlacementGroup must be in the same Cluster" — so an id that names no
//! node of the group's own cluster the account can still associate is refused
//! with the operation's modelled client error naming the member, and the update
//! writes NOTHING: neither the node set, nor another member the same request
//! carried, nor the group's lifecycle, which goes on from where it already was
//! instead of starting the association an accepted node set would have.

use serde_json::json;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_group_update::update_group;
use crate::channel_placement_groups::{
    create_cluster, create_nodes, created, deregister_node, described, id_of, without_state,
};
use crate::wire::{error_code, medialive};

/// The name every group under test is created with, so a row can assert the
/// refused update did not write the `Name` it carried alongside the bad node.
const GROUP_NAME: &str = "under-test";

/// The `Name` a row's refused update asks for at the same time as the bad node.
const RENAME_TO: &str = "renamed-by-a-refused-update";

/// The kind of id a row sends in `Nodes`, each RESOLVED from the row's own setup
/// rather than written as a literal: what does not resolve is an id that names
/// no node of this cluster a group can be associated with, not any one id.
#[derive(Clone, Copy, Debug)]
enum Unresolvable {
    /// An id in this service's node id space that no `CreateNode` minted.
    Unminted,
    /// A node of another cluster of the same account — a real node, in the wrong
    /// cluster.
    NodeOfAnotherCluster,
    /// A node of THIS cluster whose deregistration has been accepted, so it is
    /// on its way out of the account and there is nothing left to associate.
    Deregistered,
}

/// One row: the lifecycle the group is in when the refused update arrives, the
/// kind of id that update names, and whether it carries a `Name` too.
#[derive(Clone, Copy, Debug)]
struct Row {
    case: &'static str,
    /// Indices into the nodes the row's own cluster has that the group is
    /// created with. Empty is a group associated with no node.
    nodes: &'static [usize],
    /// Whether the clock has already run once before the update, so the group is
    /// in a settled state rather than in the transient one its create started.
    settle_first: bool,
    /// Whether the refused update also carries a `Name` — a member that WOULD
    /// have been written had the request been accepted.
    also_rename: bool,
    /// The id the update names.
    unresolvable: Unresolvable,
    /// The states the group may be in RIGHT AFTER the refusal: the one it was
    /// already in, since the refusal neither wrote nor started anything. A row
    /// whose own association is still in flight names both of that
    /// association's states, because the clock may legitimately have run between
    /// the read before the update and the one after it — what it may never be is
    /// a state only the refused node set would have led to.
    states_now: &'static [&'static str],
    /// The state the group is in once the clock has run after the refusal: the
    /// one its OWN node set implies, since the refused update changed nothing.
    settled_state: &'static str,
}

/// Rule: whatever lifecycle the group is in, an update whose `Nodes` names an id
/// that is not a node of the group's own cluster is refused with
/// `UnprocessableEntityException` (HTTP 422) naming the member, writes neither
/// the node set nor any other member the same request carried, and starts no
/// transition — so once the clock has run the group is in the state its own node
/// set implies rather than in one the refused node set would have led to.
///
/// The rows differ along the lifecycle the update arrives into (no node at all,
/// one node settled, one node with the association still in flight) and along
/// whether a second member rides along, because that is what this half of the
/// rule generalises over; the id each row names varies with it so the rows are
/// not all about one id either. The states are the model's own
/// (`smithy_query(query="shape", shape="ChannelPlacementGroupState")`).
#[tokio::test]
async fn a_refused_node_reference_starts_no_assignment() {
    let rows: [Row; 4] = [
        Row {
            case: "a group associated with no node",
            nodes: &[],
            settle_first: true,
            also_rename: false,
            unresolvable: Unresolvable::Unminted,
            states_now: &["UNASSIGNED"],
            settled_state: "UNASSIGNED",
        },
        Row {
            case: "a group associated with one node",
            nodes: &[0],
            settle_first: true,
            also_rename: false,
            unresolvable: Unresolvable::NodeOfAnotherCluster,
            states_now: &["ASSIGNED"],
            settled_state: "ASSIGNED",
        },
        Row {
            case: "a group whose association is still in flight",
            nodes: &[0],
            settle_first: false,
            also_rename: false,
            unresolvable: Unresolvable::Deregistered,
            states_now: &["ASSIGNING", "ASSIGNED"],
            settled_state: "ASSIGNED",
        },
        Row {
            case: "a request that also renames the group",
            nodes: &[0],
            settle_first: true,
            also_rename: true,
            unresolvable: Unresolvable::Unminted,
            states_now: &["ASSIGNED"],
            settled_state: "ASSIGNED",
        },
    ];

    for row in rows {
        let Row {
            case,
            nodes,
            settle_first,
            also_rename,
            unresolvable,
            states_now,
            settled_state,
        } = row;
        let router = medialive();
        let cluster = create_cluster(&router, case, "primary").await;
        // Two nodes of the group's own cluster: the one a row may be associated
        // with, and the one whose deregistration a row names.
        let minted = create_nodes(&router, case, &cluster, 2).await;
        let elsewhere = create_cluster(&router, case, "secondary").await;
        let far = create_nodes(&router, case, &elsewhere, 1).await.remove(0);
        deregister_node(&router, case, &cluster, &minted[1]).await;

        let unresolvable = match unresolvable {
            // An id nothing holds: `Id` on a node is "the resource-id portion of
            // the ARN" and the model constrains it no further, so what makes
            // this one unresolvable is only that no `CreateNode` minted it.
            Unresolvable::Unminted => "9999999".to_string(),
            Unresolvable::NodeOfAnotherCluster => far.clone(),
            Unresolvable::Deregistered => minted[1].clone(),
        };
        let start_nodes: Vec<String> = nodes.iter().map(|index| minted[*index].clone()).collect();
        let group = created(
            &router,
            case,
            &cluster,
            json!({ "name": GROUP_NAME, "nodes": start_nodes }),
        )
        .await;
        let id = id_of(&group);
        if settle_first {
            wait_for_the_clock();
        }
        let before = described(&router, case, &cluster, &id).await;

        let mut request = json!({ "nodes": [unresolvable.clone()] });
        if also_rename {
            request["name"] = json!(RENAME_TO);
        }
        let (status, body) = update_group(&router, &cluster, &id, request).await;

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
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(&unresolvable) && message.contains(&cluster),
            "case {case:?}: expected the refusal to name the id it could not resolve \
             ({unresolvable:?}) and the cluster it was resolved against ({cluster:?}), \
             got {message:?}"
        );

        // Nothing the request carried reached the store.
        let after = described(&router, case, &cluster, &id).await;
        assert_eq!(
            without_state(&after),
            without_state(&before),
            "case {case:?}: expected the refused update to change nothing, got {after}"
        );
        // Nor did the refusal move the group's own lifecycle: it is still in the
        // state it was in, which for a row whose association is in flight is
        // either of that association's two states and for every other row is the
        // one state it had settled in.
        let state_now = after["state"].as_str().unwrap_or_default().to_string();
        assert!(
            states_now.contains(&state_now.as_str()),
            "case {case:?}: expected the group to still be one of {states_now:?} right after \
             the refusal, got {state_now:?}"
        );

        // And no transition was started by it: once the clock has run the group
        // is in the state its OWN node set implies, still associated with the
        // nodes it had and still carrying the name it had.
        wait_for_the_clock();
        let settled = described(&router, case, &cluster, &id).await;
        assert_eq!(
            settled["state"],
            json!(settled_state),
            "case {case:?}: expected the group to be {settled_state} after the clock ran, got {:?}",
            settled["state"]
        );
        assert_eq!(
            settled["nodes"],
            json!(start_nodes),
            "case {case:?}: expected the group to keep its own nodes {start_nodes:?}, got {:?}",
            settled["nodes"]
        );
        assert_eq!(
            settled["name"],
            json!(GROUP_NAME),
            "case {case:?}: expected the group to keep the name {GROUP_NAME:?}, got {:?}",
            settled["name"]
        );
    }
}

/// Rule, the row the table above cannot carry: a group goes on reporting a node
/// whose deregistration was accepted AFTER the association was made (the delete
/// did not change that stored state), but an update that re-sends that very id
/// is a NEW association with a node there is nothing left to associate, so it is
/// refused like any other id that does not resolve.
#[tokio::test]
async fn re_sending_a_node_that_has_since_been_deregistered_is_refused() {
    let case = "a node deregistered after it was associated";
    let router = medialive();
    let cluster = create_cluster(&router, case, "primary").await;
    let node = create_nodes(&router, case, &cluster, 1).await.remove(0);
    let group = created(
        &router,
        case,
        &cluster,
        json!({ "name": GROUP_NAME, "nodes": [node.clone()] }),
    )
    .await;
    let id = id_of(&group);
    wait_for_the_clock();
    deregister_node(&router, case, &cluster, &node).await;
    let before = described(&router, case, &cluster, &id).await;
    assert_eq!(
        before["nodes"],
        json!([node.clone()]),
        "case {case:?}: expected the group to still report the node it was associated with, \
         got {:?}",
        before["nodes"]
    );

    let (status, body) =
        update_group(&router, &cluster, &id, json!({ "nodes": [node.clone()] })).await;

    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body}"
    );
    let after = described(&router, case, &cluster, &id).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the refused update to change nothing, got {after}"
    );
}

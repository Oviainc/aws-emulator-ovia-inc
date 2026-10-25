// SPDX-License-Identifier: CC0-1.0

//! Deleting a member is ASYNCHRONOUS, end to end over HTTP: the call starts the removal
//! and the records it removes stay readable and listable, reporting the `DELETING` of
//! their status enum until the clock owes them `DELETED`.
//!
//! The rule under test: a delete of a resource whose status enum declares `DELETING` and
//! `DELETED` starts the removal rather than completing it, so every read and listing of
//! that resource — and of the resources deleted with it — answers from the stored record
//! and reports the stage its status names. `MemberStatus`, `NodeStatus` and
//! `NetworkStatus` each declare both values, and `ListMembersInput$Status` /
//! `ListNodesInput$Status` / `ListNetworksInput$Status` accept them as filters.
//!
//! The tables below span the classes the rule generalises over: how many members the
//! network holds and which of them is deleted (first, middle, last, sole), how many nodes
//! the deleted member owns (none, one, several), and member counts from one to three for
//! the network that loses its last participating member.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, delete_member, get_member, get_network, get_node, listed_ids, listed_member_ids,
    network_with_members, PAST_TRANSITION,
};
use crate::network_and_node_lifecycle::create_node;

/// A node configuration every Fabric member can take.
fn node_configuration() -> Value {
    serde_json::json!({ "InstanceType": "bc.t3.small" })
}

/// The node ids `ListNodes` reports for one member of `network_id` under `query`.
async fn listed_nodes(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    query: &str,
) -> Vec<String> {
    let path = format!("/networks/{network_id}/nodes?memberId={member_id}&{query}");
    listed_ids(&call(router, "GET", &path, None).await, "Nodes")
}

/// The network ids `ListNetworks` reports under `query`.
async fn listed_networks(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    listed_ids(
        &call(router, "GET", &format!("/networks?{query}"), None).await,
        "Networks",
    )
}

/// `DeleteMember` starts the deletion: the member and the nodes deleted with it stay
/// readable, report `DELETING`, are reported by the listings filtered on that status, and
/// are owed `DELETED` on the clock — while every other member and node is untouched.
#[tokio::test]
async fn delete_member_reports_deleting_then_the_clock_owes_deleted() {
    // Rows: how many members the network holds, which of them is deleted (so: sole,
    // first, middle and last in the stored ordering), and how many nodes that member
    // owns (none, one, several).
    let cases: &[(&str, usize, usize, usize)] = &[
        ("sole member, no node", 0, 0, 0),
        ("sole member, one node", 0, 0, 1),
        ("first of three, two nodes", 2, 0, 2),
        ("middle of three, no node", 2, 1, 0),
        ("last of three, one node", 2, 2, 1),
    ];

    for &(case, extra, position, node_count) in cases {
        let (router, network_id, mut members) =
            network_with_members(
                &format!("deleting-{extra}-{position}-{node_count}"),
                "deletion-net",
                extra,
            )
            .await;
        let doomed = members.remove(position);
        let survivors = members;

        let mut doomed_nodes = Vec::new();
        for index in 0..node_count {
            doomed_nodes.push(
                create_node(
                    &router,
                    &network_id,
                    &doomed,
                    &format!("doomed-{position}-{index}"),
                    node_configuration(),
                )
                .await,
            );
        }
        let survivor_node = match survivors.first() {
            Some(survivor) => Some((
                survivor.clone(),
                create_node(
                    &router,
                    &network_id,
                    survivor,
                    &format!("survivor-{position}"),
                    node_configuration(),
                )
                .await,
            )),
            None => None,
        };
        tokio::time::sleep(PAST_TRANSITION).await;

        let answer = delete_member(&router, &network_id, &doomed).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteMember 200, got {} {:?}",
            answer.status, answer.body
        );

        // The member is still there, reporting the stage its status names.
        let read = get_member(&router, &network_id, &doomed).await;
        assert_eq!(
            (read.status, read.text(&["Member", "Status"]).as_str()),
            (200, "DELETING"),
            "case {case:?}: expected GetMember 200 DELETING right after the delete, got {} {:?}",
            read.status,
            read.body
        );
        // And so are "all associated resources" it is deleted with.
        for node_id in &doomed_nodes {
            let read = get_node(&router, &network_id, &doomed, node_id).await;
            assert_eq!(
                (read.status, read.text(&["Node", "Status"]).as_str()),
                (200, "DELETING"),
                "case {case:?}: expected the deleted member's node {node_id} 200 DELETING, got \
                 {} {:?}",
                read.status,
                read.body
            );
        }

        // The listings report it, and the status filters select on the stored value.
        let listed = listed_member_ids(&router, &network_id, "").await;
        assert!(
            listed.contains(&doomed),
            "case {case:?}: expected ListMembers to keep reporting {doomed}, got {listed:?}"
        );
        assert_eq!(
            listed_member_ids(&router, &network_id, "status=DELETING").await,
            vec![doomed.clone()],
            "case {case:?}: expected ListMembers?status=DELETING to report only {doomed}"
        );
        assert_eq!(
            listed_member_ids(&router, &network_id, "status=AVAILABLE").await,
            survivors,
            "case {case:?}: expected ListMembers?status=AVAILABLE to report only the survivors"
        );
        assert_eq!(
            listed_member_ids(&router, &network_id, "status=DELETED").await,
            Vec::<String>::new(),
            "case {case:?}: expected no member DELETED before the deletion came due"
        );
        assert_eq!(
            {
                let mut listed =
                    listed_nodes(&router, &network_id, &doomed, "status=DELETING").await;
                listed.sort();
                listed
            },
            {
                let mut expected = doomed_nodes.clone();
                expected.sort();
                expected
            },
            "case {case:?}: expected ListNodes?status=DELETING to report the member's nodes"
        );

        // Nothing else on the network moved.
        for survivor in &survivors {
            let read = get_member(&router, &network_id, survivor).await;
            assert_eq!(
                (read.status, read.text(&["Member", "Status"]).as_str()),
                (200, "AVAILABLE"),
                "case {case:?}: expected the surviving member {survivor} untouched, got {} {:?}",
                read.status,
                read.body
            );
        }
        if let Some((survivor, node_id)) = survivor_node.as_ref() {
            let read = get_node(&router, &network_id, survivor, node_id).await;
            assert_eq!(
                (read.status, read.text(&["Node", "Status"]).as_str()),
                (200, "AVAILABLE"),
                "case {case:?}: expected another member's node untouched, got {} {:?}",
                read.status,
                read.body
            );
        }

        // The network goes the same way exactly when it has lost its last participating
        // member.
        let emptied = survivors.is_empty();
        let expected_network = if emptied { "DELETING" } else { "AVAILABLE" };
        let read = get_network(&router, &network_id).await;
        assert_eq!(
            (read.status, read.text(&["Network", "Status"]).as_str()),
            (200, expected_network),
            "case {case:?}: expected GetNetwork 200 {expected_network}, got {} {:?}",
            read.status,
            read.body
        );

        // Past the deletion span the clock owes every one of them `DELETED`.
        tokio::time::sleep(PAST_TRANSITION).await;
        let read = get_member(&router, &network_id, &doomed).await;
        assert_eq!(
            (read.status, read.text(&["Member", "Status"]).as_str()),
            (200, "DELETED"),
            "case {case:?}: expected GetMember 200 DELETED once the deletion came due, got {} {:?}",
            read.status,
            read.body
        );
        for node_id in &doomed_nodes {
            let read = get_node(&router, &network_id, &doomed, node_id).await;
            assert_eq!(
                (read.status, read.text(&["Node", "Status"]).as_str()),
                (200, "DELETED"),
                "case {case:?}: expected node {node_id} 200 DELETED once due, got {} {:?}",
                read.status,
                read.body
            );
        }
        assert_eq!(
            listed_member_ids(&router, &network_id, "status=DELETED").await,
            vec![doomed.clone()],
            "case {case:?}: expected ListMembers?status=DELETED to report {doomed}"
        );
        let expected_network = if emptied { "DELETED" } else { "AVAILABLE" };
        let read = get_network(&router, &network_id).await;
        assert_eq!(
            (read.status, read.text(&["Network", "Status"]).as_str()),
            (200, expected_network),
            "case {case:?}: expected GetNetwork 200 {expected_network} once due, got {} {:?}",
            read.status,
            read.body
        );

        // `DELETED` is terminal: reading it changes nothing, and a repeated delete starts
        // no second deletion — it never re-arms the transition.
        let again = delete_member(&router, &network_id, &doomed).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected a repeated DeleteMember 200, got {} {:?}",
            again.status, again.body
        );
        let read = get_member(&router, &network_id, &doomed).await;
        assert_eq!(
            (read.status, read.text(&["Member", "Status"]).as_str()),
            (200, "DELETED"),
            "case {case:?}: expected the member to stay DELETED, got {} {:?}",
            read.status,
            read.body
        );
    }
}

/// "If MemberId is the last member in a network specified by the last Amazon Web Services
/// account, the network is deleted also": the network stands while a participating member
/// is left, and then reports the `DELETING` and `DELETED` of `NetworkStatus` rather than
/// vanishing.
#[tokio::test]
async fn the_network_follows_its_last_member_through_deleting_to_deleted() {
    // Rows: member counts from one (the founding member alone) to three, each emptied one
    // member at a time.
    for extra in 0..3_usize {
        let case = format!("{} member(s)", extra + 1);
        let (router, network_id, members) =
            network_with_members(&format!("network-deleting-{extra}"), "deletion-net", extra)
                .await;

        for (position, doomed) in members.iter().enumerate() {
            let last = position + 1 == members.len();
            let answer = delete_member(&router, &network_id, doomed).await;
            assert_eq!(
                answer.status, 200,
                "case {case}/{position}: expected DeleteMember 200, got {} {:?}",
                answer.status, answer.body
            );

            let expected = if last { "DELETING" } else { "AVAILABLE" };
            let read = get_network(&router, &network_id).await;
            assert_eq!(
                (read.status, read.text(&["Network", "Status"]).as_str()),
                (200, expected),
                "case {case}/{position}: expected GetNetwork 200 {expected} after deleting \
                 {doomed}, got {} {:?}",
                read.status,
                read.body
            );
        }

        // The listing reports the network under the status it holds, both before and after
        // the deletion comes due.
        let listed = listed_networks(&router, "status=DELETING").await;
        assert!(
            listed.contains(&network_id),
            "case {case}: expected ListNetworks?status=DELETING to report {network_id}, got \
             {listed:?}"
        );
        tokio::time::sleep(PAST_TRANSITION).await;
        let read = get_network(&router, &network_id).await;
        assert_eq!(
            (read.status, read.text(&["Network", "Status"]).as_str()),
            (200, "DELETED"),
            "case {case}: expected GetNetwork 200 DELETED once the deletion came due, got {} {:?}",
            read.status,
            read.body
        );
        let listed = listed_networks(&router, "status=DELETED").await;
        assert!(
            listed.contains(&network_id),
            "case {case}: expected ListNetworks?status=DELETED to report {network_id}, got \
             {listed:?}"
        );
    }
}

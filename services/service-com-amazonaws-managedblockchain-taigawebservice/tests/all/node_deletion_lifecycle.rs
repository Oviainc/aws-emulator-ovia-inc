// SPDX-License-Identifier: CC0-1.0

//! Deleting a node is ASYNCHRONOUS, end to end over HTTP: the call starts the removal and
//! the node stays readable and listable, reporting the `DELETING` of `NodeStatus` until the
//! clock owes it `DELETED`.
//!
//! The rule under test: a delete of a resource whose status enum declares `DELETING` and
//! `DELETED` starts the removal rather than completing it, so every read and listing of
//! that resource answers from the stored record and reports the stage its status names —
//! and the only status such a delete cannot be completed from is the one whose creation has
//! not finished. `NodeStatus` declares both deletion values and `ListNodesInput$Status`
//! accepts them as filters, which is what makes them observable.
//!
//! The tables below span the classes the rule generalises over: how many nodes the member
//! holds and which of them is deleted (sole, first, middle, last), and every value
//! `NodeStatus` declares as the status the delete is asked to start from.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::{
    ComAmazonawsManagedblockchainTaigawebserviceState as ServiceState, StoredNode,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    available_network, call, delete_node, get_member, get_network, get_node, listed_ids,
    router_and_engine, ACCOUNT, PAST_TRANSITION, REGION,
};
use crate::network_and_node_lifecycle::create_node;

/// Long enough that a transition seeded with this much left to run has not come due by the
/// end of a test.
const FAR_FUTURE: Duration = Duration::from_secs(3_600);

/// A node configuration every Fabric member can take.
fn node_configuration() -> Value {
    json!({ "InstanceType": "bc.t3.small" })
}

/// The node ids `ListNodes` reports for one member of `network_id` under `query` (empty for
/// none), in the order it reported them.
async fn listed_nodes(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    query: &str,
) -> Vec<String> {
    let mut path = format!("/networks/{network_id}/nodes?memberId={member_id}");
    if !query.is_empty() {
        path.push('&');
        path.push_str(query);
    }
    listed_ids(&call(router, "GET", &path, None).await, "Nodes")
}

/// The `Status` `GetNode` reports for `node_id`, with the HTTP status it answered.
async fn node_status(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
) -> (u16, String) {
    let answer = get_node(router, network_id, member_id, node_id).await;
    (answer.status, answer.text(&["Node", "Status"]))
}

/// Write one node per entry of `statuses` into the store as nodes of `network_id`, and
/// return their ids in the order the store holds them.
///
/// Each seeded record is a copy of the one `CreateNode` stored, with only its id, ARN,
/// status and the instants of its pending transitions changed — so a case tests the
/// handler, not a hand-built record no create could have produced. A `CREATING` seed keeps
/// that status for the whole test (its `AVAILABLE` is not due yet) and a `DELETING` one
/// likewise; every other value is one `settle` never moves.
fn seed_nodes(
    engine: &engine::InMemoryEngine,
    network_id: &str,
    statuses: &[&str],
) -> Vec<String> {
    let state = engine.state::<ServiceState>();
    let mut guard = state.lock();
    let network = guard
        .networks
        .get_mut(network_id)
        .expect("the created network is stored");
    let template = network
        .nodes
        .values()
        .next()
        .expect("CreateNode stored a node to copy")
        .clone();
    let now = SystemTime::now();
    let mut ids = Vec::new();
    for (index, status) in statuses.iter().enumerate() {
        // Zero-padded, so the id ordering the store pages on is the row ordering, and
        // within `ResourceIdString`'s `@length(1, 32)`.
        let id = format!("nd-SEED{index:021}");
        let node = StoredNode {
            id: id.clone(),
            status: (*status).to_string(),
            available_at: now + FAR_FUTURE,
            deleted_at: match *status {
                "DELETING" => Some(now + FAR_FUTURE),
                "DELETED" => Some(now),
                _ => None,
            },
            arn: format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:nodes/{id}"),
            ..template.clone()
        };
        network.nodes.insert(id.clone(), node);
        ids.push(id);
    }
    ids
}

/// `DeleteNode` starts the deletion: the node stays readable, reports `DELETING`, is
/// reported by the listing filtered on that status, and is owed `DELETED` on the clock —
/// while every other node, the member that owns it and the network are untouched.
#[tokio::test]
async fn delete_node_reports_deleting_then_the_clock_owes_deleted() {
    // Rows: how many nodes the member holds and which of them is deleted — so: the sole
    // node, and the first, middle and last of three in the stored ordering a page cursor
    // keys on.
    let cases: &[(&str, usize, usize)] = &[
        ("sole node", 1, 0),
        ("first of three", 3, 0),
        ("middle of three", 3, 1),
        ("last of three", 3, 2),
    ];

    for &(case, count, position) in cases {
        let (router, _engine) = router_and_engine();
        let (network_id, member_id) = available_network(
            &router,
            &format!("node-deleting-{count}-{position}"),
            "node-deletion-net",
        )
        .await;

        let mut node_ids: Vec<String> = Vec::new();
        for index in 0..count {
            node_ids.push(
                create_node(
                    &router,
                    &network_id,
                    &member_id,
                    &format!("node-{count}-{position}-{index}"),
                    node_configuration(),
                )
                .await,
            );
        }
        node_ids.sort();
        let all = node_ids.clone();
        let doomed = node_ids.remove(position);
        let survivors = node_ids;
        tokio::time::sleep(PAST_TRANSITION).await;

        let answer = delete_node(&router, &network_id, &member_id, &doomed).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteNode 200, got {} {:?}",
            answer.status, answer.body
        );

        // The node is still there, reporting the stage its status names.
        let read = node_status(&router, &network_id, &member_id, &doomed).await;
        assert_eq!(
            (read.0, read.1.as_str()),
            (200, "DELETING"),
            "case {case:?}: expected GetNode 200 DELETING right after the delete, got {read:?}"
        );

        // The listing reports it, and the status filter selects on the stored value.
        assert_eq!(
            listed_nodes(&router, &network_id, &member_id, "").await,
            all,
            "case {case:?}: expected ListNodes to keep reporting every node, {doomed} included"
        );
        assert_eq!(
            listed_nodes(&router, &network_id, &member_id, "status=DELETING").await,
            vec![doomed.clone()],
            "case {case:?}: expected ListNodes?status=DELETING to report only {doomed}"
        );
        assert_eq!(
            listed_nodes(&router, &network_id, &member_id, "status=AVAILABLE").await,
            survivors,
            "case {case:?}: expected ListNodes?status=AVAILABLE to report only the survivors"
        );
        assert_eq!(
            listed_nodes(&router, &network_id, &member_id, "status=DELETED").await,
            Vec::<String>::new(),
            "case {case:?}: expected no node DELETED before the deletion came due"
        );

        // Nothing else moved: a node is deleted on its own, so the member that owns it and
        // the network it runs on stand.
        for survivor in &survivors {
            let read = node_status(&router, &network_id, &member_id, survivor).await;
            assert_eq!(
                (read.0, read.1.as_str()),
                (200, "AVAILABLE"),
                "case {case:?}: expected the surviving node {survivor} untouched, got {read:?}"
            );
        }
        let read = get_member(&router, &network_id, &member_id).await;
        assert_eq!(
            (read.status, read.text(&["Member", "Status"]).as_str()),
            (200, "AVAILABLE"),
            "case {case:?}: expected the owning member untouched, got {} {:?}",
            read.status,
            read.body
        );
        let read = get_network(&router, &network_id).await;
        assert_eq!(
            (read.status, read.text(&["Network", "Status"]).as_str()),
            (200, "AVAILABLE"),
            "case {case:?}: expected the network untouched, got {} {:?}",
            read.status,
            read.body
        );

        // Past the deletion span the clock owes the node `DELETED`, and the listing reports
        // it under that status.
        tokio::time::sleep(PAST_TRANSITION).await;
        let read = node_status(&router, &network_id, &member_id, &doomed).await;
        assert_eq!(
            (read.0, read.1.as_str()),
            (200, "DELETED"),
            "case {case:?}: expected GetNode 200 DELETED once the deletion came due, got \
             {read:?}"
        );
        assert_eq!(
            listed_nodes(&router, &network_id, &member_id, "status=DELETED").await,
            vec![doomed.clone()],
            "case {case:?}: expected ListNodes?status=DELETED to report {doomed}"
        );
        assert_eq!(
            listed_nodes(&router, &network_id, &member_id, "status=DELETING").await,
            Vec::<String>::new(),
            "case {case:?}: expected no node left DELETING once the deletion came due"
        );

        // `DELETED` is terminal: reading it changes nothing, and a repeated delete starts
        // no second deletion — it never re-arms the transition.
        let again = delete_node(&router, &network_id, &member_id, &doomed).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected a repeated DeleteNode 200, got {} {:?}",
            again.status, again.body
        );
        let read = node_status(&router, &network_id, &member_id, &doomed).await;
        assert_eq!(
            (read.0, read.1.as_str()),
            (200, "DELETED"),
            "case {case:?}: expected the node to stay DELETED, got {read:?}"
        );
    }
}

/// Only a node whose creation has not finished is refused as not ready: every settled
/// `NodeStatus` can be deleted, and the delete leaves the record readable in the stage its
/// status names.
#[tokio::test]
async fn a_node_still_creating_cannot_be_deleted_yet() {
    // Rows: every value `NodeStatus` declares (smithy_query: shape NodeStatus = CREATING |
    // AVAILABLE | UNHEALTHY | CREATE_FAILED | UPDATING | DELETING | DELETED | FAILED |
    // INACCESSIBLE_ENCRYPTION_KEY). `CREATING` is "the process of creating a node"
    // (`Node$Status`) and is the one status the delete cannot complete from; `FAILED` is
    // documented "must be deleted" and `INACCESSIBLE_ENCRYPTION_KEY` "we recommend deleting
    // and recreating the resource", so those are deletable like the rest.
    let cases: &[(&str, u16, &str)] = &[
        ("CREATING", 409, "CREATING"),
        ("AVAILABLE", 200, "DELETING"),
        ("UNHEALTHY", 200, "DELETING"),
        ("CREATE_FAILED", 200, "DELETING"),
        ("UPDATING", 200, "DELETING"),
        ("DELETING", 200, "DELETING"),
        ("DELETED", 200, "DELETED"),
        ("FAILED", 200, "DELETING"),
        ("INACCESSIBLE_ENCRYPTION_KEY", 200, "DELETING"),
    ];

    let (router, engine) = router_and_engine();
    let (network_id, member_id) =
        available_network(&router, "node-statuses", "node-status-net").await;
    // The record every seed is copied from, so each row drives the handler over a node a
    // create really stored.
    let template = create_node(
        &router,
        &network_id,
        &member_id,
        "node-status-template",
        node_configuration(),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let statuses: Vec<&str> = cases.iter().map(|(status, _, _)| *status).collect();
    let seeded = seed_nodes(&engine, &network_id, &statuses);

    for (&(status, expected, expected_after), node_id) in cases.iter().zip(&seeded) {
        let answer = delete_node(&router, &network_id, &member_id, node_id).await;
        assert_eq!(
            answer.status, expected,
            "case {status}: expected DeleteNode {expected}, got {} {:?}",
            answer.status, answer.body
        );
        if expected != 200 {
            assert_eq!(
                answer.error_type, "ResourceNotReadyException",
                "case {status}: expected the modelled not-ready error, got {:?} {:?}",
                answer.error_type, answer.body
            );
        }
        let read = node_status(&router, &network_id, &member_id, node_id).await;
        assert_eq!(
            (read.0, read.1.as_str()),
            (200, expected_after),
            "case {status}: expected the node to read back as {expected_after}, got {read:?}"
        );
    }

    // The refusals and the deletes above reached only the node each named: the template is
    // the one node still `AVAILABLE`.
    assert_eq!(
        listed_nodes(&router, &network_id, &member_id, "status=AVAILABLE").await,
        vec![template.clone()],
        "case after the table: expected only the untouched template node AVAILABLE"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! The status window `UpdateNode` opens on a node, end to end over the router.
//!
//! An update STARTS the configuration change rather than completing it, so the node it
//! writes reports `UPDATING` ("The node is in the process of being updated" —
//! `Node$Status`) until the clock owes it `AVAILABLE` again, and `ListNodes` selects on
//! that same stored value (`ListNodesInput$Status`, bound as the `status` `httpQuery`,
//! accepts `UPDATING`). The status values and the member names come from `smithy_query`
//! (`Node$Status`, `ListNodesInput$Status`, `NodeLogPublishingConfiguration` -> `Fabric`,
//! `NodeFabricLogPublishingConfiguration` -> `ChaincodeLogs` / `PeerLogs`,
//! `LogConfigurations` -> `Cloudwatch`, `LogConfiguration` -> `Enabled`).
//!
//! The tables below span the class the rule generalises over on the node side: WHICH log
//! streams successive updates name (so each update re-opens the window the previous one's
//! settle closed), two updates overlapping inside one window, HOW MANY of a member's
//! nodes are being updated at once (the listing filter partitions them), and the status
//! the node was in when the update arrived (a creation still running, a removal already
//! under way — neither of which an update takes the node out of).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    available_network, call, delete_node, get_node, listed_ids, new_router, Answer,
    PAST_TRANSITION,
};
use crate::network_and_node_lifecycle::create_node;

/// The `NodeStatus` value a node in service reports.
const AVAILABLE: &str = "AVAILABLE";

/// The `NodeStatus` value a node reports while a configuration change it is in runs.
const UPDATING: &str = "UPDATING";

/// The `NodeStatus` value a node whose creation is still running reports.
const CREATING: &str = "CREATING";

/// The `NodeStatus` value a node whose removal has begun reports.
const DELETING: &str = "DELETING";

/// The instance type every node in these cases runs on (one the `STANDARD` edition
/// offers).
const INSTANCE_TYPE: &str = "bc.t3.small";

/// A `NodeLogPublishingConfiguration` naming each log stream whose `Enabled` is given,
/// and leaving out the one that is `None`.
fn node_logs(peer: Option<bool>, chaincode: Option<bool>) -> Value {
    let mut fabric = json!({});
    if let Some(enabled) = peer {
        fabric["PeerLogs"] = json!({ "Cloudwatch": { "Enabled": enabled } });
    }
    if let Some(enabled) = chaincode {
        fabric["ChaincodeLogs"] = json!({ "Cloudwatch": { "Enabled": enabled } });
    }
    json!({ "Fabric": fabric })
}

/// One `UpdateNode` call: `PATCH /networks/{NetworkId}/nodes/{NodeId}`, scoped by the
/// member that owns the node (`UpdateNodeInput$MemberId` rides in the body).
async fn update_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
    patch: &Value,
) -> Answer {
    let body = json!({ "MemberId": member_id, "LogPublishingConfiguration": patch });
    call(
        router,
        "PATCH",
        &format!("/networks/{network_id}/nodes/{node_id}"),
        Some(body),
    )
    .await
}

/// The `Status` `GetNode` reports for one node.
async fn node_status(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
) -> String {
    let answer = get_node(router, network_id, member_id, node_id).await;
    assert_eq!(
        answer.status, 200,
        "case GetNode {node_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["Node", "Status"])
}

/// The `LogPublishingConfiguration` `GetNode` reports for one node.
async fn node_logs_stored(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
) -> Value {
    let answer = get_node(router, network_id, member_id, node_id).await;
    answer.body["Node"]["LogPublishingConfiguration"].clone()
}

/// The node ids `ListNodes` reports for one member of `network_id` in `status`, sorted so
/// a case compares a SET rather than the listing's paging order.
async fn listed_node_ids(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    status: &str,
) -> Vec<String> {
    let answer = call(
        router,
        "GET",
        &format!("/networks/{network_id}/nodes?memberId={member_id}&status={status}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case ListNodes status={status}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let mut ids = listed_ids(&answer, "Nodes");
    ids.sort();
    ids
}

/// Create one node and wait for the clock to settle its creation, so the update window is
/// the one an update opens on a node already in service.
async fn available_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
) -> String {
    let node_id = create_node(
        router,
        network_id,
        member_id,
        token,
        json!({ "InstanceType": INSTANCE_TYPE }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    node_id
}

/// Every update opens the window again, whichever log streams it names: the node reports
/// `UPDATING` straight after the call, the listing filter reports it there, and the clock
/// settles it back to `AVAILABLE` carrying the parameters that update named.
#[tokio::test]
async fn every_update_re_opens_the_node_status_window() {
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "node-window-steps", "net-node-window").await;
    let node_id = available_node(&router, &network_id, &member_id, "node-window-steps-node").await;

    // Rows: which stream each update names, and the configuration the node carries once
    // that update has settled (the streams the request leaves out keep what they had).
    let steps: Vec<(&str, Value, Value)> = vec![
        (
            "the peer log stream switched on",
            node_logs(Some(true), None),
            node_logs(Some(true), None),
        ),
        (
            "the chaincode log stream switched on",
            node_logs(None, Some(true)),
            node_logs(Some(true), Some(true)),
        ),
        (
            "the peer log stream switched back off",
            node_logs(Some(false), None),
            node_logs(Some(false), Some(true)),
        ),
        (
            "both log streams switched off",
            node_logs(Some(false), Some(false)),
            node_logs(Some(false), Some(false)),
        ),
    ];

    for (case, patch, settled) in &steps {
        let before = node_status(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            before, AVAILABLE,
            "case {case}: expected the node to be {AVAILABLE} before the update, got \
             {before:?}"
        );

        let answer = update_node(&router, &network_id, &member_id, &node_id, patch).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateNode 200, got {} {:?}",
            answer.status, answer.body
        );

        // Nothing slept: the window is non-zero, so a caller reading the node straight
        // after the call still observes the change under way.
        let during = node_status(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            during, UPDATING,
            "case {case}: expected the node to be {UPDATING} straight after the update, \
             got {during:?}"
        );
        let updating = listed_node_ids(&router, &network_id, &member_id, UPDATING).await;
        assert_eq!(
            updating,
            vec![node_id.clone()],
            "case {case}: expected ListNodes status={UPDATING} to report the node being \
             updated, got {updating:?}"
        );
        let available = listed_node_ids(&router, &network_id, &member_id, AVAILABLE).await;
        assert!(
            available.is_empty(),
            "case {case}: expected ListNodes status={AVAILABLE} to report nothing while \
             the change runs, got {available:?}"
        );

        // The change settles on its own: the clock owes the node `AVAILABLE` again.
        tokio::time::sleep(PAST_TRANSITION).await;
        let after = node_status(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            after, AVAILABLE,
            "case {case}: expected the node to settle back to {AVAILABLE}, got {after:?}"
        );
        let stored = node_logs_stored(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            stored, *settled,
            "case {case}: expected the settled node to carry {settled:?}, got {stored:?}"
        );
    }
}

/// An update that arrives while the previous one's window is still open keeps the node
/// `UPDATING` and applies its own parameters: the window runs from the call that asked
/// for it, so the later update's parameters are the ones the settled node carries.
#[tokio::test]
async fn a_second_update_inside_the_node_window_keeps_it_open() {
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "node-window-overlap", "net-node-overlap").await;
    let node_id =
        available_node(&router, &network_id, &member_id, "node-window-overlap-node").await;

    // Rows: the second update names the opposite setting of the first, so what the node
    // carries afterwards says which call's parameters won.
    let steps: Vec<(&str, Value)> = vec![
        ("the first update", node_logs(Some(true), Some(true))),
        (
            "a second update inside the window",
            node_logs(Some(false), None),
        ),
    ];
    for (case, patch) in &steps {
        let answer = update_node(&router, &network_id, &member_id, &node_id, patch).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateNode 200, got {} {:?}",
            answer.status, answer.body
        );
        let during = node_status(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            during, UPDATING,
            "case {case}: expected the node to be {UPDATING} while the change runs, got \
             {during:?}"
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;
    let after = node_status(&router, &network_id, &member_id, &node_id).await;
    assert_eq!(
        after, AVAILABLE,
        "case after both updates: expected the node to settle back to {AVAILABLE}, got \
         {after:?}"
    );
    let stored = node_logs_stored(&router, &network_id, &member_id, &node_id).await;
    let expected = node_logs(Some(false), Some(true));
    assert_eq!(
        stored, expected,
        "case after both updates: expected the node to carry {expected:?}, got {stored:?}"
    );
}

/// The listing filter partitions a member's nodes by the status each one is stored in, for
/// any number of them being updated at once.
#[tokio::test]
async fn the_node_status_filter_reports_every_node_being_updated() {
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "node-window-many", "net-node-many").await;
    let mut nodes = Vec::new();
    for index in 0..3 {
        nodes.push(
            create_node(
                &router,
                &network_id,
                &member_id,
                &format!("node-window-many-{index}"),
                json!({ "InstanceType": INSTANCE_TYPE }),
            )
            .await,
        );
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    // Rows: HOW MANY of the member's three nodes are updated in one window — none, one,
    // two, all of them. Each row's window is opened fresh, after the previous row settled.
    for updated in 0..=nodes.len() {
        let case = format!("{updated} of {} nodes updated", nodes.len());
        for node_id in &nodes[..updated] {
            let answer = update_node(
                &router,
                &network_id,
                &member_id,
                node_id,
                &node_logs(Some(true), None),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected UpdateNode 200 for {node_id}, got {} {:?}",
                answer.status, answer.body
            );
        }

        let mut expected_updating: Vec<String> = nodes[..updated].to_vec();
        expected_updating.sort();
        let mut expected_available: Vec<String> = nodes[updated..].to_vec();
        expected_available.sort();

        let listed_updating = listed_node_ids(&router, &network_id, &member_id, UPDATING).await;
        assert_eq!(
            listed_updating, expected_updating,
            "case {case}: expected ListNodes status={UPDATING} to report \
             {expected_updating:?}, got {listed_updating:?}"
        );
        let listed_available = listed_node_ids(&router, &network_id, &member_id, AVAILABLE).await;
        assert_eq!(
            listed_available, expected_available,
            "case {case}: expected ListNodes status={AVAILABLE} to report \
             {expected_available:?}, got {listed_available:?}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;
        let settled = listed_node_ids(&router, &network_id, &member_id, AVAILABLE).await;
        let mut all = nodes.clone();
        all.sort();
        assert_eq!(
            settled, all,
            "case {case}: expected every node to settle back to {AVAILABLE}, got \
             {settled:?}"
        );
    }
}

/// An update does not take a node out of a transition it is already in: a creation still
/// running keeps reporting `CREATING` and goes on to `AVAILABLE`, and a removal already
/// under way keeps reporting `DELETING` — the update window belongs to a node that is in
/// service. The parameters the update named are applied either way.
#[tokio::test]
async fn an_update_does_not_disturb_a_node_transition_already_under_way() {
    // A node whose creation is still running.
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "node-window-creating", "net-node-creating").await;
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "node-window-creating-node",
        json!({ "InstanceType": INSTANCE_TYPE }),
    )
    .await;
    let patch = node_logs(Some(true), None);
    let answer = update_node(&router, &network_id, &member_id, &node_id, &patch).await;
    assert_eq!(
        answer.status, 200,
        "case a node still creating: expected UpdateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    let during = node_status(&router, &network_id, &member_id, &node_id).await;
    assert_eq!(
        during, CREATING,
        "case a node still creating: expected the creation to keep reporting {CREATING}, \
         got {during:?}"
    );
    let updating = listed_node_ids(&router, &network_id, &member_id, UPDATING).await;
    assert!(
        updating.is_empty(),
        "case a node still creating: expected ListNodes status={UPDATING} to report \
         nothing, got {updating:?}"
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    let after = node_status(&router, &network_id, &member_id, &node_id).await;
    assert_eq!(
        after, AVAILABLE,
        "case a node still creating: expected the creation to settle to {AVAILABLE}, got \
         {after:?}"
    );
    let stored = node_logs_stored(&router, &network_id, &member_id, &node_id).await;
    assert_eq!(
        stored, patch,
        "case a node still creating: expected the update's parameters to be applied, got \
         {stored:?}"
    );

    // A node whose removal has begun: nothing brings it back out of `DELETING`.
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "node-window-deleting", "net-node-deleting").await;
    let node_id =
        available_node(&router, &network_id, &member_id, "node-window-deleting-node").await;
    let answer = delete_node(&router, &network_id, &member_id, &node_id).await;
    assert_eq!(
        answer.status, 200,
        "case a node being removed: expected DeleteNode 200, got {} {:?}",
        answer.status, answer.body
    );
    let answer = update_node(&router, &network_id, &member_id, &node_id, &patch).await;
    assert_eq!(
        answer.status, 200,
        "case a node being removed: expected UpdateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    let during = node_status(&router, &network_id, &member_id, &node_id).await;
    assert_eq!(
        during, DELETING,
        "case a node being removed: expected the removal to keep reporting {DELETING}, got \
         {during:?}"
    );
    let updating = listed_node_ids(&router, &network_id, &member_id, UPDATING).await;
    assert!(
        updating.is_empty(),
        "case a node being removed: expected ListNodes status={UPDATING} to report \
         nothing, got {updating:?}"
    );
}

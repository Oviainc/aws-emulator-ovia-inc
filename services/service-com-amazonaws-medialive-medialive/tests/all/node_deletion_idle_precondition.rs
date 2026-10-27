// SPDX-License-Identifier: CC0-1.0

//! `DeleteNode`'s documented precondition — "Delete a Node. The Node must be
//! IDLE." (`smithy_query(query="doc", shape="DeleteNode")`) — end to end over
//! the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suite is a TABLE whose rows differ along the two things the rule
//! generalises over: what the channel reaching the node is DOING when the delete
//! arrives (the [`Work`] axis — no channel at all, one that never started, one
//! coming up, one up, one coming down, one stopped again, one deleted, and one
//! running on a different node of the same cluster), and which state the node
//! itself is in (where the create left it, and each value `UpdateNodeState`
//! accepts). A node is reached by work through the ChannelPlacementGroup it is
//! assigned to, so each row builds that chain — cluster, node, group, channel —
//! out of ids the store really minted.

use serde_json::json;
use service_com_amazonaws_medialive_medialive::state::node_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_groups::{create_channel, created as created_group, id_of};
use crate::node_lifecycle::{
    cluster, created as created_node, delete_node, described, node_id, state_updated,
};
use crate::wire::{call, error_code, error_message, medialive};

/// Every state a CALLER can put a node into, plus the row that asks for none and
/// leaves the node where `CreateNode` put it.
///
/// `ACTIVE` and `DRAINING` are exactly the value set of `UpdateNodeStateShape`
/// (`smithy_query(query="shape", shape="UpdateNodeStateShape")`), named through
/// the service's own constants so the suite and the store read one source.
const REQUESTED_STATES: [Option<&str>; 3] =
    [None, Some(node_state::ACTIVE), Some(node_state::DRAINING)];

/// What the channel that reaches the node is doing when `DeleteNode` arrives.
#[derive(Clone, Copy, Debug)]
enum Work {
    /// Nothing is placed in the group the node is assigned to.
    NoChannel,
    /// A channel is placed there but has never been started (`IDLE`).
    NeverStarted,
    /// A start has been accepted and has not settled yet (`STARTING`).
    ComingUp,
    /// The channel is up (`RUNNING`).
    Up,
    /// A stop has been accepted and has not settled yet (`STOPPING`).
    ComingDown,
    /// The channel ran and has stopped again (`IDLE`).
    StoppedAgain,
    /// The channel was deleted, so it is placed nowhere (`DELETED`).
    Gone,
    /// A channel is up in ANOTHER group of the same cluster, assigned to
    /// another node.
    UpOnAnotherNode,
}

impl Work {
    /// Whether this phase leaves work running ON the node under test, which is
    /// what the delete is refused for.
    fn occupies_the_node(self) -> bool {
        matches!(self, Self::ComingUp | Self::Up | Self::ComingDown)
    }
}

/// `POST /prod/channels/{ChannelId}/start`, asserted to have been accepted.
async fn start_channel(router: &MultiServiceRouter, case: &str, channel: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/prod/channels/{channel}/start"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartChannel({channel}): expected 200, got {status}: {body}"
    );
}

/// `POST /prod/channels/{ChannelId}/stop`, asserted to have been accepted.
async fn stop_channel(router: &MultiServiceRouter, case: &str, channel: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/prod/channels/{channel}/stop"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StopChannel({channel}): expected 200, got {status}: {body}"
    );
}

/// The `State` `DescribeChannel` reports for `channel`.
async fn channel_state_of(router: &MultiServiceRouter, case: &str, channel: &str) -> String {
    let (status, body) = call(router, "GET", &format!("/prod/channels/{channel}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel({channel}): expected 200, got {status}: {body}"
    );
    body["state"].as_str().unwrap_or_default().to_string()
}

/// Bring `channel` all the way down, whatever phase it is in, and answer the
/// state it settled in: a start that was in flight is let finish, a running
/// channel is stopped, and a stop that was in flight is let finish.
async fn settle_down(router: &MultiServiceRouter, case: &str, channel: &str) -> String {
    wait_for_the_clock();
    if channel_state_of(router, case, channel).await == "RUNNING" {
        stop_channel(router, case, channel).await;
        wait_for_the_clock();
    }
    channel_state_of(router, case, channel).await
}

/// One cluster holding the node under test, the group that node is assigned to,
/// and the channel `work` describes, driven to the phase `work` names.
///
/// Answers the cluster, the node, and the channel the row created (`None` for
/// the row that places nothing).
async fn chain(
    router: &MultiServiceRouter,
    case: &str,
    work: Work,
) -> (String, String, Option<String>) {
    let cluster_id = cluster(router, "cluster").await;
    let node = created_node(router, case, &cluster_id, json!({ "name": "node" })).await;
    let node = node_id(case, &node);
    let group = created_group(
        router,
        case,
        &cluster_id,
        json!({ "name": "group", "nodes": [node.clone()] }),
    )
    .await;
    let group = id_of(&group);

    let placement = match work {
        Work::NoChannel => None,
        Work::UpOnAnotherNode => {
            // A second node of the same cluster, with a group of its own: the
            // channel runs in the account, in this cluster, but not on the node
            // under test.
            let other = created_node(router, case, &cluster_id, json!({ "name": "other-node" })).await;
            let other = node_id(case, &other);
            let other_group = created_group(
                router,
                case,
                &cluster_id,
                json!({ "name": "other-group", "nodes": [other] }),
            )
            .await;
            Some(id_of(&other_group))
        }
        _ => Some(group),
    };

    let channel = match placement {
        None => None,
        Some(group) => Some(create_channel(router, case, "channel", Some((&cluster_id, &group))).await),
    };

    // A created channel is `CREATING` and settles to `IDLE`; every phase below
    // starts from there.
    if let Some(channel) = channel.as_deref() {
        wait_for_the_clock();
        match work {
            Work::NeverStarted | Work::NoChannel => {}
            Work::ComingUp => start_channel(router, case, channel).await,
            Work::Up | Work::UpOnAnotherNode => {
                start_channel(router, case, channel).await;
                wait_for_the_clock();
            }
            Work::ComingDown => {
                start_channel(router, case, channel).await;
                wait_for_the_clock();
                stop_channel(router, case, channel).await;
            }
            Work::StoppedAgain => {
                start_channel(router, case, channel).await;
                wait_for_the_clock();
                stop_channel(router, case, channel).await;
                wait_for_the_clock();
            }
            Work::Gone => {
                let (status, body) =
                    call(router, "DELETE", &format!("/prod/channels/{channel}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteChannel({channel}): expected 200, got {status}: {body}"
                );
                wait_for_the_clock();
            }
        }
    }

    (cluster_id, node, channel)
}

/// Rule: `DeleteNode` is accepted only for a node that is IDLE — a node a
/// channel is running on (one placed in a ChannelPlacementGroup the node is
/// assigned to, whose pipelines are up) reports `IN_USE`, is refused with the
/// modelled `ConflictException` (409) naming the node and the channels, and is
/// left exactly as it was; once the work has stopped the very same delete is
/// accepted with 202 and the node reports `DEREGISTERING`.
///
/// Which state a CALLER last asked for is not a precondition of its own: every
/// row runs for the node as `CreateNode` left it and for each value
/// `UpdateNodeState` accepts, and the answer depends only on the work.
#[tokio::test]
async fn delete_node_is_refused_exactly_while_a_channel_runs_on_the_node() {
    // (case, what the channel reaching the node is doing)
    let phases: [(&str, Work); 8] = [
        ("no channel placed", Work::NoChannel),
        ("placed, never started", Work::NeverStarted),
        ("placed, coming up", Work::ComingUp),
        ("placed, up", Work::Up),
        ("placed, coming down", Work::ComingDown),
        ("placed, stopped again", Work::StoppedAgain),
        ("placed, then deleted", Work::Gone),
        ("up on another node", Work::UpOnAnotherNode),
    ];

    for (phase, work) in phases {
        for requested in REQUESTED_STATES {
            let case = format!("{phase} / state {requested:?}");
            let case = case.as_str();
            let router = medialive();
            let (cluster_id, node, channel) = chain(&router, case, work).await;

            // The state the node is in when the delete arrives: the one the
            // caller asked for, or where the create left it.
            if let Some(state) = requested {
                let updated = state_updated(&router, case, &cluster_id, &node, Some(state)).await;
                assert_eq!(
                    updated["state"],
                    json!(if work.occupies_the_node() {
                        node_state::IN_USE
                    } else {
                        state
                    }),
                    "case {case:?}: expected UpdateNodeState to report the node's current state, \
                     got {:?}",
                    updated["state"]
                );
            }
            let expected_state = match (work.occupies_the_node(), requested) {
                (true, _) => node_state::IN_USE,
                (false, Some(state)) => state,
                (false, None) => node_state::CREATED,
            };
            let before = described(&router, case, &cluster_id, &node).await;
            assert_eq!(
                before["state"],
                json!(expected_state),
                "case {case:?}: expected DescribeNode to report {expected_state}, got {:?}",
                before["state"]
            );

            let (status, body) = delete_node(&router, &cluster_id, &node).await;
            if work.occupies_the_node() {
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected deleting a node a channel runs on to conflict, \
                     got {status}: {body}"
                );
                assert_eq!(
                    error_code(status, &body),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {body}"
                );
                let message = error_message(&body);
                let channel = channel
                    .clone()
                    .unwrap_or_else(|| panic!("case {case:?}: this phase places a channel"));
                assert!(
                    message.contains(&node) && message.contains(&channel),
                    "case {case:?}: expected the refusal to name node {node} and channel \
                     {channel}, got {message:?}"
                );
                assert!(
                    message.to_ascii_lowercase().contains("idle"),
                    "case {case:?}: expected the refusal to name the precondition it enforces, \
                     got {message:?}"
                );

                // Nothing was written: the node is still where it was, not
                // heading for deregistration.
                let after = described(&router, case, &cluster_id, &node).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: expected a refused delete to leave the node exactly as it \
                     was, got {after}"
                );

                // The refusal is about the WORK, not about the node: once the
                // channel has stopped, the same delete is accepted.
                let settled = settle_down(&router, case, &channel).await;
                assert_eq!(
                    settled, "IDLE",
                    "case {case:?}: expected the channel to have stopped, got {settled}"
                );
                let (status, accepted) = delete_node(&router, &cluster_id, &node).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: expected the delete to be accepted once the channel stopped, \
                     got {status}: {accepted}"
                );
                assert_eq!(
                    accepted["state"],
                    json!(node_state::DEREGISTERING),
                    "case {case:?}: expected the accepted delete to report DEREGISTERING, got {:?}",
                    accepted["state"]
                );
            } else {
                assert_eq!(
                    status, 202,
                    "case {case:?}: expected deleting an idle node to be accepted, \
                     got {status}: {body}"
                );
                assert_eq!(
                    body["state"],
                    json!(node_state::DEREGISTERING),
                    "case {case:?}: expected the accepted delete to report DEREGISTERING, got {:?}",
                    body["state"]
                );
                let after = described(&router, case, &cluster_id, &node).await;
                assert_eq!(
                    after["state"],
                    json!(node_state::DEREGISTERING),
                    "case {case:?}: expected the stored node to be DEREGISTERING, got {:?}",
                    after["state"]
                );
            }
        }
    }
}

/// Rule: a node reports `IN_USE` exactly while a channel is running on it, and
/// the state a caller asked for again as soon as that work stops — the reported
/// `State` is derived from the store on every read, so the create, the read and
/// the state update cannot disagree about it.
///
/// One cluster, one node, one group, one channel, walked through the channel's
/// own lifecycle: the rows are the phases of that walk, and the node's reported
/// state is asserted at each.
#[tokio::test]
async fn a_node_reports_in_use_while_its_channel_runs() {
    let case = "in-use reporting";
    let router = medialive();
    let cluster_id = cluster(&router, "cluster").await;
    let node = created_node(&router, case, &cluster_id, json!({ "name": "node" })).await;
    let node = node_id(case, &node);
    let group = created_group(
        &router,
        case,
        &cluster_id,
        json!({ "name": "group", "nodes": [node.clone()] }),
    )
    .await;
    let group = id_of(&group);
    // The caller isolates the node, so the state it reports between runs is
    // one the caller asked for rather than the one the create left — and the
    // row that is most at stake: a drained node is idle, not busy.
    state_updated(
        &router,
        case,
        &cluster_id,
        &node,
        Some(node_state::DRAINING),
    )
    .await;
    let channel = create_channel(&router, case, "channel", Some((&cluster_id, &group))).await;

    // (what just happened, the state the node reports after it)
    let walk: [(&str, &str); 5] = [
        ("the channel is still creating", node_state::DRAINING),
        ("the channel has settled to IDLE", node_state::DRAINING),
        ("the channel is STARTING", node_state::IN_USE),
        ("the channel is RUNNING", node_state::IN_USE),
        ("the channel has stopped again", node_state::DRAINING),
    ];

    for (step, (what, expected)) in walk.iter().enumerate() {
        match step {
            0 => {}
            1 => wait_for_the_clock(),
            2 => start_channel(&router, what, &channel).await,
            3 => wait_for_the_clock(),
            _ => {
                let settled = settle_down(&router, what, &channel).await;
                assert_eq!(
                    settled, "IDLE",
                    "case {what:?}: expected the channel to have stopped, got {settled}"
                );
            }
        }
        let reported = described(&router, what, &cluster_id, &node).await;
        assert_eq!(
            reported["state"],
            json!(expected),
            "case {what:?}: expected DescribeNode to report {expected}, got {:?}",
            reported["state"]
        );
        // The group the node is assigned to is reported whatever the work is
        // doing: the association is not what makes the node busy.
        assert_eq!(
            reported["channelPlacementGroups"],
            json!([group]),
            "case {what:?}: expected the node to report its group, got {:?}",
            reported["channelPlacementGroups"]
        );
    }
}

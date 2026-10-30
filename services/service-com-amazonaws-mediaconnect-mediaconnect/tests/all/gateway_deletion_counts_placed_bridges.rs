// SPDX-License-Identifier: CC0-1.0

//! The bridge half of DeleteGateway's documented precondition — "before you can delete a
//! gateway, you must deregister its instances and delete its bridges" — end to end over
//! HTTP: CreateGateway -> CreateBridge -> UpdateBridgeState -> DeleteGateway.
//!
//! The rule under test: a child record the service keeps only so it stays READABLE in its
//! terminal retired state is no longer one of its parent's children, so it does not hold
//! the parent's delete back; a child whose retirement is still in flight has not reached
//! that state and still does. Read against a gateway, "its bridges" are therefore the
//! bridges placed on it that have not reached the terminal `DELETED` state — the same
//! reading of the sentence its instance half applies to the terminal `DEREGISTERED`
//! (`tests/all/gateway_instances.rs`).
//!
//! The rows span what that rule generalises over: the state the placed bridge carries
//! (EVERY value of the modelled `BridgeState` enum, from the model's own catalog), how MANY
//! bridges are placed and in which mix of states (none, one, several, and several of mixed
//! state, so the count the refusal reports is asserted too), and whether the bridge reached
//! its retirement over the wire through UpdateBridgeState or in the store.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::BRIDGE_STATE_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{create_bridge, describe_bridge, gateway_with_networks};
use crate::bridge_members_and_deletion::force_bridge_state;
use crate::flow_lifecycle::Emulator;

/// Long enough for the clock to have made a transition due: the span the service holds a
/// bridge in a transient state (`mc::TRANSITION_SPAN`) plus slack for a slow runner.
fn settle_span() -> Duration {
    mc::TRANSITION_SPAN + Duration::from_millis(400)
}

async fn delete_gateway(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/gateways/{arn}"), None)
        .await
}

async fn describe_gateway(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/gateways/{arn}"), None)
        .await
}

async fn update_bridge_state(
    emulator: &Emulator,
    arn: &str,
    desired_state: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/bridges/{arn}/state"),
            Some(json!({ "desiredState": desired_state })),
        )
        .await
}

/// A ground-to-cloud (ingress) bridge placed on `placement_arn`, carrying the one source
/// CreateBridge requires — a network source, the kind that direction ingests, naming the
/// network the fixture gateway declares.
fn bridge_body(name: &str, placement_arn: &str) -> Value {
    json!({
        "name": name,
        "placementArn": placement_arn,
        "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 2 },
        "sources": [{ "networkSource": {
            "multicastIp": "224.0.1.1",
            "name": "src",
            "networkName": "net-1",
            "port": 5000,
            "protocol": "rtp",
        }}],
    })
}

/// Create one bridge placed on `placement_arn` over the wire and answer its ARN.
async fn bridge_on(emulator: &Emulator, case: &str, name: &str, placement_arn: &str) -> String {
    let (status, code, created) = create_bridge(emulator, bridge_body(name, placement_arn)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture bridge create expected 201, got {status} {code:?}: {created}"
    );
    created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers a bridge ARN: {created}"))
        .to_string()
}

/// Place one bridge per entry of `bridge_states` on `placement_arn`, each in the state the
/// entry names, and answer their ARNs in order.
async fn bridges_in_states(
    emulator: &Emulator,
    case: &str,
    placement_arn: &str,
    bridge_states: &[&str],
) -> Vec<String> {
    let mut arns = Vec::new();
    for (index, bridge_state) in bridge_states.iter().enumerate() {
        let name = format!("bridge-{index}");
        let arn = bridge_on(emulator, case, &name, placement_arn).await;
        force_bridge_state(emulator, &arn, bridge_state);
        arns.push(arn);
    }
    arns
}

/// Assert what DeleteGateway answers for a gateway carrying `placed` bridges the
/// precondition counts: the delete goes through when none are left, and is refused with
/// this service's state-refusal envelope — naming the gateway and the count — while any
/// remain.
async fn assert_delete(emulator: &Emulator, case: &str, gateway_arn: &str, placed: usize) {
    let (status, code, answered) = delete_gateway(emulator, gateway_arn).await;
    if placed == 0 {
        assert_eq!(
            (status, code.as_deref(), answered["gatewayArn"].as_str()),
            (200, None, Some(gateway_arn)),
            "case {case:?}: no bridge is still placed on the gateway, so the delete goes \
             through, got {status} {code:?}: {answered}"
        );
        let (status, code, described) = describe_gateway(emulator, gateway_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the retired gateway stays readable, got {status} {code:?}: \
             {described}"
        );
        assert_eq!(
            described["gateway"]["gatewayState"].as_str(),
            Some(mc::GATEWAY_STATE_DELETING),
            "case {case:?}: the accepted delete puts the gateway on its way out, got \
             {described}"
        );
    } else {
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: {placed} bridge(s) are still placed on the gateway, so the \
             delete is refused, got {status} {code:?}: {answered}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(mc::BAD_REQUEST_PREFIX)
                && message.contains(gateway_arn)
                && message.contains(&format!("{placed} bridge(s)")),
            "case {case:?}: the refusal is this service's state-refusal envelope naming the \
             gateway and the {placed} bridge(s) it counts, got {message:?}"
        );
        let (status, code, described) = describe_gateway(emulator, gateway_arn).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                described["gateway"]["gatewayState"].as_str()
            ),
            (200, None, Some(mc::GATEWAY_STATE_ACTIVE)),
            "case {case:?}: a refused delete leaves the gateway as it was, got {status} \
             {code:?}: {described}"
        );
    }
}

/// Every value of the modelled `BridgeState` enum — the catalog is `BRIDGE_STATE_VALUES`,
/// generated from the model (`smithy_query(query="shape",shape="BridgeState")`) — held by
/// the one bridge placed on a gateway: the delete is refused for each, except for the
/// terminal `DELETED`, where the record is retired and no longer one of the gateway's
/// bridges.
#[tokio::test]
async fn only_a_terminally_deleted_bridge_stops_holding_its_gateways_delete_back() {
    let emulator = Emulator::new();

    for bridge_state in BRIDGE_STATE_VALUES {
        let case = format!("a bridge in {bridge_state} is placed on the gateway");
        let gateway_arn = gateway_with_networks(
            &emulator,
            &format!("gw-{}", bridge_state.to_lowercase()),
            &["net-1"],
        )
        .await;
        bridges_in_states(&emulator, &case, &gateway_arn, &[bridge_state]).await;
        // The clock settles a new gateway into ACTIVE; the bridge states are forced, so
        // nothing else is owed.
        tokio::time::sleep(settle_span()).await;

        let placed = usize::from(*bridge_state != mc::BRIDGE_STATE_DELETED);
        assert_delete(&emulator, &case, &gateway_arn, placed).await;
    }
}

/// One row of the mixed-placement table: the states of the bridges placed on the case's own
/// gateway, and how many of them the precondition counts.
struct PlacementCase {
    case: &'static str,
    bridge_states: &'static [&'static str],
    /// The bridges that have NOT reached the terminal `DELETED` state.
    placed: usize,
}

/// How many bridges are placed, and in which mix of states: the count the refusal reports
/// is the number of bridges that are still the gateway's, whatever else the store holds for
/// it.
const PLACEMENT_CASES: [PlacementCase; 6] = [
    PlacementCase {
        case: "no bridge was ever placed on the gateway",
        bridge_states: &[],
        placed: 0,
    },
    PlacementCase {
        case: "every placed bridge reached the terminal DELETED",
        bridge_states: &[mc::BRIDGE_STATE_DELETED, mc::BRIDGE_STATE_DELETED],
        placed: 0,
    },
    PlacementCase {
        case: "one retired bridge beside one running bridge",
        bridge_states: &[mc::BRIDGE_STATE_DELETED, mc::BRIDGE_STATE_ACTIVE],
        placed: 1,
    },
    PlacementCase {
        case: "one retired bridge beside one whose deletion is still in flight",
        bridge_states: &[mc::BRIDGE_STATE_DELETED, mc::BRIDGE_STATE_DELETING],
        placed: 1,
    },
    PlacementCase {
        case: "two live bridges and one retired one",
        bridge_states: &[
            mc::BRIDGE_STATE_STANDBY,
            mc::BRIDGE_STATE_DELETED,
            mc::BRIDGE_STATE_ACTIVE,
        ],
        placed: 2,
    },
    PlacementCase {
        case: "three live bridges",
        bridge_states: &[
            mc::BRIDGE_STATE_CREATING,
            mc::BRIDGE_STATE_STANDBY,
            mc::BRIDGE_STATE_UPDATING,
        ],
        placed: 3,
    },
];

#[tokio::test]
async fn the_precondition_counts_only_the_bridges_still_placed_on_the_gateway() {
    let emulator = Emulator::new();

    for (index, row) in PLACEMENT_CASES.into_iter().enumerate() {
        let gateway_arn =
            gateway_with_networks(&emulator, &format!("gw-mix-{index}"), &["net-1"]).await;
        bridges_in_states(&emulator, row.case, &gateway_arn, row.bridge_states).await;
        tokio::time::sleep(settle_span()).await;

        assert_delete(&emulator, row.case, &gateway_arn, row.placed).await;
    }
}

/// A bridge another gateway holds is not this gateway's: the precondition reads the
/// placement each bridge names, so a live bridge placed elsewhere never holds this delete
/// back.
#[tokio::test]
async fn a_bridge_placed_on_another_gateway_does_not_hold_this_delete_back() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with_networks(&emulator, "gw-empty", &["net-1"]).await;
    let other_arn = gateway_with_networks(&emulator, "gw-elsewhere", &["net-1"]).await;
    let case = "a live bridge is placed on another gateway";
    bridges_in_states(&emulator, case, &other_arn, &[mc::BRIDGE_STATE_ACTIVE]).await;
    tokio::time::sleep(settle_span()).await;

    assert_delete(&emulator, case, &gateway_arn, 0).await;
    // The other gateway still has its bridge, so its own delete is still refused.
    assert_delete(&emulator, "the gateway the bridge is placed on", &other_arn, 1).await;
}

/// The same rule driven entirely over the wire, the way a client reaches it: a bridge asked
/// for the `DELETED` member of `DesiredState` holds its gateway's delete back while that
/// move is in flight, and stops holding it back once the clock has settled the bridge in the
/// terminal state — the bridge-side counterpart of a deregistration settling into
/// `DEREGISTERED`.
#[tokio::test]
async fn a_bridge_retired_through_update_bridge_state_releases_its_gateway() {
    let emulator = Emulator::new();
    let case = "a bridge asked for DELETED";
    let gateway_arn = gateway_with_networks(&emulator, "gw-wire", &["net-1"]).await;
    let bridge_arn = bridge_on(&emulator, case, "bridge-wire", &gateway_arn).await;
    tokio::time::sleep(settle_span()).await;

    assert_delete(&emulator, "the bridge is live", &gateway_arn, 1).await;

    let (status, code, answered) =
        update_bridge_state(&emulator, &bridge_arn, mc::BRIDGE_STATE_DELETED).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            answered["desiredState"].as_str(),
            answered["bridgeArn"].as_str()
        ),
        (
            202,
            None,
            Some(mc::BRIDGE_STATE_DELETED),
            Some(bridge_arn.as_str())
        ),
        "case {case:?}: the state change expected 202 and the state it is headed for, got \
         {status} {code:?}: {answered}"
    );

    // The deletion is in flight, so the bridge has not reached its terminal state and is
    // still one of the gateway's.
    let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            described["bridge"]["bridgeState"].as_str()
        ),
        (200, None, Some(mc::BRIDGE_STATE_DELETING)),
        "case {case:?}: the bridge reports DELETING while that move is in flight, got \
         {status} {code:?}: {described}"
    );
    assert_delete(&emulator, "the bridge is still DELETING", &gateway_arn, 1).await;

    tokio::time::sleep(settle_span()).await;
    let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            described["bridge"]["bridgeState"].as_str()
        ),
        (200, None, Some(mc::BRIDGE_STATE_DELETED)),
        "case {case:?}: the clock settles the bridge in the terminal DELETED, got {status} \
         {code:?}: {described}"
    );
    assert_delete(&emulator, "the bridge reached DELETED", &gateway_arn, 0).await;

    // The retired bridge stays readable, as the terminal `DELETED` state says: the gateway
    // delete neither removed it nor changed it.
    let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            described["bridge"]["bridgeState"].as_str()
        ),
        (200, None, Some(mc::BRIDGE_STATE_DELETED)),
        "case {case:?}: the retired bridge is still reported after the gateway delete, got \
         {status} {code:?}: {described}"
    );
}

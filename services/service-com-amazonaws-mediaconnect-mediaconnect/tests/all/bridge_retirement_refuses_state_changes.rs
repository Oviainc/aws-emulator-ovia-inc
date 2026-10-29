// SPDX-License-Identifier: CC0-1.0

//! The state a RETIRED bridge may be asked for, end to end over HTTP: CreateBridge ->
//! UpdateBridgeState -> DescribeBridge.
//!
//! The rule under test is the one `bridge_retirement_refuses_changes` asserts for the
//! operations that write a bridge's configuration, applied to the operation that writes its
//! STATE: a bridge whose deletion has been accepted is retired, so no request may change it
//! any more — UpdateBridgeState answers `ConflictException` (HTTP 409) and the bridge keeps
//! the state it is in. Asking a retired bridge for `DELETED` again is not a change but the
//! `@idempotent` repeat of the move already accepted, so it is still taken, and a bridge in
//! any other state of the modelled `BridgeState` enum takes every desired state.
//!
//! The rows below vary what that rule generalises over: EVERY value of the modelled
//! `BridgeState` enum as the state the addressed bridge is in (the two a retirement reaches
//! and the ten that leave it live), EVERY value of the modelled `DesiredState` enum as the
//! state asked for, and how the bridge reached its state — forced in the store, which is
//! how a bridge SITTING in a retirement state is reached, and driven over the wire, which
//! is how one still SETTLING into the terminal state is.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::{
    BRIDGE_STATE_VALUES, DESIRED_STATE_VALUES,
};
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::bridge_members_and_deletion::force_bridge_state;
use crate::flow_lifecycle::Emulator;

/// Long enough for the clock to have made a transition due: the span the service holds a
/// bridge in a transient state (`mc::TRANSITION_SPAN`) plus slack for a slow runner.
fn settle_span() -> Duration {
    mc::TRANSITION_SPAN + Duration::from_millis(400)
}

/// Every value of the modelled `BridgeState` enum (the catalog is `BRIDGE_STATE_VALUES`,
/// generated from the model), paired with whether a bridge carrying it has been retired —
/// the states a bridge reaches once its deletion has been accepted, and only those.
fn every_bridge_state() -> Vec<(&'static str, bool)> {
    let states: Vec<(&'static str, bool)> = BRIDGE_STATE_VALUES
        .iter()
        .map(|state| (*state, mc::is_bridge_retirement_state(state)))
        .collect();
    assert_eq!(
        states.len(),
        12,
        "the modelled BridgeState enum has 12 values, got {states:?}"
    );
    assert_eq!(
        states.iter().filter(|(_, retired)| *retired).count(),
        2,
        "exactly two of them are retirement states, got {states:?}"
    );
    states
}

async fn update_bridge_state(
    emulator: &Emulator,
    arn: &str,
    desired: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/bridges/{arn}/state"),
            Some(json!({ "desiredState": desired })),
        )
        .await
}

/// Create one cloud-to-ground (egress) bridge and answer its ARN. CreateBridge is
/// `bridge_lifecycle`'s subject; here it is only the fixture every row addresses.
async fn fixture_bridge(
    emulator: &Emulator,
    name: &str,
    placement_arn: &str,
    flow_arn: &str,
) -> String {
    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": name,
            "placementArn": placement_arn,
            "egressGatewayBridge": { "maxBitrate": 20_000_000 },
            "sources": [{ "flowSource": { "flowArn": flow_arn, "name": "src-a" } }],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "fixture {name:?}: create expected 201, got {status} {code:?}: {created}"
    );
    created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {name:?}: the create answers an ARN: {created}"))
        .to_string()
}

/// The `Bridge` DescribeBridge reports for `arn`, asserted to be reported at all: a retired
/// bridge stays readable, which is what makes the refusal observable.
async fn bridge_of(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, described) = describe_bridge(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
    );
    described["bridge"].clone()
}

/// Every state of the modelled enum against every desired state: a retired bridge refuses
/// each state but the one its accepted deletion is already headed for, and keeps the state
/// it is in; a live bridge takes them all.
#[tokio::test]
async fn a_retired_bridge_refuses_every_state_but_its_own_deletion() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-retire-state", &["net-1"]).await;
    let flow_arn = cloud_flow(&emulator, "flow-retire-state").await;

    for (state_index, (bridge_state, retired)) in every_bridge_state().into_iter().enumerate() {
        for (desired_index, desired) in DESIRED_STATE_VALUES.iter().enumerate() {
            let case = format!("{bridge_state} / asked for {desired}");
            // One bridge per row, so an accepted move cannot carry over into the next row.
            let arn = fixture_bridge(
                &emulator,
                &format!("bridge-retire-state-{state_index}-{desired_index}"),
                &placement,
                &flow_arn,
            )
            .await;
            force_bridge_state(&emulator, &arn, bridge_state);

            let (status, code, answered) = update_bridge_state(&emulator, &arn, desired).await;
            // Asking for the deletion a retired bridge is already headed for is the
            // `@idempotent` repeat of a move already accepted, not a change to the bridge.
            let refused = retired && *desired != mc::BRIDGE_STATE_DELETED;
            if refused {
                assert_eq!(
                    (status, code.as_deref()),
                    (409, Some("ConflictException")),
                    "case {case:?}: a retired bridge refuses another state, got {status} \
                     {code:?}: {answered}"
                );
                let message = answered["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(&arn) && message.contains(bridge_state),
                    "case {case:?}: the refusal names the bridge and the state it is in, got \
                     {message:?}"
                );
                // A refusal about the resource's STATE carries neither of the envelopes this
                // service words a refusal about a request's CONTENT in.
                for envelope in [mc::VALIDATION_FAILED_PREFIX, mc::NOT_FOUND_PREFIX] {
                    assert!(
                        !message.starts_with(envelope),
                        "case {case:?}: a state conflict is not worded in the {envelope:?} \
                         envelope, got {message:?}"
                    );
                }
                let bridge = bridge_of(&emulator, &case, &arn).await;
                assert_eq!(
                    bridge["bridgeState"].as_str(),
                    Some(bridge_state),
                    "case {case:?}: the refused call left the bridge in the state it was in, \
                     got {bridge}"
                );
                continue;
            }

            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: the state is accepted, got {status} {code:?}: {answered}"
            );
            assert_eq!(
                answered["desiredState"].as_str(),
                Some(*desired),
                "case {case:?}: the answer is the state the stored bridge is headed for, got \
                 {answered}"
            );
        }
    }
}

/// The same rule reached the way a client reaches it, with no state forced: a bridge the
/// clock is still settling into the terminal state is as retired as one already in it, so
/// the state asked for is refused while the deletion is in flight and again once it has
/// settled — and the record stays readable throughout.
#[tokio::test]
async fn a_bridge_settling_into_its_deletion_refuses_another_state() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-retire-state-live", &["net-1"]).await;
    let flow_arn = cloud_flow(&emulator, "flow-retire-state-live").await;
    let arn = fixture_bridge(&emulator, "bridge-retire-state-live", &placement, &flow_arn).await;

    // The bridge the create left in a transient state settles into the one a created bridge
    // waits in, and that live bridge takes a desired state.
    tokio::time::sleep(settle_span()).await;
    let (status, code, started) =
        update_bridge_state(&emulator, &arn, mc::BRIDGE_STATE_ACTIVE).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "a live bridge takes ACTIVE, got {status} {code:?}: {started}"
    );
    tokio::time::sleep(settle_span()).await;

    let (status, code, accepted) =
        update_bridge_state(&emulator, &arn, mc::BRIDGE_STATE_DELETED).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "the move to DELETED is accepted, got {status} {code:?}: {accepted}"
    );

    // While the deletion is in flight the bridge is still reported in the transient state,
    // and once the clock has settled it, in the terminal one. Either way its deletion has
    // been accepted, so another state is refused.
    for expected_state in [mc::BRIDGE_STATE_DELETING, mc::BRIDGE_STATE_DELETED] {
        let bridge = bridge_of(&emulator, expected_state, &arn).await;
        assert_eq!(
            bridge["bridgeState"].as_str(),
            Some(expected_state),
            "case {expected_state:?}: the bridge is readable in the state it is in, got \
             {bridge}"
        );

        for desired in DESIRED_STATE_VALUES {
            if *desired == mc::BRIDGE_STATE_DELETED {
                continue;
            }
            let case = format!("{expected_state} / asked for {desired}");
            let (status, code, answered) = update_bridge_state(&emulator, &arn, desired).await;
            assert_eq!(
                (status, code.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: a bridge whose deletion was accepted refuses another state, \
                 got {status} {code:?}: {answered}"
            );
        }

        let after = bridge_of(&emulator, expected_state, &arn).await;
        assert_eq!(
            after["bridgeState"].as_str(),
            Some(expected_state),
            "case {expected_state:?}: the refused calls left the bridge where it was, got \
             {after}"
        );
        tokio::time::sleep(settle_span()).await;
    }
}

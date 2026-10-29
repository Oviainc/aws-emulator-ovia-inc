// SPDX-License-Identifier: CC0-1.0

//! A bridge on its way out admits no further change, end to end over HTTP: CreateBridge ->
//! UpdateBridgeState -> every operation that WRITES to a stored bridge -> DescribeBridge.
//!
//! The rule under test: a bridge whose move to the `DELETED` member of `DesiredState` has
//! been accepted is RETIRED — it reports `DELETING` until the clock settles it in the
//! terminal `DELETED`, and from that instant no request may write to it any more, because
//! the deletion it is settling into is irreversible. Every such request is refused with
//! `ConflictException` (HTTP 409) and the stored bridge is left byte for byte as it was. A
//! bridge in any other state of the modelled `BridgeState` enum is live and takes the same
//! request.
//!
//! The rows below vary what that rule generalises over:
//!
//! * EVERY value of the modelled `BridgeState` enum
//!   (`smithy_query(query="shape",shape="BridgeState")`) as the state the addressed bridge
//!   is in — the two a retirement reaches and the ten that leave it live; and
//! * every operation that writes to a stored bridge, and which part of it the request
//!   touches: the bridge's own settings (UpdateBridge, on the type wrapper and on the
//!   failover config), its sources (AddBridgeSources, UpdateBridgeSource,
//!   RemoveBridgeSource) and its outputs (AddBridgeOutputs, UpdateBridgeOutput,
//!   RemoveBridgeOutput).
//!
//! The reporting operations are not in that set and are asserted to keep answering: a
//! retired bridge stays READABLE, which is what makes the refusal above observable.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::BRIDGE_STATE_VALUES;
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
/// generated from the model), and whether a bridge carrying it has been retired — the
/// states a bridge reaches once its deletion has been accepted, and only those.
///
/// `DELETING` is where a bridge sits while that deletion is in flight and `DELETED` is the
/// terminal state the clock settles it in; every other value is a state in which the bridge
/// is still a live resource.
fn every_bridge_state() -> Vec<(&'static str, bool)> {
    let states: Vec<(&'static str, bool)> = BRIDGE_STATE_VALUES
        .iter()
        .map(|state| {
            (
                *state,
                *state == mc::BRIDGE_STATE_DELETING || *state == mc::BRIDGE_STATE_DELETED,
            )
        })
        .collect();
    assert_eq!(
        states.len(),
        12,
        "the modelled BridgeState enum has 12 values, got {states:?}"
    );
    states
}

/// One request that WRITES to a stored bridge, as a client sends it: the method, the path
/// under `/v1/bridges/{BridgeArn}`, and the body it carries.
struct Change {
    case: &'static str,
    method: &'static str,
    /// What follows the bridge's ARN in the request URI (empty for the bridge itself).
    suffix: &'static str,
    body: Option<Value>,
}

/// The fixture bridge's own source and output names, which the member-level rows address.
const SOURCE_NAME: &str = "src-a";
const OUTPUT_NAME: &str = "out-a";

/// One network output, as a request configures it: the kind of output a cloud-to-ground
/// bridge delivers to the premises network.
fn network_output(name: &str, port: i64) -> Value {
    json!({ "networkOutput": {
        "ipAddress": "10.0.1.1",
        "name": name,
        "networkName": "net-1",
        "port": port,
        "protocol": "udp",
        "ttl": 32,
    }})
}

/// Every request that writes to a stored bridge, one row per part of the bridge it
/// touches. `flow_arn` is the cloud flow the fixture bridge's sources name.
fn changes(flow_arn: &str) -> Vec<Change> {
    vec![
        Change {
            case: "UpdateBridge, the bridge's own type wrapper",
            method: "PUT",
            suffix: "",
            body: Some(json!({ "egressGatewayBridge": { "maxBitrate": 150_000_000 } })),
        },
        Change {
            case: "UpdateBridge, the source failover config",
            method: "PUT",
            suffix: "",
            body: Some(json!({ "sourceFailoverConfig": { "failoverMode": "MERGE" } })),
        },
        Change {
            case: "AddBridgeSources, one more source",
            method: "POST",
            suffix: "/sources",
            body: Some(json!({ "sources": [
                { "flowSource": { "flowArn": flow_arn, "name": "src-b" } },
            ]})),
        },
        Change {
            case: "UpdateBridgeSource, the stored source",
            method: "PUT",
            suffix: "/sources/src-a",
            body: Some(json!({ "flowSource": { "flowArn": flow_arn } })),
        },
        Change {
            case: "RemoveBridgeSource, the stored source",
            method: "DELETE",
            suffix: "/sources/src-a",
            body: None,
        },
        Change {
            case: "AddBridgeOutputs, one more output",
            method: "POST",
            suffix: "/outputs",
            body: Some(json!({ "outputs": [network_output("out-b", 7_101)] })),
        },
        Change {
            case: "UpdateBridgeOutput, the stored output",
            method: "PUT",
            suffix: "/outputs/out-a",
            body: Some(json!({ "networkOutput": { "port": 7_102 } })),
        },
        Change {
            case: "RemoveBridgeOutput, the stored output",
            method: "DELETE",
            suffix: "/outputs/out-a",
            body: None,
        },
    ]
}

/// Send one row's request against the bridge `arn`.
async fn send_change(
    emulator: &Emulator,
    arn: &str,
    change: &Change,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            change.method,
            &format!("/v1/bridges/{arn}{}", change.suffix),
            change.body.clone(),
        )
        .await
}

/// Create one cloud-to-ground (egress) bridge holding the one source and the one output
/// every row addresses, and answer its ARN.
///
/// Its failover state is the one under which a bridge may hold a pair of sources at all
/// ("if the state is active, [it] can have one or two sources", `FailoverConfig$State`), so
/// the add-a-source row is a request a LIVE bridge accepts. CreateBridge itself is
/// `bridge_lifecycle`'s subject; here it is only the fixture.
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
            "sources": [{ "flowSource": { "flowArn": flow_arn, "name": SOURCE_NAME } }],
            "outputs": [network_output(OUTPUT_NAME, 7_100)],
            "sourceFailoverConfig": { "state": mc::STATE_ENABLED },
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

/// The `Bridge` DescribeBridge reports for `arn`, asserted to be reported at all: a bridge
/// on its way out stays readable, so the record a refused change left alone can be compared.
async fn bridge_of(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, described) = describe_bridge(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
    );
    described["bridge"].clone()
}

/// Every request that writes to a bridge is refused exactly while that bridge is retired,
/// and a refused request leaves the stored bridge byte for byte as it was.
#[tokio::test]
async fn a_retired_bridge_refuses_every_change() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-retirement", &["net-1"]).await;
    let flow_arn = cloud_flow(&emulator, "flow-retirement").await;

    for (state_index, (bridge_state, retired)) in every_bridge_state().into_iter().enumerate() {
        for (change_index, change) in changes(&flow_arn).iter().enumerate() {
            let case = format!("{bridge_state} / {}", change.case);
            let arn = fixture_bridge(
                &emulator,
                &format!("bridge-retire-{state_index}-{change_index}"),
                &placement,
                &flow_arn,
            )
            .await;
            force_bridge_state(&emulator, &arn, bridge_state);
            let before = bridge_of(&emulator, &case, &arn).await;

            let (status, code, answered) = send_change(&emulator, &arn, change).await;
            if retired {
                assert_eq!(
                    (status, code.as_deref()),
                    (409, Some("ConflictException")),
                    "case {case:?}: a change to a retired bridge expects 409 \
                     ConflictException, got {status} {code:?}: {answered}"
                );
                let message = answered["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(&arn) && message.contains(bridge_state),
                    "case {case:?}: the refusal names the bridge and the state it is in, \
                     got {answered}"
                );
                let after = bridge_of(&emulator, &case, &arn).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused change leaves the bridge byte for byte as it \
                     was, got {after}"
                );
            } else {
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: a live bridge takes the same change, got {status} \
                     {code:?}: {answered}"
                );
            }
        }
    }
}

/// The same rule reached the way a client reaches it, with no state forced: UpdateBridgeState
/// accepts the move to `DELETED`, and from that moment every change is refused — while the
/// deletion is still in flight (`DELETING`) and once the clock has settled it (`DELETED`) —
/// with the bridge's configuration still reported exactly as it was stored.
#[tokio::test]
async fn accepting_a_bridge_deletion_refuses_every_later_change() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-retirement-live", &["net-1"]).await;
    let flow_arn = cloud_flow(&emulator, "flow-retirement-live").await;
    let arn = fixture_bridge(&emulator, "bridge-retire-live", &placement, &flow_arn).await;

    // The bridge the create left in a transient state settles into the one a created bridge
    // waits in, and that live bridge takes a change.
    tokio::time::sleep(settle_span()).await;
    let (status, code, updated) = emulator
        .send(
            "PUT",
            &format!("/v1/bridges/{arn}"),
            Some(json!({ "egressGatewayBridge": { "maxBitrate": 30_000_000 } })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "a live bridge takes an update, got {status} {code:?}: {updated}"
    );

    let (status, code, accepted) = emulator
        .send(
            "PUT",
            &format!("/v1/bridges/{arn}/state"),
            Some(json!({ "desiredState": mc::BRIDGE_STATE_DELETED })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "the move to DELETED is accepted, got {status} {code:?}: {accepted}"
    );

    // The configuration the bridge holds as its deletion begins: nothing refused below may
    // move any of it.
    let configured = bridge_of(&emulator, "deletion accepted", &arn).await;
    assert_eq!(
        configured["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_DELETING),
        "the accepted deletion puts the bridge in DELETING, got {configured}"
    );

    // Twice over: while the deletion is in flight, and once the clock has settled the
    // terminal state.
    for expected_state in [mc::BRIDGE_STATE_DELETING, mc::BRIDGE_STATE_DELETED] {
        for change in changes(&flow_arn) {
            let case = format!("{expected_state} / {}", change.case);
            let (status, code, answered) = send_change(&emulator, &arn, &change).await;
            assert_eq!(
                (status, code.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: a change to a bridge on its way out expects 409 \
                 ConflictException, got {status} {code:?}: {answered}"
            );
        }

        let described = bridge_of(&emulator, expected_state, &arn).await;
        assert_eq!(
            described["bridgeState"].as_str(),
            Some(expected_state),
            "case {expected_state:?}: the bridge is readable in the state it is in, got \
             {described}"
        );
        for member in ["egressGatewayBridge", "sourceFailoverConfig", "sources", "outputs"] {
            assert_eq!(
                described[member], configured[member],
                "case {expected_state:?}: the refused changes left {member} as it was \
                 stored, got {described}"
            );
        }
        tokio::time::sleep(settle_span()).await;
    }
}

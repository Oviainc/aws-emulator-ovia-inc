// SPDX-License-Identifier: CC0-1.0

//! What a RETIRED bridge still admits, end to end over HTTP: CreateBridge ->
//! UpdateBridgeState -> DescribeBridge / ListBridges / DeleteBridge.
//!
//! `bridge_retirement_refuses_changes` pins what a bridge whose deletion has been accepted
//! REFUSES; this suite pins the limit of that refusal, which is the other half of the same
//! rule: the record is kept readable rather than evicted, so the reporting operations keep
//! answering for it with the configuration it was left with, and DeleteBridge — which
//! removes the RECORD rather than changing it — still takes it. A refusal that spread to
//! these calls would make a retired bridge unobservable and undeletable, which is what this
//! suite fails on.
//!
//! The rows below vary what that rule generalises over: each state a retirement reaches (the
//! transient one a deletion is in flight in and the terminal one the clock settles it in),
//! reached both over the wire through UpdateBridgeState and from the store, and each
//! reporting path a bridge is answered on (the single read and the listing).
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::bridge_members_and_deletion::force_bridge_state;
use crate::flow_lifecycle::Emulator;

/// Long enough for the clock to have made a transition due: the span the service holds a
/// bridge in a transient state (`mc::TRANSITION_SPAN`) plus slack for a slow runner.
fn settle_span() -> Duration {
    mc::TRANSITION_SPAN + Duration::from_millis(400)
}

/// One network output, as a request configures it and as a bridge reports it: the
/// configuration a retired bridge must still be reported with.
fn network_output(name: &str, port: i64) -> Value {
    json!({ "networkOutput": {
        "ipAddress": "10.0.1.1",
        "name": name,
        "networkName": "net-1",
        "port": port,
        "protocol": "rtp",
        "ttl": 64,
    }})
}

/// Create one cloud-to-ground (egress) bridge carrying a source and an output, and answer
/// its ARN with the `Bridge` the create stored. CreateBridge is `bridge_lifecycle`'s
/// subject; here it is only the fixture.
async fn fixture_bridge(
    emulator: &Emulator,
    name: &str,
    placement_arn: &str,
    flow_arn: &str,
) -> (String, Value) {
    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": name,
            "placementArn": placement_arn,
            "egressGatewayBridge": { "maxBitrate": 20_000_000 },
            "sources": [{ "flowSource": { "flowArn": flow_arn, "name": "src-a" } }],
            "outputs": [network_output("out-a", 6_100)],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "fixture {name:?}: create expected 201, got {status} {code:?}: {created}"
    );
    let arn = created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {name:?}: the create answers an ARN: {created}"))
        .to_string();
    (arn, created["bridge"].clone())
}

/// The summary ListBridges reports for `arn`, or `None` when the listing does not hold it.
async fn listed_bridge(emulator: &Emulator, case: &str, arn: &str) -> Option<Value> {
    let (status, code, listed) = emulator.send("GET", "/v1/bridges", None).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the listing expects 200, got {status} {code:?}: {listed}"
    );
    listed["bridges"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|summary| summary["bridgeArn"].as_str() == Some(arn))
        .cloned()
}

/// A bridge whose deletion has been accepted is still reported — by the single read and by
/// the listing, in the state it is in and with the configuration it was left with — at every
/// stage of that retirement.
#[tokio::test]
async fn a_retired_bridge_is_still_reported_in_the_state_it_is_in() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-retire-read", &["net-1"]).await;
    let flow_arn = cloud_flow(&emulator, "flow-retire-read").await;
    let (arn, created) = fixture_bridge(&emulator, "bridge-retire-read", &placement, &flow_arn)
        .await;

    tokio::time::sleep(settle_span()).await;
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

    // The transient state the deletion is in flight in, then the terminal one the clock
    // settles it in: the record is readable in both.
    for expected_state in [mc::BRIDGE_STATE_DELETING, mc::BRIDGE_STATE_DELETED] {
        let (status, code, described) = describe_bridge(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {expected_state:?}: a retired bridge is still readable, got {status} \
             {code:?}: {described}"
        );
        let bridge = described["bridge"].clone();
        assert_eq!(
            bridge["bridgeState"].as_str(),
            Some(expected_state),
            "case {expected_state:?}: the read reports the state the bridge is in, got {bridge}"
        );
        for member in ["bridgeArn", "name", "placementArn", "sources", "outputs"] {
            assert_eq!(
                bridge[member], created[member],
                "case {expected_state:?}: a retirement moves no configuration, so {member} is \
                 reported as it was stored, got {:?}",
                bridge[member]
            );
        }

        let summary = listed_bridge(&emulator, expected_state, &arn)
            .await
            .unwrap_or_else(|| {
                panic!("case {expected_state:?}: the listing still holds the retired bridge")
            });
        assert_eq!(
            summary,
            json!({
                "bridgeArn": arn,
                "bridgeState": expected_state,
                // The kind of bridge the create named, read through the crate's own constant
                // (`BridgeType`) rather than spelled out here.
                "bridgeType": mc::BRIDGE_TYPE_EGRESS,
                "name": created["name"],
                "placementArn": created["placementArn"],
            }),
            "case {expected_state:?}: the listing reports the stored record's own summary, \
             got {summary}"
        );

        tokio::time::sleep(settle_span()).await;
    }
}

/// DeleteBridge removes the RECORD rather than changing it, so a retired bridge still takes
/// it — from either state a retirement reaches — and afterwards it is no longer a resource
/// at all.
#[tokio::test]
async fn a_retired_bridge_is_still_deletable() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-retire-delete", &["net-1"]).await;
    let flow_arn = cloud_flow(&emulator, "flow-retire-delete").await;

    for (index, bridge_state) in [mc::BRIDGE_STATE_DELETING, mc::BRIDGE_STATE_DELETED]
        .into_iter()
        .enumerate()
    {
        let (arn, _) = fixture_bridge(
            &emulator,
            &format!("bridge-retire-delete-{index}"),
            &placement,
            &flow_arn,
        )
        .await;
        force_bridge_state(&emulator, &arn, bridge_state);

        let (status, code, deleted) = emulator
            .send("DELETE", &format!("/v1/bridges/{arn}"), None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {bridge_state:?}: a retired bridge is still deletable, got {status} \
             {code:?}: {deleted}"
        );
        assert_eq!(
            deleted["bridgeArn"].as_str(),
            Some(arn.as_str()),
            "case {bridge_state:?}: the answer names the record that was removed, got {deleted}"
        );

        let (status, code, gone) = describe_bridge(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {bridge_state:?}: the removed record is gone, got {status} {code:?}: {gone}"
        );
        assert_eq!(
            listed_bridge(&emulator, bridge_state, &arn).await,
            None,
            "case {bridge_state:?}: the listing no longer holds the removed record"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! Growing a bridge's outputs stops when the bridge's deletion is accepted, end to end over
//! HTTP: CreateBridge -> AddBridgeOutputs -> DescribeBridge -> UpdateBridgeState ->
//! AddBridgeOutputs -> DescribeBridge.
//!
//! The rule under test: a bridge whose move to the `DELETED` member of `DesiredState` has
//! been accepted is RETIRED — it reports `DELETING` while that deletion is in flight and
//! then the terminal `DELETED` the clock settles it in, and from the instant the move is
//! accepted no request may write to it, because the deletion it is settling into is
//! irreversible. AddBridgeOutputs is such a write, so it is refused with
//! `ConflictException` (HTTP 409) and the outputs DescribeBridge reports afterwards are
//! exactly the ones that were stored before the refused call.
//!
//! The refusal is attributed to the BRIDGE's state rather than to the request: every row
//! below sends a request of the same shape twice, once while the bridge is still live — where
//! it is accepted and the outputs it added are read back through DescribeBridge — and once
//! after the deletion has been accepted.
//!
//! The rows vary what the rule generalises over, which for this operation is the size of the
//! change it asks for: how many outputs the bridge ALREADY holds, and how many outputs one
//! request carries. Both stay well inside the bound a bridge's outputs are held to
//! (`mc::MAX_BRIDGE_OUTPUTS`), so no row's refusal could come from the count instead. The
//! retirement itself is observed at both of its states, reached by the clock and not forced.
//!
//! `bridge_retirement_refuses_changes` is the suite that spans every bridge-writing
//! operation against every value of the modelled `BridgeState` enum; this one stays on the
//! output-growing path and varies the request instead.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::bridge_members_and_deletion::{add_outputs, network_output};
use crate::flow_lifecycle::Emulator;

/// Long enough for the clock to have made a transition due: the span the service holds a
/// bridge in a transient state (`mc::TRANSITION_SPAN`) plus slack for a slow runner.
fn settle_span() -> Duration {
    mc::TRANSITION_SPAN + Duration::from_millis(400)
}

/// `count` network outputs named apart by `prefix`, as a request configures them: the kind of
/// output a cloud-to-ground bridge delivers to the premises network.
fn outputs(prefix: &str, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            network_output(
                &format!("{prefix}-{index}"),
                6_000 + index as i64,
                "udp",
                32,
            )
        })
        .collect()
}

/// One row: how many outputs the bridge is created holding, and how many outputs one
/// AddBridgeOutputs request carries.
struct Row {
    case: &'static str,
    held: usize,
    adding: usize,
}

/// The table. Every row differs from the others in the size of the change the request asks
/// for, never only in a name.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a bridge holding no output, one output added",
            held: 0,
            adding: 1,
        },
        Row {
            case: "a bridge already holding one output, one output added",
            held: 1,
            adding: 1,
        },
        Row {
            case: "a bridge already holding two outputs, a batch of three added",
            held: 2,
            adding: 3,
        },
    ]
}

/// Create one cloud-to-ground (egress) bridge holding `held` outputs and answer its ARN.
///
/// A cloud-to-ground bridge is the one whose content "comes from an existing MediaConnect
/// flow and is delivered to your premises", so it is the bridge that takes the network
/// outputs a request configures. CreateBridge is `bridge_lifecycle`'s subject; here it is
/// only the fixture.
async fn fixture_bridge(
    emulator: &Emulator,
    name: &str,
    placement_arn: &str,
    flow_arn: &str,
    held: usize,
) -> String {
    let mut body = json!({
        "name": name,
        "placementArn": placement_arn,
        "egressGatewayBridge": { "maxBitrate": 20_000_000 },
        "sources": [{ "flowSource": { "flowArn": flow_arn, "name": "src-a" } }],
    });
    if held > 0 {
        body["outputs"] = json!(outputs("held", held));
    }
    let (status, code, created) = create_bridge(emulator, body).await;
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
/// bridge stays READABLE, which is what makes the refusal observable.
async fn bridge_of(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, described) = describe_bridge(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
    );
    described["bridge"].clone()
}

/// How many outputs a described bridge holds. A bridge holding none reports the member not
/// at all, which is no outputs.
fn output_count(bridge: &Value) -> usize {
    bridge["outputs"].as_array().map_or(0, Vec::len)
}

/// AddBridgeOutputs is accepted while the bridge is live and refused once its deletion has
/// been accepted — for the same request, and leaving the outputs it was refused on untouched.
#[tokio::test]
async fn adding_outputs_stops_when_the_bridge_is_retired() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-add-retired", &["net-1"]).await;
    let flow_arn = cloud_flow(&emulator, "flow-add-retired").await;

    for (index, row) in rows().into_iter().enumerate() {
        let Row { case, held, adding } = row;
        assert!(
            held + adding <= mc::MAX_BRIDGE_OUTPUTS,
            "case {case:?}: the row must stay inside the output bound {}, got {}",
            mc::MAX_BRIDGE_OUTPUTS,
            held + adding
        );
        let arn = fixture_bridge(
            &emulator,
            &format!("bridge-add-retired-{index}"),
            &placement,
            &flow_arn,
            held,
        )
        .await;

        // The bridge the create left in a transient state settles into the one a created
        // bridge waits in, and that LIVE bridge takes the request.
        tokio::time::sleep(settle_span()).await;
        let live = outputs("live", adding);
        let (status, code, answered) =
            add_outputs(&emulator, &arn, json!({ "outputs": live.clone() })).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: a live bridge takes the outputs, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered["outputs"],
            json!(live),
            "case {case:?}: the answer carries the outputs added, got {answered}"
        );
        let stored = bridge_of(&emulator, case, &arn).await;
        assert_eq!(
            output_count(&stored),
            held + adding,
            "case {case:?}: the bridge holds the outputs it was created with plus the added \
             ones, got {stored}"
        );

        // From here the bridge's deletion is accepted, and nothing below may move any of the
        // outputs this snapshot holds.
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
            "case {case:?}: the move to DELETED is accepted, got {status} {code:?}: {accepted}"
        );
        let snapshot = stored["outputs"].clone();

        // Twice over: while the deletion is in flight (`DELETING`), and once the clock has
        // settled the terminal `DELETED`.
        for expected_state in [mc::BRIDGE_STATE_DELETING, mc::BRIDGE_STATE_DELETED] {
            let case = format!("{case} / {expected_state}");
            let retiring = bridge_of(&emulator, &case, &arn).await;
            assert_eq!(
                retiring["bridgeState"].as_str(),
                Some(expected_state),
                "case {case:?}: the retired bridge is readable in the state it is in, got \
                 {retiring}"
            );

            let refused_outputs = outputs(&format!("after-{expected_state}"), adding);
            let (status, code, refused) =
                add_outputs(&emulator, &arn, json!({ "outputs": refused_outputs })).await;
            assert_eq!(
                (status, code.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: adding outputs to a retired bridge expects 409 \
                 ConflictException, got {status} {code:?}: {refused}"
            );
            let message = refused["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&arn) && message.contains(expected_state),
                "case {case:?}: the refusal names the bridge and the state it is in, got \
                 {refused}"
            );

            let after = bridge_of(&emulator, &case, &arn).await;
            assert_eq!(
                after["outputs"], snapshot,
                "case {case:?}: the refused add left the bridge's outputs as they were stored, \
                 got {after}"
            );
            assert_eq!(
                output_count(&after),
                held + adding,
                "case {case:?}: a refused add grows nothing, got {after}"
            );
            tokio::time::sleep(settle_span()).await;
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! UpdateBridgeState, end to end over HTTP: CreateBridge -> UpdateBridgeState ->
//! DescribeBridge (and DeleteBridge, whose documented precondition reads the state this
//! operation writes).
//!
//! The rule under test: UpdateBridgeState asks the STORED bridge to reach the state
//! `DesiredState` names — the bridge enters the transient `BridgeState` that reports that
//! move being in progress at once and the CLOCK owes it the desired state one transition
//! span later — and the answer carries the stored bridge's ARN and the state it is now
//! headed for, so DescribeBridge afterwards reports the transition this call started. A
//! bridge already in the desired state, or already settling into it, is left exactly as it
//! is (`@idempotent`), and a bridge that has reached the terminal `DELETED` state cannot be
//! asked for another one.
//!
//! The rows below vary what that rule generalises over: EVERY value of the modelled
//! `DesiredState` enum (the catalog is `DESIRED_STATE_VALUES`, generated from the model),
//! the state the bridge starts from (a bridge waiting in `STANDBY` and a running one), and
//! the bridge type the request addresses (egress / ingress).
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::DESIRED_STATE_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{create_bridge, describe_bridge, minted_gateway_arn};
use crate::flow_lifecycle::{Emulator, REGION};

/// Long enough for the clock to have made a transition due: the span the service holds a
/// bridge in a transient state (`mc::TRANSITION_SPAN`) plus slack for a slow runner.
fn settle_span() -> Duration {
    mc::TRANSITION_SPAN + Duration::from_millis(400)
}

async fn update_bridge_state(
    emulator: &Emulator,
    arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send("PUT", &format!("/v1/bridges/{arn}/state"), Some(body))
        .await
}

async fn delete_bridge(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/bridges/{arn}"), None)
        .await
}

/// One network source, the kind a ground-to-cloud bridge ingests ("the content originates
/// at your premises"), so an ingress create below carries the one source CreateBridge
/// requires.
fn network_source(name: &str, port: i64) -> Value {
    json!({ "networkSource": {
        "multicastIp": "224.0.0.1",
        "name": name,
        "networkName": "net-1",
        "port": port,
        "protocol": "rtp",
    }})
}

/// One flow source, the kind a cloud-to-ground bridge ingests ("the content comes from an
/// existing MediaConnect flow"), so an egress create below carries the one source
/// CreateBridge requires.
fn flow_source(name: &str, suffix: u64) -> Value {
    json!({ "flowSource": {
        "flowArn": crate::bridge_lifecycle::minted_flow_arn(suffix, "flow-bridge-state"),
        "name": name,
    }})
}

/// A bridge ARN of the form this service mints, built through the crate's own encoding of
/// it — what an `@httpLabel BridgeArn` must look like (`^arn:.+:mediaconnect.+:bridge:.+$`)
/// for a request to reach the handler at all.
fn minted_bridge_arn(suffix: u64, name: &str) -> String {
    engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::bridge_resource_fragment(suffix, name),
    )
}

/// The bridge-type member a create carries, and the settings for it: a bridge is
/// cloud-to-ground (egress) or ground-to-cloud (ingress).
fn bridge_type_member(egress: bool) -> (&'static str, Value) {
    if egress {
        ("egressGatewayBridge", json!({ "maxBitrate": 60_000_000 }))
    } else {
        (
            "ingressGatewayBridge",
            json!({ "maxBitrate": 20_000_000, "maxOutputs": 4 }),
        )
    }
}

/// Create one bridge and answer its ARN, with the transition the create owes already
/// settled — so each case below starts from a bridge waiting in `STANDBY`, the state a
/// created bridge settles into.
async fn settled_bridge(emulator: &Emulator, name: &str, egress: bool, port: i64) -> String {
    let (member, settings) = bridge_type_member(egress);
    let source = if egress {
        flow_source("src", port as u64)
    } else {
        network_source("src", port)
    };
    let mut body = json!({
        "name": name,
        "placementArn": minted_gateway_arn(1, "gw-bridge-state"),
        "sources": [source],
    });
    body[member] = settings;

    let (status, code, created) = create_bridge(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the bridge fixture {name:?} expects 201, got {status} {code:?}: {created}"
    );
    let arn = created["bridge"]["bridgeArn"]
        .as_str()
        .expect("the create answers a bridge ARN")
        .to_string();

    tokio::time::sleep(settle_span()).await;
    let (_, _, described) = describe_bridge(emulator, &arn).await;
    assert_eq!(
        described["bridge"]["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_STANDBY),
        "the bridge fixture {name:?} settles into STANDBY, got {described}"
    );
    arn
}

/// One row of the UpdateBridgeState table: the state to ask for, the transient state the
/// bridge must report while that move is in progress, an optional desired state to settle
/// the bridge into FIRST (so the row starts from a running bridge rather than a waiting
/// one), and the bridge type to address.
struct StateCase {
    case: &'static str,
    /// A `DesiredState` value; the catalog is `DESIRED_STATE_VALUES`.
    desired: &'static str,
    /// The `BridgeState` the bridge reports while it is moving to [`Self::desired`].
    transient: &'static str,
    /// Applied and settled before the case's own call, or `None` to start from `STANDBY`.
    start_from: Option<&'static str>,
    egress: bool,
}

/// Every value of the modelled `DesiredState` enum, each asked for from a different
/// starting state and on a different bridge type.
const STATE_CASES: [StateCase; 4] = [
    StateCase {
        case: "a waiting bridge is asked to become ACTIVE",
        desired: "ACTIVE",
        transient: mc::BRIDGE_STATE_STARTING,
        start_from: None,
        egress: true,
    },
    StateCase {
        case: "a running bridge is asked back to STANDBY",
        desired: "STANDBY",
        transient: mc::BRIDGE_STATE_STOPPING,
        start_from: Some("ACTIVE"),
        egress: false,
    },
    StateCase {
        case: "a waiting bridge is asked to become DELETED",
        desired: "DELETED",
        transient: mc::BRIDGE_STATE_DELETING,
        start_from: None,
        egress: false,
    },
    StateCase {
        case: "a running bridge is asked to become DELETED",
        desired: "DELETED",
        transient: mc::BRIDGE_STATE_DELETING,
        start_from: Some("ACTIVE"),
        egress: true,
    },
];

/// The table above must span the modelled enum: every value `DesiredState` declares is
/// asked for by some row, and no row asks for a value it does not declare.
#[test]
fn the_table_covers_every_modelled_desired_state() {
    for value in DESIRED_STATE_VALUES {
        assert!(
            STATE_CASES.iter().any(|case| &case.desired == value),
            "case {value:?}: every DesiredState value must be exercised, got the rows {:?}",
            STATE_CASES.iter().map(|case| case.desired).collect::<Vec<_>>()
        );
    }
    for case in &STATE_CASES {
        assert!(
            DESIRED_STATE_VALUES.contains(&case.desired),
            "case {:?}: {:?} is not a value the DesiredState enum declares, which are {:?}",
            case.case,
            case.desired,
            DESIRED_STATE_VALUES
        );
    }
}

/// A desired state is accepted, reported back off the stored record, and reached through
/// the transient state the clock settles — for every value of the modelled enum.
#[tokio::test]
async fn update_bridge_state_moves_the_bridge_to_the_state_it_was_asked_for() {
    let emulator = Emulator::new();

    for (index, case) in STATE_CASES.iter().enumerate() {
        let StateCase {
            case: name_of_case,
            desired,
            transient,
            start_from,
            egress,
        } = case;
        let arn = settled_bridge(
            &emulator,
            &format!("bridge-state-{index}"),
            *egress,
            5000 + index as i64,
        )
        .await;

        // The row's starting state, settled, so the call under test starts from a bridge
        // the clock owes nothing.
        if let Some(start_from) = start_from {
            let (status, code, started) =
                update_bridge_state(&emulator, &arn, json!({ "desiredState": start_from })).await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {name_of_case:?}: the starting state {start_from:?} expects 202, got \
                 {status} {code:?}: {started}"
            );
            tokio::time::sleep(settle_span()).await;
            let (_, _, described) = describe_bridge(&emulator, &arn).await;
            assert_eq!(
                described["bridge"]["bridgeState"].as_str(),
                Some(*start_from),
                "case {name_of_case:?}: the bridge settles into its starting state \
                 {start_from:?}, got {described}"
            );
        }

        let (status, code, answered) =
            update_bridge_state(&emulator, &arn, json!({ "desiredState": desired })).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {name_of_case:?}: expected 202 Accepted, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered,
            json!({ "bridgeArn": arn, "desiredState": desired }),
            "case {name_of_case:?}: the answer is the stored bridge's ARN and the state it is \
             headed for, got {answered}"
        );

        // A caller reading the bridge straight afterwards still sees the move in progress.
        let (status, code, described) = describe_bridge(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {name_of_case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        assert_eq!(
            described["bridge"]["bridgeState"].as_str(),
            Some(*transient),
            "case {name_of_case:?}: the bridge reports the transient state {transient:?} while \
             the move is in progress, got {described}"
        );

        // The clock settles the bridge into the state it was asked for, and nothing else
        // about the bridge moves.
        tokio::time::sleep(settle_span()).await;
        let (status, code, settled) = describe_bridge(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {name_of_case:?}: describe after the transition expected 200, got {status} \
             {code:?}: {settled}"
        );
        assert_eq!(
            settled["bridge"]["bridgeState"].as_str(),
            Some(*desired),
            "case {name_of_case:?}: the clock settles the bridge into {desired:?}, got {settled}"
        );
        for member in ["bridgeArn", "name", "placementArn", "sources", "outputs"] {
            assert_eq!(
                settled["bridge"][member], described["bridge"][member],
                "case {name_of_case:?}: {member:?} is untouched by a state change, got {} then \
                 {}",
                described["bridge"][member], settled["bridge"][member]
            );
        }
    }
}

/// `@idempotent`: asking for the state a bridge is already settling into changes nothing —
/// the same answer, and the transition the first call started keeps its own deadline rather
/// than being restarted.
#[tokio::test]
async fn asking_again_for_the_same_state_restarts_nothing() {
    let emulator = Emulator::new();
    let arn = settled_bridge(&emulator, "bridge-idempotent", true, 5100).await;

    let (status, _, first) =
        update_bridge_state(&emulator, &arn, json!({ "desiredState": "ACTIVE" })).await;
    assert_eq!(status, 202, "the first call expects 202, got {first}");

    // Most of the way through the transition the first call started, then the same request
    // again: it must neither restart the move nor push its deadline out.
    tokio::time::sleep(mc::TRANSITION_SPAN - Duration::from_millis(100)).await;
    let (status, _, second) =
        update_bridge_state(&emulator, &arn, json!({ "desiredState": "ACTIVE" })).await;
    assert_eq!(status, 202, "the repeated call expects 202, got {second}");
    assert_eq!(
        second, first,
        "case \"repeated ACTIVE\": a repeated call answers the same thing, got {first} then \
         {second}"
    );

    // Past the FIRST call's deadline, but short of a deadline a restart would have set.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, _, described) = describe_bridge(&emulator, &arn).await;
    assert_eq!(
        described["bridge"]["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_ACTIVE),
        "case \"repeated ACTIVE\": the repeated call left the first transition's deadline \
         alone, got {described}"
    );

    // And once settled, asking for the state the bridge is already in is still accepted and
    // still changes nothing.
    let (status, code, again) =
        update_bridge_state(&emulator, &arn, json!({ "desiredState": "ACTIVE" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case \"settled ACTIVE\": expected 202, got {status} {code:?}: {again}"
    );
    assert_eq!(
        again, first,
        "case \"settled ACTIVE\": the answer is the stored state, got {again}"
    );
    let (_, _, unchanged) = describe_bridge(&emulator, &arn).await;
    assert_eq!(
        unchanged["bridge"], described["bridge"],
        "case \"settled ACTIVE\": a no-op call leaves the bridge byte for byte as it was, got \
         {unchanged}"
    );
}

/// A bridge that has reached the terminal `DELETED` state cannot be asked for another
/// state: the move is irreversible, which is the conflict `ConflictException` (HTTP 409)
/// reports. Asking for `DELETED` again is the idempotent no-op.
#[tokio::test]
async fn a_deleted_bridge_cannot_be_asked_for_another_state() {
    let emulator = Emulator::new();

    for (index, desired) in DESIRED_STATE_VALUES.iter().enumerate() {
        let arn = settled_bridge(
            &emulator,
            &format!("bridge-deleted-{index}"),
            index % 2 == 0,
            5200 + index as i64,
        )
        .await;
        let (status, _, deleted) =
            update_bridge_state(&emulator, &arn, json!({ "desiredState": "DELETED" })).await;
        assert_eq!(
            status, 202,
            "case {desired:?}: asking for DELETED expects 202, got {deleted}"
        );
        tokio::time::sleep(settle_span()).await;

        let (status, code, answered) =
            update_bridge_state(&emulator, &arn, json!({ "desiredState": desired })).await;
        if *desired == "DELETED" {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {desired:?}: asking a deleted bridge for DELETED is the no-op, got \
                 {status} {code:?}: {answered}"
            );
            assert_eq!(
                answered,
                json!({ "bridgeArn": arn, "desiredState": "DELETED" }),
                "case {desired:?}: the answer is the stored state, got {answered}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (409, Some("ConflictException")),
                "case {desired:?}: a deleted bridge cannot be returned to {desired:?}, got \
                 {status} {code:?}: {answered}"
            );
        }

        // Either way the bridge is still readable in its terminal state: `BridgeState`
        // declares `DELETED`, so a bridge asked for it is reported in it.
        let (status, _, described) = describe_bridge(&emulator, &arn).await;
        assert_eq!(
            (status, described["bridge"]["bridgeState"].as_str()),
            (200, Some(mc::BRIDGE_STATE_DELETED)),
            "case {desired:?}: the bridge stays readable in DELETED, got {status}: {described}"
        );
    }
}

/// One request per way an UpdateBridgeState call is refused before it changes anything, and
/// the refusal each one answers.
#[tokio::test]
async fn a_refused_update_bridge_state_request_changes_nothing() {
    let emulator = Emulator::new();
    let arn = settled_bridge(&emulator, "bridge-refusals", false, 5300).await;
    let unknown = minted_bridge_arn(404, "never-created");

    // A bridge ARN no store holds: "one or more of the resources in the request does not
    // exist in the system".
    let (status, code, answered) =
        update_bridge_state(&emulator, &unknown, json!({ "desiredState": "ACTIVE" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "case \"unknown bridge\": expected 404 NotFoundException, got {status} {code:?}: \
         {answered}"
    );
    assert_eq!(
        answered["message"].as_str(),
        Some(mc::not_found_message(&unknown).as_str()),
        "case \"unknown bridge\": the message names the identifier the request carried, got \
         {answered}"
    );

    // A value outside the modelled enum, and a request carrying no desired state at all:
    // both are refused as the request's content, in this service's validation envelope.
    for (case, body) in [
        ("a state outside the enum", json!({ "desiredState": "RUNNING" })),
        ("a lower-cased enum value", json!({ "desiredState": "active" })),
        ("no desired state at all", json!({})),
    ] {
        let (status, code, answered) = update_bridge_state(&emulator, &arn, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {answered}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: a refusal about the request's content carries the validation \
             envelope, got {message:?}"
        );
    }

    // Nothing above moved the bridge: it is still waiting where its create left it.
    let (_, _, described) = describe_bridge(&emulator, &arn).await;
    assert_eq!(
        described["bridge"]["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_STANDBY),
        "a refused call leaves the bridge in the state it was in, got {described}"
    );
}

/// The state UpdateBridgeState writes is the state DeleteBridge's documented precondition
/// reads ("before you can delete a bridge, you must stop the bridge"): a bridge asked to
/// run is not deletable, and one asked back to `STANDBY` is.
#[tokio::test]
async fn a_bridge_asked_to_run_is_not_deletable_until_it_is_asked_back_to_standby() {
    let emulator = Emulator::new();
    let arn = settled_bridge(&emulator, "bridge-delete-precondition", true, 5400).await;

    let (status, _, started) =
        update_bridge_state(&emulator, &arn, json!({ "desiredState": "ACTIVE" })).await;
    assert_eq!(status, 202, "asking for ACTIVE expects 202, got {started}");

    // While it is starting, and once it is running, the delete is refused.
    for case in ["starting", "running"] {
        let (status, code, refused) = delete_bridge(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a bridge that was not stopped is refused, got {status} {code:?}: \
             {refused}"
        );
        if case == "starting" {
            tokio::time::sleep(settle_span()).await;
        }
    }

    let (status, _, stopped) =
        update_bridge_state(&emulator, &arn, json!({ "desiredState": "STANDBY" })).await;
    assert_eq!(status, 202, "asking for STANDBY expects 202, got {stopped}");
    tokio::time::sleep(settle_span()).await;

    let (status, code, deleted) = delete_bridge(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case \"stopped\": a bridge asked back to STANDBY is deletable, got {status} {code:?}: \
         {deleted}"
    );
    let (status, code, gone) = describe_bridge(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "case \"stopped\": the deleted bridge is gone, got {status} {code:?}: {gone}"
    );
}

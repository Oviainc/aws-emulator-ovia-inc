// SPDX-License-Identifier: CC0-1.0

//! RestartRouterOutput end to end over HTTP: `POST /v1/routerOutput/restart/{Arn}`.
//!
//! The rule the cases below pin: a restart addresses ONE stored router output by ARN and
//! acts on the state that output is AT REST in — it puts the output into the transient
//! `RECOVERING` state and the clock owes it, one transition span later, the state the
//! restart resumes it in (`ACTIVE` for an output that was carrying content or had failed
//! while meant to, `STANDBY` for one that was never started) — while an output that
//! already has a change in flight, or is on its way out, is refused with the modelled
//! `ConflictException` (409) and left exactly as it was, and an ARN the store holds no
//! output for with `NotFoundException` (404).
//!
//! The rows vary what that rule generalises over: EVERY value of the state enum the model
//! declares for a router output, taken from the catalog this crate GENERATES from the model
//! (`ops::types::ROUTER_OUTPUT_STATE_VALUES`), the states a create and a restart of an
//! output actually reach over the wire, repeating the call while the restart it started is
//! still in flight, and several ARNs the store holds nothing for.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization. Every router output here
//! is CREATED over the wire; a state no implemented operation can put one in yet is then
//! written onto that created record through the shared store, which is the one member each
//! such case is about.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::ROUTER_OUTPUT_STATE_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_outputs::minted_network_interface;

/// Longer than `mc::TRANSITION_SPAN`, so a transition the clock owes is certainly due.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// An ARN satisfying the modelled `RouterOutputArn` pattern
/// (`smithy_query(query="shape",shape="RouterOutputArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerOutput:[a-z0-9]{12}$`)
/// that no create minted, so no store holds a router output for it.
const UNKNOWN_ARN: &str = "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:ffffffffffff";
/// A second absent one, with an id of digits only.
const OTHER_UNKNOWN_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:000000000042";
/// A third, in another Region and another partition — still the modelled pattern, still
/// nothing this store holds.
const FOREIGN_UNKNOWN_ARN: &str =
    "arn:aws-cn:mediaconnect:cn-north-1:000000000000:routerOutput:abcdefabcdef";

/// The states the restart's rule names, taken from the constants `crate::state` owns for
/// this family so the suite and the handler cannot spell one state two ways; each is also
/// asserted against the model-generated catalog by
/// [`restart_resumes_a_resting_output_and_conflicts_with_a_change_in_flight`].
const STATE_ACTIVE: &str = mc::ROUTER_OUTPUT_STATE_ACTIVE;
/// See [`STATE_ACTIVE`].
const STATE_ERROR: &str = mc::ROUTER_OUTPUT_STATE_ERROR;
/// See [`STATE_ACTIVE`].
const STATE_RECOVERING: &str = mc::ROUTER_OUTPUT_STATE_RECOVERING;

async fn create_router_output(emulator: &Emulator, body: Value) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/routerOutput", Some(body)).await
}

async fn restart_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/routerOutput/restart/{arn}"), None)
        .await
}

/// GetRouterOutput, the single-ARN read side (`GET /v1/routerOutput/{Arn}`).
async fn get_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

/// A create body for a STANDARD router output on the router network interface
/// `interface_arn` — a configuration must name an interface this account holds, so the
/// suites mint one first; `destination_port` varies so each case drives a record of its own.
fn create_body(interface_arn: &str, name: &str, destination_port: u16) -> Value {
    json!({
        "name": name,
        "configuration": {
            "Standard": {
                "networkInterfaceArn": interface_arn,
                "protocolConfiguration": {
                    "Rtp": {
                        "destinationAddress": "198.51.100.90",
                        "destinationPort": destination_port,
                    }
                }
            }
        },
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    })
}

/// Create a router output over the wire and answer its ARN.
async fn created_router_output(
    emulator: &Emulator,
    interface_arn: &str,
    name: &str,
    destination_port: u16,
) -> String {
    let (status, code, created) =
        create_router_output(emulator, create_body(interface_arn, name, destination_port)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the create must be accepted, got {status} {code:?}: {created}"
    );
    created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create must report an ARN, got {created}"))
        .to_string()
}

/// Create a router output over the wire and put the stored record in `state`, settled —
/// the state no implemented operation can reach yet is the ONE member written here, on a
/// record a real create made.
async fn router_output_in_state(
    emulator: &Emulator,
    interface_arn: &str,
    name: &str,
    destination_port: u16,
    state: &str,
) -> String {
    let arn = created_router_output(emulator, interface_arn, name, destination_port).await;
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let record = store
        .router_outputs
        .get_mut(&arn)
        .unwrap_or_else(|| panic!("the store must hold the created router output {arn}"));
    record.state = state.to_string();
    record.pending = None;
    arn
}

/// The stored record of the router output `arn` addresses, cloned out of the shared store.
///
/// The wire assertions below are the subject; this is read only where the WIRE form is
/// coarser than the record — a reported timestamp is epoch seconds, while the stored one
/// carries the clock's full resolution.
fn stored_record(emulator: &Emulator, arn: &str) -> mc::StoredRouterOutput {
    emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .router_outputs
        .get(arn)
        .cloned()
        .unwrap_or_else(|| panic!("the store must hold a router output for {arn}"))
}

/// How many router outputs the store holds.
fn stored_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .router_outputs
        .len()
}

/// The whole reported record of the one router output `arn` addresses, read over the wire.
async fn read_record(emulator: &Emulator, arn: &str) -> Value {
    let (status, code, read) = get_router_output(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "a stored router output must be readable, got {status} {code:?}: {read}"
    );
    read["routerOutput"].clone()
}

/// The reported state of the one router output `arn` addresses.
async fn read_state(emulator: &Emulator, arn: &str) -> String {
    read_record(emulator, arn).await["state"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The state a restart resumes an output in, or `None` when the restart must be refused as
/// a conflict with the change the output already has in flight. The rule, stated here
/// independently of the handler: an output is at REST in `STANDBY`, `ACTIVE` or `ERROR`,
/// and a restart puts it back in the state in which it is meant to carry content.
fn expected_resume_state(state: &str) -> Option<&'static str> {
    match state {
        STATE_ACTIVE | STATE_ERROR => Some(STATE_ACTIVE),
        mc::ROUTER_OUTPUT_STATE_STANDBY => Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
        _ => None,
    }
}

#[tokio::test]
async fn restart_resumes_a_resting_output_and_conflicts_with_a_change_in_flight() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-restart").await;

    // The states the rule names are the model's own values: the generated catalog is the
    // source of this table, so a value the model gains or loses shows up here.
    for named in [STATE_ACTIVE, STATE_ERROR, STATE_RECOVERING] {
        assert!(
            ROUTER_OUTPUT_STATE_VALUES.contains(&named),
            "the rule names {named:?}, which must be a RouterOutputState the model declares, \
             got {ROUTER_OUTPUT_STATE_VALUES:?}"
        );
    }

    for (index, state) in ROUTER_OUTPUT_STATE_VALUES.iter().enumerate() {
        let case = *state;
        let port = 5_000 + u16::try_from(index).unwrap_or_default();
        let arn = router_output_in_state(
            &emulator,
            &interface_arn,
            &format!("ro-restart-{case}"),
            port,
            case,
        )
        .await;
        let before = read_record(&emulator, &arn).await;
        assert_eq!(
            before["state"].as_str(),
            Some(case),
            "case {case:?}: the output must start in {case}, got {:?}",
            before["state"]
        );
        let name = before["name"].as_str().unwrap_or_default().to_string();
        let stored_before = stored_count(&emulator);

        let (status, code, body) = restart_router_output(&emulator, &arn).await;

        let Some(resumed) = expected_resume_state(case) else {
            assert_eq!(
                (status, code.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: a restart of an output in {case} is refused as a conflict, \
                 got {status} {code:?}: {body}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(case),
                "case {case:?}: the refusal names the state the output is in, got {message:?}"
            );
            assert_eq!(
                read_record(&emulator, &arn).await,
                before,
                "case {case:?}: a refused restart leaves the stored output exactly as it was"
            );
            continue;
        };

        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: a restart of an output in {case} is accepted, got {status} \
             {code:?}: {body}"
        );
        assert_eq!(
            (body["arn"].as_str(), body["name"].as_str()),
            (Some(arn.as_str()), Some(name.as_str())),
            "case {case:?}: the response reports the stored output's ARN and name, got {body}"
        );
        assert_eq!(
            body["state"].as_str(),
            Some(STATE_RECOVERING),
            "case {case:?}: an accepted restart reports the transient RECOVERING state, \
             got {body}"
        );

        // A caller reading straight after the call still observes the transient state: the
        // resumed state is owed, not immediate. The record says so too — the clock owes it
        // the resumed state — which is what makes the wire reading above the one every
        // caller gets, however loaded the machine is.
        let in_flight = stored_record(&emulator, &arn);
        assert_eq!(
            (
                in_flight.state.as_str(),
                in_flight.pending.as_ref().map(|owed| owed.status.as_str())
            ),
            (STATE_RECOVERING, Some(resumed)),
            "case {case:?}: the restart leaves the output RECOVERING with {resumed} owed, \
             got {:?} and {:?}",
            in_flight.state,
            in_flight.pending
        );
        assert_eq!(
            read_state(&emulator, &arn).await,
            STATE_RECOVERING,
            "case {case:?}: an output read straight after the restart is still RECOVERING"
        );

        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let settled = read_record(&emulator, &arn).await;
        assert_eq!(
            settled["state"].as_str(),
            Some(resumed),
            "case {case:?}: the clock resumes the restarted output in {resumed}, got {:?}",
            settled["state"]
        );

        // The restart moved the output's state and the timestamp of its last update, and
        // nothing else about the record: every other member the read reports is the one
        // the store already held.
        for (member, value) in before.as_object().into_iter().flatten() {
            if member == "state" || member == "updatedAt" {
                continue;
            }
            assert_eq!(
                settled[member], *value,
                "case {case:?}: a restart leaves {member} as the store held it, got {:?}",
                settled[member]
            );
        }
        // The reported timestamp is epoch SECONDS, so the stored record is where the move
        // is visible: the restart ran after the record was created, and the clock has since
        // settled the state it owed.
        let record = stored_record(&emulator, &arn);
        assert!(
            record.updated_at > record.created_at,
            "case {case:?}: the restart is an update, so UpdatedAt moves past CreatedAt, \
             got {:?} after {:?}",
            record.updated_at,
            record.created_at
        );
        assert!(
            record.pending.is_none(),
            "case {case:?}: the resumed state is settled, so nothing is owed, got {:?}",
            record.pending
        );
        assert_eq!(
            stored_count(&emulator),
            stored_before,
            "case {case:?}: a restart mints and removes no router output, got {} stored",
            stored_count(&emulator)
        );
    }
}

#[tokio::test]
async fn a_restart_over_the_wire_waits_for_the_states_the_clock_owes() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-restart-wire").await;
    let arn = created_router_output(&emulator, &interface_arn, "ro-restart-wire", 5004).await;

    // The creation is still in flight, so the restart conflicts with it.
    let (status, code, refused) = restart_router_output(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "a restart of an output that is still CREATING is refused, got {status} {code:?}: \
         {refused}"
    );

    // Once the clock has settled the create, the same call is accepted.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    assert_eq!(
        read_state(&emulator, &arn).await,
        mc::ROUTER_OUTPUT_STATE_STANDBY,
        "the clock settles a new output into STANDBY before the restart"
    );
    let (status, code, accepted) = restart_router_output(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref(), accepted["state"].as_str()),
        (202, None, Some(STATE_RECOVERING)),
        "a restart of a settled output is accepted and reports RECOVERING, got {status} \
         {code:?}: {accepted}"
    );

    // And a second restart, while the first is still in flight, conflicts with it.
    let (status, code, again) = restart_router_output(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "a restart of an output that is already RECOVERING is refused, got {status} \
         {code:?}: {again}"
    );

    // A restart does not START the output: one that was never started rests in STANDBY
    // again once the restart has run its course.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    assert_eq!(
        read_state(&emulator, &arn).await,
        mc::ROUTER_OUTPUT_STATE_STANDBY,
        "a restarted output that was never started resumes in STANDBY"
    );
}

#[tokio::test]
async fn restart_refuses_an_arn_the_store_holds_no_output_for() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-restart-absent").await;
    // One stored output, so each refusal below is about the ARN the request named rather
    // than about an empty store.
    let stored =
        created_router_output(&emulator, &interface_arn, "ro-restart-present", 5010).await;

    // An output the clock has already taken away is gone as well: DeleteRouterOutput
    // accepts the delete and the removal comes due one transition span later.
    let deleted =
        created_router_output(&emulator, &interface_arn, "ro-restart-deleted", 5011).await;
    let (status, code, _) = emulator
        .send("DELETE", &format!("/v1/routerOutput/{deleted}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the delete must be accepted, got {status} {code:?}"
    );
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for (case, arn) in [
        ("an id no create minted", UNKNOWN_ARN),
        ("an id of digits only", OTHER_UNKNOWN_ARN),
        ("another partition and Region", FOREIGN_UNKNOWN_ARN),
        ("an output the clock has already removed", deleted.as_str()),
    ] {
        let (status, code, body) = restart_router_output(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref(), body["message"].as_str()),
            (
                404,
                Some("NotFoundException"),
                Some(mc::not_found_message(arn).as_str())
            ),
            "case {case:?}: a restart of an ARN the store holds nothing for is refused as \
             not found, got {status} {code:?}: {body}"
        );
    }

    // None of the refusals touched the store, or the one output it still holds.
    assert_eq!(
        stored_count(&emulator),
        1,
        "a refused restart stores nothing, got {} stored",
        stored_count(&emulator)
    );
    assert_eq!(
        read_state(&emulator, &stored).await,
        mc::ROUTER_OUTPUT_STATE_STANDBY,
        "a refused restart leaves the outputs the store does hold alone"
    );
}

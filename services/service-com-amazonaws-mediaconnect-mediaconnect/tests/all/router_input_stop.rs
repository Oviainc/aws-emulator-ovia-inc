// SPDX-License-Identifier: CC0-1.0

//! StopRouterInput, end to end over HTTP: CreateRouterInput -> StopRouterInput ->
//! GetRouterInput.
//!
//! The rule the suite pins, over a table rather than over the one request that exposed it:
//! a stop acts on the STORED router input its `@httpLabel Arn` addresses and is accepted
//! whenever that record is settled (the clock owes it no state change and no accepted
//! deletion), answering the record's own ARN and name with the non-terminal `STOPPING`
//! state, which the clock then settles into `STANDBY` one `mc::TRANSITION_SPAN` later; an
//! input that is already stopped or already stopping keeps the state it is in and the call
//! persists nothing; one whose clock still owes a different change is refused with the
//! modelled `ConflictException` (409); and an ARN the store holds no router input for is the
//! modelled `NotFoundException` (404), while an ARN outside the modelled `RouterInputArn`
//! pattern is refused at the boundary with `BadRequestException` (400).
//!
//! The cases vary what that rule generalises over: which `RouterInputConfiguration` variant
//! (and so which input type) the input carries, the settled state the stop arrived in, how
//! many router inputs the store holds beside the addressed one, how many times the same
//! input is stopped, and whether the clock has settled the stop yet.
//!
//! The configuration table, the create helper and the state seeding are the ones the router
//! input restart and deletion suites own, so the router input suites cannot drift apart. The
//! harness is the one `flow_lifecycle` owns (one router, one shared engine), so every case
//! drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::time::SystemTime;

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_deletion::seed_router_input;
use crate::router_input_restart::{
    create_body, create_router_input, get_router_input, minted_configuration_rows,
    past_transition_span, settle_router_input_in, stored_state,
};

async fn stop_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/routerInput/stop/{arn}"), None)
        .await
}

async fn delete_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await
}

/// The `state` member of a response body, as a borrowed string.
fn state_of(body: &Value) -> &str {
    body.get("state").and_then(Value::as_str).unwrap_or_default()
}

/// The `state` GetRouterInput reports for a router input.
fn reported_state(body: &Value) -> &str {
    body.get("routerInput")
        .and_then(|router_input| router_input.get("state"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// When the store last recorded a change to the router input `arn`.
fn stored_updated_at(emulator: &Emulator, arn: &str) -> SystemTime {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store
        .router_inputs
        .get(arn)
        .map(|router_input| router_input.updated_at)
        .expect("the router input is in the store")
}

/// The settled states a stop is accepted from: `ACTIVE` is the state StartRouterInput
/// settles an input into, and `ERROR` is a settled state nothing in the store owes a change
/// for (`smithy_query(query="shape",shape="RouterInputState")`).
const SETTLED_STATES: [&str; 2] = [mc::ROUTER_INPUT_STATE_ACTIVE, mc::ROUTER_INPUT_STATE_ERROR];

/// A stop takes the STORED router input its ARN addresses out of the state it was running
/// in: the call answers that record's ARN and name with the transient `STOPPING`, and the
/// clock settles the input into `STANDBY` one transition span later. Nothing else in the
/// store moves, and a repeat of the stop persists nothing.
///
/// The rows are every `RouterInputConfiguration` kind crossed with the settled states a stop
/// can arrive in, out of a store that holds a growing number of router inputs — so no part
/// of the answer can be a value fixed in the handler.
#[tokio::test]
async fn a_stop_moves_every_kind_of_router_input_through_stopping_into_standby() {
    let emulator = Emulator::new();
    let rows = minted_configuration_rows(&emulator, "ri-stop").await;

    // (case, the stopped input's ARN, a sibling the stop must leave alone, that sibling's
    // state) — collected so the clock settles every row with ONE wait.
    let mut stopped: Vec<(String, String, String, &str)> = Vec::new();
    for (index, (kind, _, _, configuration, _)) in rows.iter().enumerate() {
        for from_state in SETTLED_STATES {
            let case = format!("{kind} stopped from {from_state}");
            let name = format!("input-{kind}-{from_state}").replace('/', "-");

            // Router inputs the stop must leave alone; the count grows with the row, so the
            // handler cannot be answering about "the" stored input.
            let sibling = seed_router_input(
                &emulator,
                &format!("sibling-{index}-{from_state}"),
                from_state,
            );

            let (status, code, created) =
                create_router_input(&emulator, create_body(&name, configuration.clone())).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: CreateRouterInput answers 201, got {status} {code:?} with \
                 {created}"
            );
            let arn = created["routerInput"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            settle_router_input_in(&emulator, &arn, from_state);
            let updated_before = stored_updated_at(&emulator, &arn);

            let (status, code, accepted) = stop_router_input(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: StopRouterInput answers 202, got {status} {code:?} with \
                 {accepted}"
            );
            assert_eq!(
                (
                    accepted.get("arn").and_then(Value::as_str),
                    accepted.get("name").and_then(Value::as_str),
                    state_of(&accepted),
                ),
                (
                    Some(arn.as_str()),
                    Some(name.as_str()),
                    mc::ROUTER_INPUT_STATE_STOPPING,
                ),
                "case {case:?}: the stop answers the stored input's ARN and name with the \
                 transient STOPPING, got {accepted}"
            );
            assert!(
                stored_updated_at(&emulator, &arn) > updated_before,
                "case {case:?}: the stop changed the record, so UpdatedAt moves, got {:?}",
                stored_updated_at(&emulator, &arn)
            );

            // The transient state is really observable to a caller acting straight after.
            let (status, code, read_back) = get_router_input(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref(), reported_state(&read_back)),
                (200, None, mc::ROUTER_INPUT_STATE_STOPPING),
                "case {case:?}: the input reads back STOPPING while the stop is in flight, got \
                 {status} {code:?} with {read_back}"
            );

            // A repeat answers the same thing rather than restarting the transition.
            let in_flight_updated_at = stored_updated_at(&emulator, &arn);
            let (status, code, again) = stop_router_input(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref(), state_of(&again)),
                (202, None, mc::ROUTER_INPUT_STATE_STOPPING),
                "case {case:?}: a stop repeated while the input is stopping answers STOPPING, \
                 got {status} {code:?} with {again}"
            );
            assert_eq!(
                stored_updated_at(&emulator, &arn),
                in_flight_updated_at,
                "case {case:?}: the repeated stop persisted nothing, so UpdatedAt did not move"
            );

            stopped.push((case, arn, sibling, from_state));
        }
    }

    // One wait for every row: each stop owes `STANDBY` at its own instant, and all of them
    // are due once a transition span has passed.
    tokio::time::sleep(past_transition_span()).await;

    for (case, arn, sibling, from_state) in stopped {
        let (status, code, read_back) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref(), reported_state(&read_back)),
            (200, None, mc::ROUTER_INPUT_STATE_STANDBY),
            "case {case:?}: the clock settles a stopped input into STANDBY, where a created \
             input also waits, got {status} {code:?} with {read_back}"
        );

        let settled_updated_at = stored_updated_at(&emulator, &arn);
        let (status, code, stopped_again) = stop_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref(), state_of(&stopped_again)),
            (202, None, mc::ROUTER_INPUT_STATE_STANDBY),
            "case {case:?}: stopping an input that is already stopped answers STANDBY, got \
             {status} {code:?} with {stopped_again}"
        );
        assert_eq!(
            stored_updated_at(&emulator, &arn),
            settled_updated_at,
            "case {case:?}: the no-op stop persisted nothing, so UpdatedAt did not move"
        );

        assert_eq!(
            stored_state(&emulator, &sibling),
            from_state,
            "case {case:?}: the stop left the sibling {sibling:?} in the state it was in, got \
             {:?}",
            stored_state(&emulator, &sibling)
        );
    }
}

/// A stop cannot be accepted while the clock already owes the addressed router input a
/// DIFFERENT state change: the input is between two states, which is the modelled
/// `ConflictException` (409). The refusal persists nothing, and the same stop is accepted
/// once the transition it conflicted with has settled.
///
/// The rows are every configuration kind crossed with the ways a change can be in flight:
/// the create's own transition, and the removal DeleteRouterInput has accepted — after which
/// the same ARN addresses nothing and the stop is the modelled `NotFoundException` (404).
#[tokio::test]
async fn a_stop_is_refused_while_another_state_change_is_in_flight() {
    let emulator = Emulator::new();
    let rows = minted_configuration_rows(&emulator, "ri-stop-conflict").await;

    for (kind, _, _, configuration, _) in &rows {
        let case = *kind;
        let name = format!("conflict-{kind}").replace('/', "-");
        let (status, code, created) =
            create_router_input(&emulator, create_body(&name, configuration.clone())).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: CreateRouterInput answers 201, got {status} {code:?} with {created}"
        );
        let arn = created["routerInput"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The create's transition is still in flight, so the stop conflicts with it.
        let (status, code, refused) = stop_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?} (stop while creating): expected the modelled ConflictException, got \
             {status} {code:?} with {refused}"
        );
        assert_eq!(
            stored_state(&emulator, &arn),
            mc::ROUTER_INPUT_STATE_CREATING,
            "case {case:?} (stop while creating): the refusal persisted nothing, got {refused}"
        );

        // Once the create has settled the SAME call is accepted.
        tokio::time::sleep(past_transition_span()).await;
        settle_router_input_in(&emulator, &arn, mc::ROUTER_INPUT_STATE_ACTIVE);
        let (status, code, accepted) = stop_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref(), state_of(&accepted)),
            (202, None, mc::ROUTER_INPUT_STATE_STOPPING),
            "case {case:?}: the stop is accepted once the create has settled, got {status} \
             {code:?} with {accepted}"
        );

        // A router input on its way OUT is not one a stop can still act on.
        tokio::time::sleep(past_transition_span()).await;
        let (status, code, deleted) = delete_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref(), state_of(&deleted)),
            (200, None, mc::ROUTER_INPUT_STATE_DELETING),
            "case {case:?}: the delete reports DELETING, got {status} {code:?} with {deleted}"
        );
        let (status, code, refused) = stop_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?} (stop while deleting): expected the modelled ConflictException, got \
             {status} {code:?} with {refused}"
        );
        assert_eq!(
            stored_state(&emulator, &arn),
            mc::ROUTER_INPUT_STATE_DELETING,
            "case {case:?} (stop while deleting): the refusal did not cancel the deletion, got \
             {refused}"
        );

        // The clock completes the deletion, after which the ARN addresses nothing.
        tokio::time::sleep(past_transition_span()).await;
        let (status, code, body) = stop_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: a stop of a deleted router input is NotFoundException, got {status} \
             {code:?} with {body}"
        );
    }
}

/// An ARN of the modelled form that the store holds no router input for is a
/// `NotFoundException` (404) from the stop — never a success and never a 501 — with the
/// request's own label in this service's not-found envelope. An ARN outside the modelled
/// `RouterInputArn` pattern never reaches the handler: the `Arn` member declares that
/// pattern (`smithy_query(query="operation",operation="StopRouterInput")`), so the boundary
/// refuses it with `BadRequestException` (400).
///
/// The rows span the ways an ARN can address no router input (an id never minted, and each
/// ARN segment changed in turn) and the ways one can fall outside the pattern.
#[tokio::test]
async fn a_stop_refuses_an_arn_that_addresses_no_router_input() {
    let emulator = Emulator::new();

    // One stored, settled router input, so a refusal is about the ARN rather than an empty
    // store — and the positive control at the end of the test.
    let held = seed_router_input(&emulator, "held-input", mc::ROUTER_INPUT_STATE_ACTIVE);

    let not_found = [
        (
            "an id this store never minted",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:ffffffffffff",
        ),
        (
            "another Region",
            "arn:aws:mediaconnect:eu-west-1:000000000000:routerInput:000000000001",
        ),
        (
            "another account",
            "arn:aws:mediaconnect:us-east-1:000000000001:routerInput:000000000001",
        ),
        (
            "another partition",
            "arn:aws-us-gov:mediaconnect:us-east-1:000000000000:routerInput:000000000001",
        ),
    ];
    for (case, arn) in not_found {
        let (status, code, body) = stop_router_input(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: StopRouterInput answers NotFoundException, got {status} {code:?} \
             with {body}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(mc::not_found_message(arn).as_str()),
            "case {case:?}: the refusal names the identifier the request carried, got {body}"
        );
    }

    let outside_the_pattern = [
        ("not an ARN at all", "router-input-1"),
        (
            "a router OUTPUT ARN",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:000000000001",
        ),
        (
            "an id that is one character too short",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:00000000001",
        ),
        (
            "an id with an uppercase character",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:00000000000A",
        ),
    ];
    for (case, arn) in outside_the_pattern {
        let (status, code, body) = stop_router_input(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: the boundary refuses an ARN outside the modelled pattern, got \
             {status} {code:?} with {body}"
        );
    }

    // The positive control: the ARN the store DOES hold is still stoppable, so none of the
    // refusals above removed or hid it.
    let (status, code, accepted) = stop_router_input(&emulator, &held).await;
    assert_eq!(
        (status, code.as_deref(), state_of(&accepted)),
        (202, None, mc::ROUTER_INPUT_STATE_STOPPING),
        "the router input the store holds is stopped, got {status} {code:?} with {accepted}"
    );
}

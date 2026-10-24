// SPDX-License-Identifier: CC0-1.0

//! StopRouterOutput, end to end over HTTP: CreateRouterOutput -> StopRouterOutput ->
//! GetRouterOutput.
//!
//! The rule the suite pins, over a table rather than over the one request that exposed it:
//! a stop acts on the STORED router output its `@httpLabel Arn` addresses and is accepted
//! whenever that record is settled (the clock owes it no state change and no accepted
//! deletion), answering the record's own ARN and name with the non-terminal `STOPPING`
//! state, which the clock then settles into `STANDBY` one `mc::TRANSITION_SPAN` later; a
//! router output that is already stopped or already stopping keeps the state it is in and
//! the call persists nothing; one whose clock still owes a different change is refused with
//! the modelled `ConflictException` (409); and an ARN the store holds no router output for
//! is the modelled `NotFoundException` (404), while an ARN outside the modelled
//! `RouterOutputArn` pattern is refused at the boundary with `BadRequestException` (400).
//!
//! The cases vary what that rule generalises over: which `RouterOutputConfiguration`
//! variant (and so which output type) the output carries, its tier, maximum bitrate,
//! routing scope and Region, the state the output was in when the stop arrived, how many
//! router outputs the store holds beside it, how many times the same output is stopped, and
//! whether the clock has settled the stop yet.
//!
//! StartRouterOutput is still scaffolded
//! (`issues/things-that-do-not-work/20261024T010542Z-mediaconnect-startrouteroutput-stub.md`),
//! so the operational states an output other than a freshly created one is in are seeded
//! through the store, exactly as `tests/all/router_input_restart.rs` seeds them for the
//! router input side of the same rule. The record is left SETTLED, as a created output is
//! once the clock has settled it.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization. The fixtures and the
//! create helper are the ones `router_outputs` owns, so the tables of the router output
//! suites cannot drift apart.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_outputs::{
    create_router_output, standard_with_protocol, FLOW_ARN, MEDIALIVE_INPUT_ARN, ROLE_ARN,
    SECRET_ARN,
};

/// Long enough for a transition the clock owes to be due: twice [`mc::TRANSITION_SPAN`], so
/// a settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

async fn stop_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/routerOutput/stop/{arn}"), None)
        .await
}

async fn get_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

async fn delete_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

/// The `state` member of a response body, as a borrowed string.
fn state_of(body: &Value) -> &str {
    body.get("state").and_then(Value::as_str).unwrap_or_default()
}

/// The `state` GetRouterOutput reports for a router output.
fn reported_state(body: &Value) -> &str {
    body.get("routerOutput")
        .and_then(|output| output.get("state"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Put the stored router output `arn` in `state` with NOTHING owed by the clock — the shape
/// a record has once a transition has settled.
///
/// StartRouterOutput is still scaffolded, so the operational states a stop can arrive in
/// are reached through the store rather than through a call that answers 501.
fn settle_router_output_in(emulator: &Emulator, arn: &str, state: &str) {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let router_output = store
        .router_outputs
        .get_mut(arn)
        .expect("the router output the create minted is in the store");
    router_output.state = state.to_string();
    router_output.pending = None;
}

/// The state the store holds for `arn`, read straight from the record.
fn stored_state(emulator: &Emulator, arn: &str) -> String {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store
        .router_outputs
        .get(arn)
        .map(|router_output| router_output.state.clone())
        .unwrap_or_default()
}

/// When the store last recorded a change to `arn`.
fn stored_updated_at(emulator: &Emulator, arn: &str) -> SystemTime {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store
        .router_outputs
        .get(arn)
        .map(|router_output| router_output.updated_at)
        .expect("the router output is in the store")
}

/// One create request: a kind of router output, named by the `RouterOutputConfiguration`
/// member it carries.
struct ConfigurationRow {
    case: &'static str,
    body: Value,
}

/// One row per `RouterOutputConfiguration` member the model declares
/// (`smithy_query(query="shape",shape="RouterOutputConfiguration")`: Standard,
/// MediaConnectFlow, MediaLiveInput) — and for the standard kind two different protocol
/// configurations — differing also in tier, maximum bitrate, routing scope and Region, so
/// the stop rule is asserted over the whole class of router outputs rather than over one
/// configuration.
fn configuration_rows() -> Vec<ConfigurationRow> {
    vec![
        ConfigurationRow {
            case: "standard rtp",
            body: json!({
                "name": "rtp-output",
                "configuration": standard_with_protocol(json!({
                    "Rtp": {
                        "destinationAddress": "203.0.113.10",
                        "destinationPort": 5004,
                        "forwardErrorCorrection": "ENABLED",
                    }
                }), "RTP"),
                "maximumBitrate": 20_000_000_i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            }),
        },
        ConfigurationRow {
            case: "standard srt listener",
            body: json!({
                "name": "srt-listener-output",
                "configuration": standard_with_protocol(json!({
                    "SrtListener": {
                        "port": 3100,
                        "minimumLatencyMilliseconds": 120,
                    }
                }), "SRT_LISTENER"),
                "maximumBitrate": 100_000_000_i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_100",
                "availabilityZone": "us-east-1c",
                "tags": { "team": "broadcast" },
            }),
        },
        ConfigurationRow {
            case: "mediaconnect flow destination in another Region",
            body: json!({
                "name": "flow-output",
                "configuration": json!({
                    "MediaConnectFlow": {
                        "flowArn": FLOW_ARN,
                        "destinationTransitEncryption": {
                            "encryptionKeyType": "SECRETS_MANAGER",
                            "encryptionKeyConfiguration": {
                                "SecretsManager": {
                                    "secretArn": SECRET_ARN,
                                    "roleArn": ROLE_ARN,
                                }
                            },
                        },
                    }
                }),
                "maximumBitrate": 500_000_000_i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_500",
                "regionName": "eu-west-1",
                "availabilityZone": "eu-west-1b",
            }),
        },
        ConfigurationRow {
            case: "medialive input destination",
            body: json!({
                "name": "medialive-output",
                "configuration": json!({
                    "MediaLiveInput": {
                        "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
                        "mediaLivePipelineId": "PIPELINE_1",
                        "destinationTransitEncryption": {
                            "encryptionKeyType": "AUTOMATIC",
                            "encryptionKeyConfiguration": { "Automatic": {} },
                        },
                    }
                }),
                "maximumBitrate": 250_i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_250",
                "maintenanceConfiguration": { "Default": {} },
            }),
        },
    ]
}

/// Create the router output `row` describes and return its `(Arn, Name)`.
async fn create(emulator: &Emulator, row: &ConfigurationRow) -> (String, String) {
    let case = row.case;
    let (status, code, body) = create_router_output(emulator, row.body.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterOutput answers 201, got {status} {code:?} with {body:?}"
    );
    let created = body
        .get("routerOutput")
        .cloned()
        .unwrap_or(Value::Null);
    (
        created
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// A stop takes the STORED router output its ARN addresses out of the state it was running
/// in: the call answers that record's ARN and name with the transient `STOPPING`, and the
/// clock settles the output into `STANDBY` one transition span later. Nothing else in the
/// store moves.
///
/// The rows are every configuration kind crossed with the settled states a stop can arrive
/// in, out of a store that holds a growing number of router outputs — so no part of the
/// answer can be a value fixed in the handler.
#[tokio::test]
async fn stop_moves_the_stored_router_output_through_stopping_into_standby() {
    // The settled states a stop is accepted from: `ACTIVE` is the state StartRouterOutput
    // settles an output into, and `ERROR` is a settled state nothing in the store owes a
    // change for (`smithy_query(query="shape",shape="RouterOutputState")`).
    let from_states = [
        mc::ROUTER_OUTPUT_STATE_ACTIVE,
        mc::ROUTER_OUTPUT_STATE_ERROR,
    ];
    let rows = configuration_rows();

    for from_state in from_states {
        for (index, row) in rows.iter().enumerate() {
            let case = format!("{} stopped from {from_state}", row.case);
            let emulator = Emulator::new();

            // Other router outputs the stop must leave alone; the count varies with the row.
            let mut untouched: Vec<String> = Vec::new();
            for other in rows.iter().take(index) {
                let (other_arn, _) = create(&emulator, other).await;
                settle_router_output_in(&emulator, &other_arn, from_state);
                untouched.push(other_arn);
            }

            let (arn, name) = create(&emulator, row).await;
            settle_router_output_in(&emulator, &arn, from_state);
            let updated_before = stored_updated_at(&emulator, &arn);

            let (status, code, stopped) = stop_router_output(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: StopRouterOutput answers 202, got {status} {code:?} with \
                 {stopped:?}"
            );
            assert_eq!(
                (
                    stopped.get("arn").and_then(Value::as_str),
                    stopped.get("name").and_then(Value::as_str),
                    state_of(&stopped),
                ),
                (
                    Some(arn.as_str()),
                    Some(name.as_str()),
                    mc::ROUTER_OUTPUT_STATE_STOPPING,
                ),
                "case {case:?}: the stop answers the stored output's ARN and name with the \
                 transient STOPPING, got {stopped:?}"
            );
            assert!(
                stored_updated_at(&emulator, &arn) > updated_before,
                "case {case:?}: the stop changed the record, so UpdatedAt moves, got {:?}",
                stored_updated_at(&emulator, &arn)
            );

            // The transient state is really observable to a caller acting straight after.
            let (status, code, body) = get_router_output(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref(), reported_state(&body)),
                (200, None, mc::ROUTER_OUTPUT_STATE_STOPPING),
                "case {case:?}: the output reads back STOPPING while the stop is in flight, \
                 got {status} {code:?} with {body:?}"
            );

            // A repeat answers the same thing rather than restarting the transition.
            let (status, code, again) = stop_router_output(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref(), state_of(&again)),
                (202, None, mc::ROUTER_OUTPUT_STATE_STOPPING),
                "case {case:?}: a stop repeated while the output is stopping answers STOPPING, \
                 got {status} {code:?} with {again:?}"
            );

            // The clock settles the stop: the output waits in STANDBY, where a created
            // output also waits, and a further stop is the no-op that reports it.
            tokio::time::sleep(past_transition_span()).await;
            let (status, code, body) = get_router_output(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref(), reported_state(&body)),
                (200, None, mc::ROUTER_OUTPUT_STATE_STANDBY),
                "case {case:?}: the clock settles a stopped output into STANDBY, got {status} \
                 {code:?} with {body:?}"
            );

            let settled_updated_at = stored_updated_at(&emulator, &arn);
            let (status, code, stopped_again) = stop_router_output(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref(), state_of(&stopped_again)),
                (202, None, mc::ROUTER_OUTPUT_STATE_STANDBY),
                "case {case:?}: stopping an output that is already stopped answers STANDBY, \
                 got {status} {code:?} with {stopped_again:?}"
            );
            assert_eq!(
                stored_updated_at(&emulator, &arn),
                settled_updated_at,
                "case {case:?}: the no-op stop persisted nothing, so UpdatedAt did not move"
            );

            // Nothing else was stopped.
            for other in &untouched {
                assert_eq!(
                    stored_state(&emulator, other),
                    from_state,
                    "case {case:?}: the stop left {other:?} in the state it was in"
                );
            }
        }
    }
}

/// A stop cannot be accepted while the clock already owes the addressed router output a
/// DIFFERENT state change: the output is between two states, which is the modelled
/// `ConflictException` (409). The refusal persists nothing, and the stop is accepted once
/// the transition it conflicted with has settled.
///
/// The rows are the ways a change can be in flight: the create's own transition, and the
/// removal DeleteRouterOutput has accepted — after which the same ARN addresses nothing and
/// the stop is the modelled `NotFoundException` (404).
#[tokio::test]
async fn a_stop_is_refused_while_another_state_change_is_in_flight() {
    let rows = configuration_rows();

    for row in &rows {
        let case = row.case;
        let emulator = Emulator::new();
        let (arn, _) = create(&emulator, row).await;

        // The create's transition is still in flight, so the stop conflicts with it.
        let (status, code, refused) = stop_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?} (stop while creating): expected the modelled ConflictException, got \
             {status} {code:?} with {refused:?}"
        );
        assert_eq!(
            stored_state(&emulator, &arn),
            mc::ROUTER_OUTPUT_STATE_CREATING,
            "case {case:?} (stop while creating): the refusal persisted nothing, got {refused:?}"
        );

        // Once the create has settled the SAME call is accepted.
        tokio::time::sleep(past_transition_span()).await;
        settle_router_output_in(&emulator, &arn, mc::ROUTER_OUTPUT_STATE_ACTIVE);
        let (status, code, stopped) = stop_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref(), state_of(&stopped)),
            (202, None, mc::ROUTER_OUTPUT_STATE_STOPPING),
            "case {case:?}: the stop is accepted once the create has settled, got {status} \
             {code:?} with {stopped:?}"
        );

        // A router output on its way OUT is not one a stop can still act on.
        tokio::time::sleep(past_transition_span()).await;
        let (status, _, deleted) = delete_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, state_of(&deleted)),
            (200, mc::ROUTER_OUTPUT_STATE_DELETING),
            "case {case:?}: the delete reports DELETING, got {status} with {deleted:?}"
        );
        let (status, code, refused) = stop_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?} (stop while deleting): expected the modelled ConflictException, got \
             {status} {code:?} with {refused:?}"
        );
        assert_eq!(
            stored_state(&emulator, &arn),
            mc::ROUTER_OUTPUT_STATE_DELETING,
            "case {case:?} (stop while deleting): the refusal did not cancel the deletion, got \
             {refused:?}"
        );

        // The clock completes the deletion, after which the ARN addresses nothing.
        tokio::time::sleep(past_transition_span()).await;
        let (status, code, body) = stop_router_output(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: a stop of a deleted router output is NotFoundException, got \
             {status} {code:?} with {body:?}"
        );
    }
}

/// An ARN of the modelled form that the store holds no router output for is a
/// `NotFoundException` (404) from the stop — never a success and never a 501 — with the
/// request's own label in this service's not-found envelope. An ARN outside the modelled
/// `RouterOutputArn` pattern never reaches the handler: the `Arn` member declares that
/// pattern, so the boundary refuses it with `BadRequestException` (400).
///
/// The rows span the ways an ARN can address no router output (an id never minted, and each
/// ARN segment changed in turn) and the ways one can fall outside the pattern.
#[tokio::test]
async fn a_stop_refuses_an_arn_that_addresses_no_router_output() {
    let emulator = Emulator::new();

    // One stored, settled router output, so a refusal is about the ARN rather than an empty
    // store — and the positive control at the end of the test.
    let rows = configuration_rows();
    let (held, _) = create(&emulator, &rows[0]).await;
    settle_router_output_in(&emulator, &held, mc::ROUTER_OUTPUT_STATE_ACTIVE);

    let not_found = [
        (
            "an id this store never minted",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:ffffffffffff",
        ),
        (
            "another Region",
            "arn:aws:mediaconnect:eu-west-1:000000000000:routerOutput:000000000001",
        ),
        (
            "another account",
            "arn:aws:mediaconnect:us-east-1:000000000001:routerOutput:000000000001",
        ),
        (
            "another partition",
            "arn:aws-us-gov:mediaconnect:us-east-1:000000000000:routerOutput:000000000001",
        ),
    ];
    for (case, arn) in not_found {
        let (status, code, body) = stop_router_output(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: StopRouterOutput answers NotFoundException, got {status} {code:?} \
             with {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(mc::not_found_message(arn).as_str()),
            "case {case:?}: the refusal names the identifier the request carried, got {body:?}"
        );
    }

    let outside_the_pattern = [
        ("not an ARN at all", "router-output-1"),
        (
            "a router INPUT ARN",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:000000000001",
        ),
        (
            "an id that is one character too short",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:00000000001",
        ),
        (
            "an id with an uppercase character",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:00000000000A",
        ),
    ];
    for (case, arn) in outside_the_pattern {
        let (status, code, body) = stop_router_output(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: the boundary refuses an ARN outside the modelled pattern, got \
             {status} {code:?} with {body:?}"
        );
    }

    // The positive control: the ARN the store DOES hold is still stoppable, so none of the
    // refusals above removed or hid it.
    let (status, code, stopped) = stop_router_output(&emulator, &held).await;
    assert_eq!(
        (status, code.as_deref(), state_of(&stopped)),
        (202, None, mc::ROUTER_OUTPUT_STATE_STOPPING),
        "the router output the store holds is stopped, got {status} {code:?} with {stopped:?}"
    );
}

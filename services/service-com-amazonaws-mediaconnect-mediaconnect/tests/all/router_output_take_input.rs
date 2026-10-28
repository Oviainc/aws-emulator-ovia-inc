// SPDX-License-Identifier: CC0-1.0

//! TakeRouterInput, end to end over HTTP: `PUT /v1/routerOutput/takeRouterInput/{Arn}`.
//!
//! The rules the cases below pin, over a table rather than over one request:
//!
//! 1. A take records an ASSOCIATION on the stored router output its `@httpLabel`
//!    `RouterOutputArn` addresses: the router input the request names becomes that output's
//!    `RoutedInputArn`, and a request that names NO router input clears the association.
//!    The response reports the stored output's own ARN and name beside the associated
//!    input's ARN and name, and the association is what every later read computes from —
//!    GetRouterOutput's `RoutedInputArn` / `RoutedState` and GetRouterInput's
//!    `RoutedOutputs`.
//!
//! 2. The call answers `202 Accepted`, so the association is not in effect the instant it
//!    answers: the routed state is the non-terminal `ROUTING`, which the clock settles one
//!    `mc::TRANSITION_SPAN` later into `ROUTED` for an output that carries an input and
//!    `UNROUTED` for one whose association was cleared. A take asking for the association
//!    the output is already in persists nothing (`UpdatedAt` does not move) and reports the
//!    routed state it is in, as an `@idempotent` call must.
//!
//! 3. Both resources the request names are resolved against the STORE: an ARN the store
//!    holds no router output for, and one it holds no router input for, are each the
//!    modelled `NotFoundException` (404) in this service's not-found envelope, while an ARN
//!    outside the modelled pattern never reaches the handler (`BadRequestException`, 400).
//!    A take is refused with the modelled `ConflictException` (409) while the clock still
//!    owes the output a state change or either resource is on its way out, and such a
//!    refusal persists nothing.
//!
//! The rows vary what those rules generalise over: which of the `RouterInputConfiguration`
//! members the input was created with (so which input type), which
//! `RouterOutputConfiguration` member, tier, bitrate, routing scope and Region the output
//! carries, the settled state the output was in when the take arrived, how many router
//! outputs and inputs the store holds beside the pair under test, whether the request names
//! an input or none, the form the "no input" request takes, how often the same take is
//! repeated and whether the clock has settled it yet.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization. The fixtures and the
//! create helpers are the ones `router_outputs` and `router_input_restart` own, and the
//! store-seeding helpers the ones `router_output_stop` and `router_input_deletion` own, so
//! the router suites cannot drift apart.

use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::ROUTER_OUTPUT_ROUTED_STATE_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_restart::{
    create_body, create_router_input, get_router_input, minted_configuration_rows,
    past_transition_span, settle_router_input_in,
};
use crate::router_output_stop::settle_router_output_in;
use crate::router_outputs::{
    create_router_output, minted_configuration_targets, standard_with_protocol,
    ConfigurationTargets, MEDIALIVE_INPUT_ARN, ROLE_ARN, SECRET_ARN,
};

/// Send one TakeRouterInput request. `router_input_arn` is the input the request asks for,
/// or `None` for a request that names none — carried as a body with no `routerInputArn`
/// member, which is what clearing the association looks like on the wire.
///
/// `pub(crate)` because `tests/all/update_router_output.rs` reaches the ACTIVE ROUTE its
/// `FabricConfiguration` precondition is stated against through this one helper, so the two
/// suites cannot drift from the wire shape.
pub(crate) async fn take_router_input(
    emulator: &Emulator,
    router_output_arn: &str,
    router_input_arn: Option<&str>,
) -> (u16, Option<String>, Value) {
    let body = match router_input_arn {
        Some(router_input_arn) => json!({ "routerInputArn": router_input_arn }),
        None => json!({}),
    };
    emulator
        .send(
            "PUT",
            &format!("/v1/routerOutput/takeRouterInput/{router_output_arn}"),
            Some(body),
        )
        .await
}

async fn get_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

async fn delete_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await
}

async fn delete_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

/// The `(routedState, routerOutputArn, routerOutputName, routerInputArn, routerInputName)`
/// a take answers.
fn taken(body: &Value) -> (&str, &str, &str, Option<&str>, Option<&str>) {
    (
        body.get("routedState")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        body.get("routerOutputArn")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        body.get("routerOutputName")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        body.get("routerInputArn").and_then(Value::as_str),
        body.get("routerInputName").and_then(Value::as_str),
    )
}

/// The `(routedState, routedInputArn)` GetRouterOutput reports for a router output.
fn reported_route(body: &Value) -> (&str, Option<&str>) {
    let output = body.get("routerOutput").unwrap_or(&Value::Null);
    (
        output
            .get("routedState")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        output.get("routedInputArn").and_then(Value::as_str),
    )
}

/// `RouterInput$RoutedOutputs` as GetRouterInput reports it — the number of outputs routed
/// to the input, which lives on the OUTPUT records and so is the other side of the same
/// association.
fn reported_routed_outputs(body: &Value) -> i64 {
    body.get("routerInput")
        .and_then(|router_input| router_input.get("routedOutputs"))
        .and_then(Value::as_i64)
        .unwrap_or(-1)
}

/// The association the store holds for `arn`: `(RoutedInputArn, UpdatedAt)`, read straight
/// off the record, so a case can tell a call that persisted nothing from one that did.
fn stored_route(emulator: &Emulator, arn: &str) -> (Option<String>, SystemTime) {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store
        .router_outputs
        .get(arn)
        .map(|router_output| (router_output.routed_input_arn.clone(), router_output.updated_at))
        .expect("the router output is in the store")
}

/// One kind of router output: a `CreateRouterOutput` body, differing from the others along
/// the configuration union member (so the output type), the tier, the maximum bitrate, the
/// routing scope, the Region and the maintenance configuration.
///
/// The rows span every `RouterOutputConfiguration` member the model declares
/// (`smithy_query(query="shape",shape="RouterOutputConfiguration")`: Standard,
/// MediaConnectFlow, MediaLiveInput), with two protocol configurations for the standard
/// kind, so the rules above are asserted over the whole class of router outputs rather than
/// over one configuration.
fn output_bodies(targets: &ConfigurationTargets) -> Vec<(&'static str, Value)> {
    vec![
        (
            "standard rtp",
            json!({
                "configuration": standard_with_protocol(&targets.interface_arn, json!({
                    "Rtp": {
                        "destinationAddress": "203.0.113.30",
                        "destinationPort": 5004,
                        "forwardErrorCorrection": "ENABLED",
                    }
                }), "RTP"),
                "maximumBitrate": 20_000_000_i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            }),
        ),
        (
            "standard srt listener",
            json!({
                "configuration": standard_with_protocol(&targets.interface_arn, json!({
                    "SrtListener": { "port": 3300, "minimumLatencyMilliseconds": 120 }
                }), "SRT_LISTENER"),
                "maximumBitrate": 100_000_000_i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_100",
                "availabilityZone": "us-east-1c",
                "tags": { "team": "broadcast" },
            }),
        ),
        (
            "mediaconnect flow destination in another Region",
            json!({
                "configuration": {
                    "MediaConnectFlow": {
                        "flowArn": targets.flow_arn,
                        "flowSourceArn": targets.flow_source_arn,
                        "destinationTransitEncryption": {
                            "encryptionKeyType": "SECRETS_MANAGER",
                            "encryptionKeyConfiguration": {
                                "SecretsManager": { "secretArn": SECRET_ARN, "roleArn": ROLE_ARN }
                            },
                        },
                    }
                },
                "maximumBitrate": 500_000_000_i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_500",
                "regionName": "eu-west-1",
                "availabilityZone": "eu-west-1b",
            }),
        ),
        (
            "medialive input destination",
            json!({
                "configuration": {
                    "MediaLiveInput": {
                        "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
                        "mediaLivePipelineId": "PIPELINE_1",
                        "destinationTransitEncryption": {
                            "encryptionKeyType": "AUTOMATIC",
                            "encryptionKeyConfiguration": { "Automatic": {} },
                        },
                    }
                },
                "maximumBitrate": 250_i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_250",
                "maintenanceConfiguration": { "Default": {} },
            }),
        ),
    ]
}

/// How many rows [`output_bodies`] has, needed before there is an emulator to mint the
/// configuration targets on.
const OUTPUT_BODY_COUNT: usize = 4;

/// Create the router output `body` describes under `name` and answer its `(Arn, Name)`,
/// SETTLED in `state` — the shape a record has once the clock has finished its creation.
async fn create_output(
    emulator: &Emulator,
    case: &str,
    name: &str,
    mut body: Value,
    state: &str,
) -> (String, String) {
    body["name"] = json!(name);
    let (status, code, created) = create_router_output(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterOutput answers 201, got {status} {code:?} with {created:?}"
    );
    let output = created
        .get("routerOutput")
        .cloned()
        .unwrap_or(Value::Null);
    let arn = output
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let output_name = output
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        reported_route(&json!({ "routerOutput": output })),
        (mc::ROUTED_STATE_UNROUTED, None),
        "case {case:?}: a created router output carries no association yet"
    );
    settle_router_output_in(emulator, &arn, state);
    (arn, output_name)
}

/// Create the router input `configuration` describes under `name` and answer its
/// `(Arn, Name)`.
async fn create_input(
    emulator: &Emulator,
    case: &str,
    name: &str,
    configuration: Value,
) -> (String, String) {
    let (status, code, created) = create_router_input(emulator, create_body(name, configuration)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateRouterInput answers 201, got {status} {code:?} with {created:?}"
    );
    let router_input = created
        .get("routerInput")
        .cloned()
        .unwrap_or(Value::Null);
    (
        router_input
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        router_input
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// The three routed states this suite names are exactly the `RouterOutputRoutedState` enum
/// the model declares, read from the catalog the crate GENERATES from that model — so a row
/// below cannot assert a value the API does not have.
#[test]
fn the_routed_states_are_the_ones_the_model_declares() {
    let mut named = [
        mc::ROUTED_STATE_ROUTED,
        mc::ROUTED_STATE_ROUTING,
        mc::ROUTED_STATE_UNROUTED,
    ];
    named.sort_unstable();
    let mut declared = ROUTER_OUTPUT_ROUTED_STATE_VALUES.to_vec();
    declared.sort_unstable();
    assert_eq!(
        named.to_vec(),
        declared,
        "the suite names every RouterOutputRoutedState the model declares, got \
         {ROUTER_OUTPUT_ROUTED_STATE_VALUES:?}"
    );
}

/// A take associates the STORED router input with the STORED router output, answers the
/// non-terminal `ROUTING`, and the clock settles the association into `ROUTED` — for every
/// kind of router input crossed with every kind of router output, out of a store that holds
/// a growing number of each.
///
/// Both sides of the association are then read back: the output reports the input as its
/// `RoutedInputArn`, and the input counts the output in its `RoutedOutputs`. A take
/// repeating the association the output is already in persists nothing.
#[tokio::test]
async fn a_take_associates_the_stored_input_with_the_stored_output() {
    // The settled states a take is accepted from: where a created output waits, and where a
    // started one runs (`smithy_query(query="shape",shape="RouterOutputState")`).
    let settled_states = [
        mc::ROUTER_OUTPUT_STATE_STANDBY,
        mc::ROUTER_OUTPUT_STATE_ACTIVE,
    ];
    let emulator = Emulator::new();
    let input_rows = minted_configuration_rows(&emulator, "take").await;
    let targets = minted_configuration_targets(&emulator, "take").await;
    let outputs = output_bodies(&targets);
    assert_eq!(
        outputs.len(),
        OUTPUT_BODY_COUNT,
        "OUTPUT_BODY_COUNT must be the number of output bodies, got {}",
        outputs.len()
    );

    // Outputs the takes below must leave alone: one per row, created before the pair under
    // test, so the store holds a growing number of records that keep their own association.
    let mut untouched: Vec<String> = Vec::new();

    for (index, (input_case, _, _, configuration, _)) in input_rows.iter().enumerate() {
        let (output_case, output_body) = &outputs[index % OUTPUT_BODY_COUNT];
        let settled_state = settled_states[index % settled_states.len()];
        let case = format!("{input_case} into {output_case} in {settled_state}");

        let (other_arn, _) = create_output(
            &emulator,
            &case,
            &format!("untouched-output-{index}"),
            output_body.clone(),
            settled_state,
        )
        .await;
        untouched.push(other_arn);

        let (output_arn, output_name) = create_output(
            &emulator,
            &case,
            &format!("taking-output-{index}"),
            output_body.clone(),
            settled_state,
        )
        .await;
        let (input_arn, input_name) = create_input(
            &emulator,
            &case,
            &format!("taken-input-{index}"),
            configuration.clone(),
        )
        .await;

        let (status, code, taken_body) =
            take_router_input(&emulator, &output_arn, Some(&input_arn)).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: TakeRouterInput answers 202, got {status} {code:?} with \
             {taken_body:?}"
        );
        assert_eq!(
            taken(&taken_body),
            (
                mc::ROUTED_STATE_ROUTING,
                output_arn.as_str(),
                output_name.as_str(),
                Some(input_arn.as_str()),
                Some(input_name.as_str()),
            ),
            "case {case:?}: the take answers the stored pair with the transient ROUTING, got \
             {taken_body:?}"
        );

        // The association is on the record, and the transient routed state is observable to
        // a caller reading the output straight afterwards.
        let (stored_input_arn, updated_after_take) = stored_route(&emulator, &output_arn);
        assert_eq!(
            stored_input_arn.as_deref(),
            Some(input_arn.as_str()),
            "case {case:?}: the take recorded the association on the stored output"
        );
        let (status, code, body) = get_router_output(&emulator, &output_arn).await;
        assert_eq!(
            (status, code.as_deref(), reported_route(&body)),
            (
                200,
                None,
                (mc::ROUTED_STATE_ROUTING, Some(input_arn.as_str())),
            ),
            "case {case:?}: the output reads back ROUTING while the take is in flight, got \
             {status} {code:?} with {body:?}"
        );

        // A repeat of the `@idempotent` take asks for the association the output is already
        // in, so it persists nothing and reports the state it is in.
        let (status, code, again) = take_router_input(&emulator, &output_arn, Some(&input_arn)).await;
        assert_eq!(
            (status, code.as_deref(), taken(&again).0),
            (202, None, mc::ROUTED_STATE_ROUTING),
            "case {case:?}: a take repeated while the output is routing answers ROUTING, got \
             {status} {code:?} with {again:?}"
        );
        assert_eq!(
            stored_route(&emulator, &output_arn).1,
            updated_after_take,
            "case {case:?}: the repeated take persisted nothing, so UpdatedAt did not move"
        );

        // The clock settles the association: the output is ROUTED, and the input counts it.
        tokio::time::sleep(past_transition_span()).await;
        let (status, code, body) = get_router_output(&emulator, &output_arn).await;
        assert_eq!(
            (status, code.as_deref(), reported_route(&body)),
            (
                200,
                None,
                (mc::ROUTED_STATE_ROUTED, Some(input_arn.as_str())),
            ),
            "case {case:?}: the clock settles the association into ROUTED, got {status} \
             {code:?} with {body:?}"
        );
        let (status, code, body) = get_router_input(&emulator, &input_arn).await;
        assert_eq!(
            (status, code.as_deref(), reported_routed_outputs(&body)),
            (200, None, 1),
            "case {case:?}: the input counts the output routed to it, got {status} {code:?} \
             with {body:?}"
        );

        let settled_updated_at = stored_route(&emulator, &output_arn).1;
        let (status, code, settled_again) =
            take_router_input(&emulator, &output_arn, Some(&input_arn)).await;
        assert_eq!(
            (status, code.as_deref(), taken(&settled_again).0),
            (202, None, mc::ROUTED_STATE_ROUTED),
            "case {case:?}: a take of the association the output already carries answers \
             ROUTED, got {status} {code:?} with {settled_again:?}"
        );
        assert_eq!(
            stored_route(&emulator, &output_arn).1,
            settled_updated_at,
            "case {case:?}: the settled no-op take persisted nothing"
        );

        // Nothing else took an input.
        for other in &untouched {
            assert_eq!(
                stored_route(&emulator, other).0,
                None,
                "case {case:?}: the take left {other:?} unrouted"
            );
        }
    }
}

/// A take SWITCHES the association rather than adding one: the output carries exactly the
/// input the last accepted take named, and a take that names no input clears the
/// association, which the clock settles into `UNROUTED`. Both sides are read back, so the
/// count the input reports follows the association rather than the number of calls.
///
/// The rows vary the kind of router input the output is switched between and the kind of
/// router output doing the carrying.
#[tokio::test]
async fn a_take_switches_the_association_and_a_take_without_an_input_clears_it() {
    let emulator = Emulator::new();
    let input_rows = minted_configuration_rows(&emulator, "switch").await;
    let targets = minted_configuration_targets(&emulator, "switch").await;
    let outputs = output_bodies(&targets);

    for index in 0..OUTPUT_BODY_COUNT {
        let (output_case, output_body) = &outputs[index];
        let first = &input_rows[index % input_rows.len()];
        let second = &input_rows[(index + 1) % input_rows.len()];
        let case = format!("{output_case} switched from {} to {}", first.0, second.0);

        let (output_arn, output_name) = create_output(
            &emulator,
            &case,
            &format!("switching-output-{index}"),
            output_body.clone(),
            mc::ROUTER_OUTPUT_STATE_ACTIVE,
        )
        .await;
        let (first_arn, _) = create_input(
            &emulator,
            &case,
            &format!("first-input-{index}"),
            first.3.clone(),
        )
        .await;
        let (second_arn, second_name) = create_input(
            &emulator,
            &case,
            &format!("second-input-{index}"),
            second.3.clone(),
        )
        .await;
        // One input is settled and the other is not: the association is about the input the
        // store HOLDS, not about the state its own lifecycle is in.
        settle_router_input_in(&emulator, &first_arn, mc::ROUTER_INPUT_STATE_ACTIVE);

        let (status, _, _) = take_router_input(&emulator, &output_arn, Some(&first_arn)).await;
        assert_eq!(status, 202, "case {case:?}: the first take is accepted");

        // An association the clock has not settled yet is not a state change a take refuses
        // on: one arriving while the output is still `ROUTING` re-points it, and the clock
        // owes the new association instead. The output is pointed back at the first input,
        // so the rest of the case starts from the association it began with.
        let (status, code, rerouted) =
            take_router_input(&emulator, &output_arn, Some(&second_arn)).await;
        assert_eq!(
            (status, code.as_deref(), taken(&rerouted).0, taken(&rerouted).3),
            (
                202,
                None,
                mc::ROUTED_STATE_ROUTING,
                Some(second_arn.as_str()),
            ),
            "case {case:?}: a take arriving while the output is ROUTING re-points it, got \
             {status} {code:?} with {rerouted:?}"
        );
        let (status, code, back) = take_router_input(&emulator, &output_arn, Some(&first_arn)).await;
        assert_eq!(
            (status, code.as_deref(), taken(&back).3),
            (202, None, Some(first_arn.as_str())),
            "case {case:?}: the output is pointed back at the first input, got {status} \
             {code:?} with {back:?}"
        );
        tokio::time::sleep(past_transition_span()).await;

        let (status, code, switched) =
            take_router_input(&emulator, &output_arn, Some(&second_arn)).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: the switching take answers 202, got {status} {code:?} with \
             {switched:?}"
        );
        assert_eq!(
            taken(&switched),
            (
                mc::ROUTED_STATE_ROUTING,
                output_arn.as_str(),
                output_name.as_str(),
                Some(second_arn.as_str()),
                Some(second_name.as_str()),
            ),
            "case {case:?}: the switch answers the input it took, got {switched:?}"
        );

        tokio::time::sleep(past_transition_span()).await;
        let (_, _, body) = get_router_output(&emulator, &output_arn).await;
        assert_eq!(
            reported_route(&body),
            (mc::ROUTED_STATE_ROUTED, Some(second_arn.as_str())),
            "case {case:?}: the output carries exactly the input the last take named, got \
             {body:?}"
        );
        let (_, _, first_body) = get_router_input(&emulator, &first_arn).await;
        let (_, _, second_body) = get_router_input(&emulator, &second_arn).await;
        assert_eq!(
            (
                reported_routed_outputs(&first_body),
                reported_routed_outputs(&second_body),
            ),
            (0, 1),
            "case {case:?}: the switch moved the count from the first input to the second, \
             got {first_body:?} and {second_body:?}"
        );

        // A take that names NO input clears the association, through the same transient.
        let (status, code, cleared) = take_router_input(&emulator, &output_arn, None).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: the clearing take answers 202, got {status} {code:?} with \
             {cleared:?}"
        );
        assert_eq!(
            taken(&cleared),
            (
                mc::ROUTED_STATE_ROUTING,
                output_arn.as_str(),
                output_name.as_str(),
                None,
                None,
            ),
            "case {case:?}: a take naming no input answers no input, got {cleared:?}"
        );

        tokio::time::sleep(past_transition_span()).await;
        let (_, _, body) = get_router_output(&emulator, &output_arn).await;
        assert_eq!(
            reported_route(&body),
            (mc::ROUTED_STATE_UNROUTED, None),
            "case {case:?}: the clock settles a cleared association into UNROUTED, got \
             {body:?}"
        );
        let (_, _, second_body) = get_router_input(&emulator, &second_arn).await;
        assert_eq!(
            reported_routed_outputs(&second_body),
            0,
            "case {case:?}: the cleared association is counted by nobody, got {second_body:?}"
        );

        // Clearing an association the output does not have changes nothing at all.
        let updated_before = stored_route(&emulator, &output_arn).1;
        let (status, code, nothing) = take_router_input(&emulator, &output_arn, None).await;
        assert_eq!(
            (status, code.as_deref(), taken(&nothing).0),
            (202, None, mc::ROUTED_STATE_UNROUTED),
            "case {case:?}: a take naming no input on an unrouted output answers UNROUTED, \
             got {status} {code:?} with {nothing:?}"
        );
        assert_eq!(
            stored_route(&emulator, &output_arn).1,
            updated_before,
            "case {case:?}: the no-op take persisted nothing, so UpdatedAt did not move"
        );
    }
}

/// A take resolves BOTH ARNs it carries against the store, and is refused while either
/// resource is not one it can associate: an ARN naming no router output or no router input
/// is the modelled `NotFoundException` (404) in this service's not-found envelope, an ARN
/// outside the modelled pattern is refused at the boundary (`BadRequestException`, 400), and
/// an output the clock still owes a state change — or either resource on its way out — is
/// the modelled `ConflictException` (409). No refusal persists anything.
#[tokio::test]
async fn a_take_refuses_an_arn_that_addresses_nothing_and_a_resource_that_is_not_settled() {
    let emulator = Emulator::new();
    let input_rows = minted_configuration_rows(&emulator, "refusal").await;
    let targets = minted_configuration_targets(&emulator, "refusal").await;
    let outputs = output_bodies(&targets);

    // The pair the positive control at the end of each block runs against.
    let (output_arn, _) = create_output(
        &emulator,
        "refusals",
        "refusal-output",
        outputs[0].1.clone(),
        mc::ROUTER_OUTPUT_STATE_STANDBY,
    )
    .await;
    let (input_arn, _) = create_input(
        &emulator,
        "refusals",
        "refusal-input",
        input_rows[0].3.clone(),
    )
    .await;

    // An ARN of the modelled form the store holds no ROUTER OUTPUT for.
    let unknown_outputs = [
        (
            "an output id this store never minted",
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
    for (case, arn) in unknown_outputs {
        let (status, code, body) = take_router_input(&emulator, arn, Some(&input_arn)).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: TakeRouterInput answers NotFoundException, got {status} {code:?} \
             with {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(mc::not_found_message(arn).as_str()),
            "case {case:?}: the refusal names the identifier the request carried, got {body:?}"
        );
    }

    // An ARN of the modelled form the store holds no ROUTER INPUT for.
    let unknown_inputs = [
        (
            "an input id this store never minted",
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
    ];
    for (case, arn) in unknown_inputs {
        let (status, code, body) = take_router_input(&emulator, &output_arn, Some(arn)).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: TakeRouterInput answers NotFoundException for the input, got \
             {status} {code:?} with {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(mc::not_found_message(arn).as_str()),
            "case {case:?}: the refusal names the input ARN the request carried, got {body:?}"
        );
        assert_eq!(
            stored_route(&emulator, &output_arn).0,
            None,
            "case {case:?}: the refusal left the output unrouted"
        );
    }

    // An ARN outside the modelled pattern never reaches the handler, on either member.
    let outside_the_pattern = [
        ("not an ARN at all", "router-output-1"),
        (
            "a router INPUT ARN in the output label",
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
        let (status, code, body) = take_router_input(&emulator, arn, Some(&input_arn)).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: the boundary refuses an output ARN outside the modelled pattern, \
             got {status} {code:?} with {body:?}"
        );
    }
    let outside_the_input_pattern = [
        ("not an ARN at all", "router-input-1"),
        (
            "a router OUTPUT ARN in the input member",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:000000000001",
        ),
        (
            "an id that is one character too long",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:0000000000011",
        ),
    ];
    for (case, arn) in outside_the_input_pattern {
        let (status, code, body) = take_router_input(&emulator, &output_arn, Some(arn)).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: the boundary refuses an input ARN outside the modelled pattern, \
             got {status} {code:?} with {body:?}"
        );
    }

    // An output the clock still owes its creation is between two states.
    let (creating_arn, _) = {
        let (status, code, created) = create_router_output(
            &emulator,
            {
                let mut body = outputs[1].1.clone();
                body["name"] = json!("creating-output");
                body
            },
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "CreateRouterOutput answers 201, got {status} {code:?} with {created:?}"
        );
        let output = created.get("routerOutput").cloned().unwrap_or(Value::Null);
        (
            output
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            output
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    };
    let (status, code, refused) =
        take_router_input(&emulator, &creating_arn, Some(&input_arn)).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "a take while the output is still being created is ConflictException, got {status} \
         {code:?} with {refused:?}"
    );
    assert_eq!(
        stored_route(&emulator, &creating_arn).0,
        None,
        "the refusal persisted no association, got {refused:?}"
    );
    // Once the create has settled the SAME call is accepted.
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, accepted) =
        take_router_input(&emulator, &creating_arn, Some(&input_arn)).await;
    assert_eq!(
        (status, code.as_deref(), taken(&accepted).0),
        (202, None, mc::ROUTED_STATE_ROUTING),
        "the take is accepted once the create has settled, got {status} {code:?} with \
         {accepted:?}"
    );

    // An INPUT whose delete has been accepted is not one an output can be routed to.
    let (doomed_input_arn, _) = create_input(
        &emulator,
        "deleting input",
        "doomed-input",
        input_rows[1].3.clone(),
    )
    .await;
    tokio::time::sleep(past_transition_span()).await;
    let (status, _, deleted) = delete_router_input(&emulator, &doomed_input_arn).await;
    assert_eq!(
        status, 200,
        "DeleteRouterInput accepts the delete, got {status} with {deleted:?}"
    );
    let (status, code, refused) =
        take_router_input(&emulator, &output_arn, Some(&doomed_input_arn)).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "a take of an input that is being deleted is ConflictException, got {status} {code:?} \
         with {refused:?}"
    );
    assert_eq!(
        stored_route(&emulator, &output_arn).0,
        None,
        "the refusal left the output unrouted, got {refused:?}"
    );
    // Once the clock has completed that deletion the same ARN addresses nothing.
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, gone) =
        take_router_input(&emulator, &output_arn, Some(&doomed_input_arn)).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "a take of a deleted input is NotFoundException, got {status} {code:?} with {gone:?}"
    );

    // An OUTPUT on its way out is not one a take can act on either, and once the clock has
    // completed its deletion the ARN addresses nothing.
    let (status, _, deleted) = delete_router_output(&emulator, &output_arn).await;
    assert_eq!(
        status, 200,
        "DeleteRouterOutput accepts the delete, got {status} with {deleted:?}"
    );
    let (status, code, refused) = take_router_input(&emulator, &output_arn, Some(&input_arn)).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "a take of an output that is being deleted is ConflictException, got {status} {code:?} \
         with {refused:?}"
    );
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, gone) = take_router_input(&emulator, &output_arn, Some(&input_arn)).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "a take of a deleted output is NotFoundException, got {status} {code:?} with {gone:?}"
    );
}

/// An association names a router input the store HOLDS: once the clock has completed the
/// deletion of that input the output carries nothing again, and both sides of the
/// association report it — the output is `UNROUTED` with no `RoutedInputArn`, and a take can
/// associate it with another input straight away.
#[tokio::test]
async fn the_clock_drops_an_association_into_a_router_input_it_has_removed() {
    let emulator = Emulator::new();
    let input_rows = minted_configuration_rows(&emulator, "dropped").await;
    let targets = minted_configuration_targets(&emulator, "dropped").await;
    let outputs = output_bodies(&targets);

    for index in 0..OUTPUT_BODY_COUNT {
        let (output_case, output_body) = &outputs[index];
        let row = &input_rows[index % input_rows.len()];
        let case = format!("{output_case} carrying {}", row.0);

        let (output_arn, _) = create_output(
            &emulator,
            &case,
            &format!("dropped-output-{index}"),
            output_body.clone(),
            mc::ROUTER_OUTPUT_STATE_ACTIVE,
        )
        .await;
        let (input_arn, _) = create_input(
            &emulator,
            &case,
            &format!("dropped-input-{index}"),
            row.3.clone(),
        )
        .await;
        let (replacement_arn, _) = create_input(
            &emulator,
            &case,
            &format!("replacement-input-{index}"),
            row.3.clone(),
        )
        .await;

        let (status, _, _) = take_router_input(&emulator, &output_arn, Some(&input_arn)).await;
        assert_eq!(status, 202, "case {case:?}: the take is accepted");
        tokio::time::sleep(past_transition_span()).await;

        let (status, _, deleted) = delete_router_input(&emulator, &input_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteRouterInput accepts the delete, got {deleted:?}"
        );
        // While the deletion is in flight the input is still in the store, so the
        // association still names it.
        let (_, _, body) = get_router_output(&emulator, &output_arn).await;
        assert_eq!(
            reported_route(&body),
            (mc::ROUTED_STATE_ROUTED, Some(input_arn.as_str())),
            "case {case:?}: the association survives while the input is still held, got \
             {body:?}"
        );

        tokio::time::sleep(past_transition_span()).await;
        let (status, code, body) = get_router_output(&emulator, &output_arn).await;
        assert_eq!(
            (status, code.as_deref(), reported_route(&body)),
            (200, None, (mc::ROUTED_STATE_UNROUTED, None)),
            "case {case:?}: the completed deletion took the association with it, got {status} \
             {code:?} with {body:?}"
        );

        // The output is free to take another input immediately.
        let (status, code, retaken) =
            take_router_input(&emulator, &output_arn, Some(&replacement_arn)).await;
        assert_eq!(
            (status, code.as_deref(), taken(&retaken).3),
            (202, None, Some(replacement_arn.as_str())),
            "case {case:?}: the output takes another input after the drop, got {status} \
             {code:?} with {retaken:?}"
        );
    }
}

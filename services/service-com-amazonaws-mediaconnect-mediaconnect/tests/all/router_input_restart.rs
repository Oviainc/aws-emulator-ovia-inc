// SPDX-License-Identifier: CC0-1.0

//! The router input lifecycle this change implements, end to end over HTTP:
//! CreateRouterInput -> GetRouterInput -> RestartRouterInput.
//!
//! The subject is RestartRouterInput: it must move the STORED router input its ARN
//! addresses into the non-terminal `RECOVERING` state and let the clock return it to the
//! operational state it was in — never answer a value fixed in the handler. The cases
//! below therefore vary what the rule generalises over: which of the five kinds of router
//! input configuration the input was created with, which operational state it was in when
//! the restart arrived, whether a state change was already in flight, and whether the ARN
//! addresses an input at all.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches: routing,
//! restJson1 deserialization, constraint validation, handler, serialization.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};


/// Long enough for a transition the clock owes to be due: twice
/// [`mc::TRANSITION_SPAN`], so a settled state is never read early.
///
/// `pub(crate)` because the router input START suite waits the same span out (see
/// `tests/all/router_input_start.rs`).
pub(crate) fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

/// Send one CreateRouterInput request.
///
/// `pub(crate)` because `tests/all/router_input_batch_get.rs` creates what it reads back
/// through the same helper, so the two suites cannot drift.
pub(crate) async fn create_router_input(
    emulator: &Emulator,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/routerInput", Some(body)).await
}

/// Send one GetRouterInput request. `pub(crate)` for the same reason as
/// [`create_router_input`].
pub(crate) async fn get_router_input(
    emulator: &Emulator,
    arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerInput/{arn}"), None)
        .await
}

async fn restart_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/routerInput/restart/{arn}"), None)
        .await
}

/// CreateRouterOutput, the operation the router OUTPUT slice owns: a router output is the
/// resource whose `RoutedInputArn` is what `RouterInput$RoutedOutputs` counts.
///
/// A router output's configuration must name a router network interface this account HOLDS,
/// so `interface_arn` is one minted over the wire
/// (`crate::router_outputs::minted_network_interface`).
async fn create_router_output(
    emulator: &Emulator,
    interface_arn: &str,
    name: &str,
    port: i64,
) -> String {
    let body = json!({
        "name": name,
        "configuration": union(
            "Standard",
            "standard",
            json!({
                "networkInterfaceArn": interface_arn,
                "protocolConfiguration": union(
                    "Rtp",
                    "rtp",
                    json!({ "destinationAddress": "198.51.100.10", "destinationPort": port }),
                ),
            }),
        ),
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    });
    let (status, _, created) = emulator.send("POST", "/v1/routerOutput", Some(body)).await;
    assert_eq!(
        status, 201,
        "CreateRouterOutput answers 201 for {name:?}, got {created}"
    );
    created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One union member, keyed BOTH by its Smithy member name and by its `@jsonName`.
///
/// AWS keys a union member on the wire by its `@jsonName` (`standard`); this emulator's
/// union codec matches by the Smithy MEMBER NAME (`Standard`), because the generated union
/// schema carries no `@jsonName` — the wire defect already filed as cause 1 of
/// `issues/things-that-do-not-work/20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md`,
/// which lives in the framework's union codec and not in any handler. A body that carries
/// both keys sets EXACTLY ONE member under either codec, because the key the codec does not
/// match is skipped as an unknown member (restJson1's forward-compatible rule) — so the
/// rows below assert the handler's own rule without depending on which key wins.
///
/// `pub(crate)` because `router_network_interfaces` builds its union payloads the same way.
pub(crate) fn union(member_name: &str, json_name: &str, value: Value) -> Value {
    json!({ member_name: value.clone(), json_name: value })
}

/// The value of union member `member_name` / `json_name` in `payload`, whichever key the
/// response carries (see [`union`]).
pub(crate) fn union_member<'a>(payload: &'a Value, member_name: &str, json_name: &str) -> &'a Value {
    payload
        .get(member_name)
        .or_else(|| payload.get(json_name))
        .unwrap_or(&Value::Null)
}

/// One row of [`configuration_rows`]: `(case, union member name, jsonName, configuration,
/// expected InputType)`.
pub(crate) type ConfigurationRow = (
    &'static str,
    &'static str,
    &'static str,
    Value,
    &'static str,
);

/// The five `RouterInputConfiguration` union members the model declares
/// (`smithy_query(query="shape",shape="RouterInputConfiguration")`: Standard,
/// MediaLiveChannel, Failover, MediaConnectFlow, Merge), as
/// `(case, member name, jsonName, configuration, expected InputType)`.
///
/// Every row is a DIFFERENT kind of input, which is what the input's `InputType` and
/// `StreamDetails` follow from, so the restart rule is asserted over the whole class rather
/// than over one configuration.
///
/// `interface_arn` is the router network interface a STANDARD / FAILOVER / MERGE input
/// ingests on and `flow_arn` the flow a MEDIACONNECT FLOW input takes its content from: both
/// are resources of THIS service, which the create resolves against the store, so both must
/// be ones the account HOLDS — see [`minted_configuration_rows`], which mints them over the
/// wire. The MediaLive channel belongs to another service, so it stays a constant.
///
/// `pub(crate)` because the router input START suite spans the same class (see
/// `tests/all/router_input_start.rs`), as does the batch read suite (see
/// `tests/all/router_input_batch_get.rs`), so there is one configuration table rather than
/// one per suite.
pub(crate) fn configuration_rows(interface_arn: &str, flow_arn: &str) -> Vec<ConfigurationRow> {
    let automatic_decryption = json!({
        "encryptionKeyType": "AUTOMATIC",
        "encryptionKeyConfiguration": union("Automatic", "automatic", json!({})),
    });
    vec![
        (
            "standard/rtp",
            "Standard",
            "standard",
            union(
                "Standard",
                "standard",
                json!({
                    "networkInterfaceArn": interface_arn,
                    "protocol": "RTP",
                    "protocolConfiguration": union(
                        "Rtp",
                        "rtp",
                        json!({ "port": 5000, "forwardErrorCorrection": "ENABLED" }),
                    ),
                }),
            ),
            "STANDARD",
        ),
        (
            "standard/srt-listener",
            "Standard",
            "standard",
            union(
                "Standard",
                "standard",
                json!({
                    "networkInterfaceArn": interface_arn,
                    "protocol": "SRT_LISTENER",
                    "protocolConfiguration": union(
                        "SrtListener",
                        "srtListener",
                        json!({ "port": 9000, "minimumLatencyMilliseconds": 120 }),
                    ),
                }),
            ),
            "STANDARD",
        ),
        (
            "failover",
            "Failover",
            "failover",
            union(
                "Failover",
                "failover",
                json!({
                    "networkInterfaceArn": interface_arn,
                    "sourcePriorityMode": "PRIMARY_SECONDARY",
                    "primarySourceIndex": 0,
                    // "A list of exactly two protocol configurations for the failover
                    // input sources. Both must use the same protocol type"
                    // (`smithy_query(query="shape",shape="FailoverRouterInputConfiguration"`,
                    // member `ProtocolConfigurations`).
                    "protocolConfigurations": [
                        union(
                            "Rist",
                            "rist",
                            json!({ "port": 4000, "recoveryLatencyMilliseconds": 200 }),
                        ),
                        union(
                            "Rist",
                            "rist",
                            json!({ "port": 4002, "recoveryLatencyMilliseconds": 200 }),
                        ),
                    ],
                }),
            ),
            "FAILOVER",
        ),
        (
            "merge",
            "Merge",
            "merge",
            union(
                "Merge",
                "merge",
                json!({
                    "networkInterfaceArn": interface_arn,
                    "mergeRecoveryWindowMilliseconds": 500,
                    // The same pair rule the merge input sources carry ("...Both must use
                    // the same protocol type").
                    "protocolConfigurations": [
                        union("Rtp", "rtp", json!({ "port": 5004 })),
                        union("Rtp", "rtp", json!({ "port": 5006 })),
                    ],
                }),
            ),
            "MERGE",
        ),
        (
            "mediaconnect-flow",
            "MediaConnectFlow",
            "mediaConnectFlow",
            union(
                "MediaConnectFlow",
                "mediaConnectFlow",
                json!({
                    "flowArn": flow_arn,
                    "sourceTransitDecryption": automatic_decryption.clone(),
                }),
            ),
            "MEDIACONNECT_FLOW",
        ),
        (
            "medialive-channel",
            "MediaLiveChannel",
            "mediaLiveChannel",
            union(
                "MediaLiveChannel",
                "mediaLiveChannel",
                json!({
                    "mediaLiveChannelArn": "arn:aws:medialive:us-east-1:000000000000:channel:1234567",
                    "mediaLivePipelineId": "PIPELINE_0",
                    "sourceTransitDecryption": automatic_decryption,
                }),
            ),
            "MEDIALIVE_CHANNEL",
        ),
    ]
}

/// [`configuration_rows`] against resources this account HOLDS: one router network interface
/// and one flow, both minted over the wire on `emulator` through the helpers the router
/// output suite owns, named after `prefix` so one emulator can hold several sets.
///
/// Every suite that creates a router input goes through this, because the create resolves
/// every reference of THIS service a configuration names against the store (see
/// `tests/all/router_input_configuration_targets.rs`).
pub(crate) async fn minted_configuration_rows(
    emulator: &Emulator,
    prefix: &str,
) -> Vec<ConfigurationRow> {
    let interface_arn =
        crate::router_outputs::minted_network_interface(emulator, &format!("rni-{prefix}")).await;
    let (flow_arn, _) =
        crate::router_outputs::minted_flow_source(emulator, &format!("flow-{prefix}")).await;
    configuration_rows(&interface_arn, &flow_arn)
}

/// A minimal valid CreateRouterInput body for `configuration`.
///
/// `pub(crate)` because the router input start and batch read suites create their inputs
/// the same way (see `tests/all/router_input_start.rs` and
/// `tests/all/router_input_batch_get.rs`).
pub(crate) fn create_body(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "configuration": configuration,
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "INPUT_20",
    })
}

/// Every ARN CreateRouterInput mints must satisfy the modelled `RouterInputArn` constraint
/// (`smithy_query(query="shape",shape="RouterInputArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerInput:[a-z0-9]{12}$`),
/// because GetRouterInput and RestartRouterInput take it as an `@httpLabel` and the
/// framework validates the member against that pattern.
///
/// The check runs through the crate's own encoding of that pattern and then re-asserts,
/// segment by segment, the parts the pattern fixes, so a bug in that function cannot hide
/// here.
fn assert_minted_router_input_arn(case: &str, arn: &str, id: &str) {
    assert!(
        mc::is_mintable_router_input_arn(arn),
        "case {case:?}: minted RouterInputArn must satisfy the modelled pattern, got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        7,
        "case {case:?}: a router input ARN has 7 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "routerInput"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert_eq!(
        segments[6], id,
        "case {case:?}: the last segment is the router input id, got {arn:?}"
    );
    assert!(
        id.len() == 12 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "case {case:?}: the router input id is 12 lowercase alphanumerics, got {id:?}"
    );
}

/// Put the stored router input `arn` into the settled state `state`, the way the state a
/// router input is in when a restart arrives is reached.
///
/// StopRouterInput moves an input into `STOPPING` rather than into an arbitrary state, so
/// the states an input other than a freshly created one is in are reached through the store
/// rather than through thousands of round trips. The record is left SETTLED (no transition
/// owed), exactly as a created input is once the clock has settled it.
///
/// `pub(crate)` because the router input start suite reaches the state a start arrives in
/// the same way (see `tests/all/router_input_start.rs`), as does the update suite (see
/// `tests/all/update_router_input.rs`).
pub(crate) fn settle_router_input_in(emulator: &Emulator, arn: &str, state: &str) {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let router_input = store
        .router_inputs
        .get_mut(arn)
        .expect("the router input the create minted is in the store");
    router_input.state = state.to_string();
    router_input.pending = None;
}

/// The state a stored router input is in, read straight from the store.
///
/// `pub(crate)` because the router input start suite asserts on the stored state too (see
/// `tests/all/router_input_start.rs`).
pub(crate) fn stored_state(emulator: &Emulator, arn: &str) -> String {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store
        .router_inputs
        .get(arn)
        .map(|router_input| router_input.state.clone())
        .unwrap_or_default()
}

/// Fill the store with `count` router inputs, the way the deletion suite seeds one.
///
/// The store ceiling is a bound on what is STORED, so the suite that asserts it needs a
/// full store rather than a thousand round trips: the calls that matter (the one that fills
/// the last slot, the ones past the bound) still go over the wire.
fn seed_router_inputs(emulator: &Emulator, count: usize) {
    for index in 0..count {
        crate::router_input_deletion::seed_router_input(
            emulator,
            &format!("seeded-router-input-{index}"),
            mc::ROUTER_INPUT_STATE_STANDBY,
        );
    }
}

/// A restart moves the addressed router input into `RECOVERING` and the clock returns it to
/// the operational state it was in — for every kind of router input and every operational
/// state it can be restarted from.
///
/// The rows vary the configuration kind (all five union members) and the state the input
/// was in. `ERROR` is the one state that is not the outcome: recovering from it is what the
/// operation is for, so an input restarted out of `ERROR` comes back `ACTIVE`.
#[tokio::test]
async fn restart_recovers_every_kind_of_router_input_and_the_clock_returns_it() {
    let emulator = Emulator::new();

    // (starting state, the state the clock owes after the restart).
    let state_rows = [
        (mc::ROUTER_INPUT_STATE_STANDBY, mc::ROUTER_INPUT_STATE_STANDBY),
        (mc::ROUTER_INPUT_STATE_ACTIVE, mc::ROUTER_INPUT_STATE_ACTIVE),
        (mc::ROUTER_INPUT_STATE_ERROR, mc::ROUTER_INPUT_STATE_ACTIVE),
    ];

    let mut restarted: Vec<(String, String, &str)> = Vec::new();
    for (kind, member_name, json_name, configuration, expected_input_type) in
        minted_configuration_rows(&emulator, "restart-kinds").await
    {
        for (started_in, expected_settled) in state_rows {
            let case = format!("{kind} from {started_in}");
            let name = format!("input-{kind}-{started_in}").replace('/', "-");
            let (status, code, created) =
                create_router_input(&emulator, create_body(&name, configuration.clone())).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: CreateRouterInput answers 201, got {created}"
            );
            let router_input = &created["routerInput"];
            let arn = router_input["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_minted_router_input_arn(
                &case,
                &arn,
                router_input["id"].as_str().unwrap_or_default(),
            );
            assert_eq!(
                router_input["inputType"].as_str(),
                Some(expected_input_type),
                "case {case:?}: InputType follows from the configuration union member, got \
                 {router_input}"
            );
            assert!(
                !union_member(&router_input["streamDetails"], member_name, json_name).is_null(),
                "case {case:?}: StreamDetails reports the {member_name} member, got \
                 {router_input}"
            );

            settle_router_input_in(&emulator, &arn, started_in);

            let (status, code, restarted_body) = restart_router_input(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: RestartRouterInput answers 202, got {restarted_body}"
            );
            assert_eq!(
                (
                    restarted_body["arn"].as_str(),
                    restarted_body["name"].as_str(),
                    restarted_body["state"].as_str(),
                ),
                (
                    Some(arn.as_str()),
                    Some(name.as_str()),
                    Some(mc::ROUTER_INPUT_STATE_RECOVERING),
                ),
                "case {case:?}: the restart reports the addressed input in RECOVERING, got \
                 {restarted_body}"
            );

            // The state is PERSISTED, not just reported: the reader computes it from the
            // same record.
            let (status, _, read_back) = get_router_input(&emulator, &arn).await;
            assert_eq!(
                (status, read_back["routerInput"]["state"].as_str()),
                (200, Some(mc::ROUTER_INPUT_STATE_RECOVERING)),
                "case {case:?}: GetRouterInput reports the restart in flight, got {read_back}"
            );

            restarted.push((case, arn, expected_settled));
        }
    }

    // One wait for every row: each restart owes its state change at its own instant, and
    // all of them are due once a transition span has passed.
    tokio::time::sleep(past_transition_span()).await;

    for (case, arn, expected_settled) in restarted {
        let (status, code, read_back) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref(), read_back["routerInput"]["state"].as_str()),
            (200, None, Some(expected_settled)),
            "case {case:?}: the clock returns the input to its operational state, got \
             {read_back}"
        );
        // A read does not move it again: the second reader sees what the first one did.
        let (_, _, read_again) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            read_again["routerInput"]["state"].as_str(),
            Some(expected_settled),
            "case {case:?}: reading twice reports one state, got {read_again}"
        );
    }
}

/// A created router input is answered in the non-terminal `CREATING` state and the clock
/// settles it into `STANDBY`; a restart is refused with `ConflictException` (409) for as
/// long as a state change is in flight — the create's, or a restart's own.
///
/// This is the whole clock path through the wire, where the table above seeds the state it
/// starts from: it proves the transient state is really observable, that the refusal
/// persists nothing, and that the input is restartable again once the transition settles.
#[tokio::test]
async fn a_restart_is_refused_while_a_state_change_is_in_flight() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "in-flight")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    let (status, _, created) =
        create_router_input(&emulator, create_body("in-flight", configuration)).await;
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        (status, created["routerInput"]["state"].as_str()),
        (201, Some(mc::ROUTER_INPUT_STATE_CREATING)),
        "a created router input is answered in CREATING, got {created}"
    );

    // The create's transition is still in flight, so the restart conflicts with it.
    let (status, code, refused) = restart_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "case \"restart while creating\": expected the modelled ConflictException, got {refused}"
    );
    assert_eq!(
        stored_state(&emulator, &arn),
        mc::ROUTER_INPUT_STATE_CREATING,
        "case \"restart while creating\": the refusal persisted nothing, got {refused}"
    );

    // Once the clock has settled the create, the input is in STANDBY: created, not running.
    tokio::time::sleep(past_transition_span()).await;
    let (_, _, read_back) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        read_back["routerInput"]["state"].as_str(),
        Some(mc::ROUTER_INPUT_STATE_STANDBY),
        "case \"created input settles\": the clock settles CREATING into STANDBY, got \
         {read_back}"
    );

    let (status, _, accepted) = restart_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, accepted["state"].as_str()),
        (202, Some(mc::ROUTER_INPUT_STATE_RECOVERING)),
        "case \"restart a settled input\": the restart is accepted, got {accepted}"
    );

    // A second restart conflicts with the first one, for the same reason.
    let (status, code, refused) = restart_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "case \"restart while recovering\": expected the modelled ConflictException, got \
         {refused}"
    );
    assert_eq!(
        stored_state(&emulator, &arn),
        mc::ROUTER_INPUT_STATE_RECOVERING,
        "case \"restart while recovering\": the refusal persisted nothing, got {refused}"
    );

    tokio::time::sleep(past_transition_span()).await;
    let (_, _, read_back) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        read_back["routerInput"]["state"].as_str(),
        Some(mc::ROUTER_INPUT_STATE_STANDBY),
        "case \"restart settles\": the clock returns the input to STANDBY, got {read_back}"
    );
}

/// An ARN that addresses no router input is answered with the modelled
/// `NotFoundException` (404) by both the restart and the reader, and one that does not
/// satisfy the modelled `RouterInputArn` pattern is refused at the boundary with
/// `BadRequestException` (400) — never a 501 and never a success.
#[tokio::test]
async fn an_arn_that_addresses_no_router_input_is_not_found() {
    let emulator = Emulator::new();

    // Rows differ in WHY the ARN resolves nothing: an id this store never minted, another
    // account's, another Region's — each a well-formed `RouterInputArn`.
    let unresolved = [
        (
            "never minted",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:00000000beef",
        ),
        (
            "another account",
            "arn:aws:mediaconnect:us-east-1:999999999999:routerInput:0000000000a1",
        ),
        (
            "another region",
            "arn:aws:mediaconnect:eu-west-1:000000000000:routerInput:0000000000a1",
        ),
        (
            "another partition",
            "arn:aws-us-gov:mediaconnect:us-east-1:000000000000:routerInput:0000000000a1",
        ),
    ];
    for (case, arn) in unresolved {
        for (operation, answered) in [
            ("RestartRouterInput", restart_router_input(&emulator, arn).await),
            ("GetRouterInput", get_router_input(&emulator, arn).await),
        ] {
            let (status, code, body) = answered;
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?} ({operation}): expected the modelled NotFoundException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(mc::not_found_message(arn).as_str()),
                "case {case:?} ({operation}): the message names the identifier the request \
                 carried, got {body}"
            );
        }
    }

    // Rows that are not `RouterInputArn`s at all: the id is the one part the pattern fixes
    // exactly (twelve lowercase alphanumerics), so these are refused before any handler
    // runs.
    let malformed = [
        ("id too short", "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:0000000000a"),
        ("id too long", "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:0000000000a1b"),
        ("id not lowercase", "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:0000000000A1"),
        ("not an arn", "routerInput:0000000000a1"),
    ];
    for (case, arn) in malformed {
        let (status, code, body) = restart_router_input(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: an ARN failing the modelled pattern is refused at the boundary, \
             got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal carries this service's validation envelope, got {body}"
        );
    }
}

/// A created router input round-trips through GetRouterInput: every member the create
/// carried is reported back, and every member it did not carry is the one the service
/// derives.
///
/// The rows vary what the request leaves out, which is what the derivation generalises
/// over.
#[tokio::test]
async fn a_created_router_input_round_trips_through_get() {
    let emulator = Emulator::new();
    let (_, _, _, standard_configuration, _) = minted_configuration_rows(&emulator, "round-trip")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    // Everything the model lets a create carry.
    let mut body = create_body("fully-specified", standard_configuration.clone());
    body["regionName"] = json!(REGION);
    body["availabilityZone"] = json!(format!("{REGION}b"));
    body["tags"] = json!({ "stage": "test", "team": "video" });
    body["maintenanceConfiguration"] = union(
        "PreferredDayTime",
        "preferredDayTime",
        json!({ "day": "TUESDAY", "time": "02:00" }),
    );
    body["transitEncryption"] = json!({
        "encryptionKeyType": "SECRETS_MANAGER",
        "encryptionKeyConfiguration": union(
            "SecretsManager",
            "secretsManager",
            json!({
                "secretArn": "arn:aws:secretsmanager:us-east-1:000000000000:secret:transit",
                "roleArn": "arn:aws:iam::000000000000:role/MediaConnectTransit",
            }),
        ),
    });
    body["contentQualityAnalysisConfiguration"] = union(
        "ContentLevel",
        "contentLevel",
        json!({ "blackFrames": { "state": "ENABLED", "thresholdSeconds": 10 } }),
    );
    body["clientToken"] = json!("token-fully-specified");

    let (status, _, created) = create_router_input(&emulator, body.clone()).await;
    assert_eq!(status, 201, "the fully specified create is accepted: {created}");
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, read_back) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "GetRouterInput answers 200 for the input the create minted, got {read_back}"
    );
    let stored = &read_back["routerInput"];
    assert_eq!(
        (
            stored["name"].as_str(),
            stored["tier"].as_str(),
            stored["routingScope"].as_str(),
            stored["maximumBitrate"].as_i64(),
            stored["regionName"].as_str(),
            stored["availabilityZone"].as_str(),
        ),
        (
            Some("fully-specified"),
            Some("INPUT_20"),
            Some("REGIONAL"),
            Some(20_000_000),
            Some(REGION),
            Some(format!("{REGION}b").as_str()),
        ),
        "case \"fully specified\": every member the create carried is reported back, got \
         {stored}"
    );
    assert_eq!(
        stored["tags"],
        json!({ "stage": "test", "team": "video" }),
        "case \"fully specified\": the tags the create carried, got {stored}"
    );
    assert_eq!(
        (
            stored["maintenanceType"].as_str(),
            union_member(
                &stored["maintenanceConfiguration"],
                "PreferredDayTime",
                "preferredDayTime",
            )["day"]
                .as_str(),
            stored["transitEncryption"]["encryptionKeyType"].as_str(),
            stored["contentQualityAnalysisType"].as_str(),
        ),
        (
            Some("PREFERRED_DAY_TIME"),
            Some("TUESDAY"),
            Some("SECRETS_MANAGER"),
            Some("CONTENT_LEVEL"),
        ),
        "case \"fully specified\": MaintenanceType follows from the configuration union \
         member, got {stored}"
    );
    assert_eq!(
        (
            stored["routedOutputs"].as_i64(),
            stored["messages"].as_array().map(Vec::len),
            stored["ipAddress"].as_str().is_some(),
        ),
        (Some(0), Some(0), true),
        "case \"fully specified\": no output is routed to a new input, it reports no \
         messages, and an input that ingests on a network interface has an address, got \
         {stored}"
    );
    assert_eq!(
        stored, &created["routerInput"],
        "case \"fully specified\": the reader reports the record the create stored, got \
         {stored}"
    );

    // Repeating the create with the SAME `@idempotencyToken` answers with the input that
    // token already minted rather than minting a second one.
    let (status, _, repeated) = create_router_input(&emulator, body).await;
    assert_eq!(
        (status, repeated["routerInput"]["arn"].as_str()),
        (201, Some(arn.as_str())),
        "case \"repeated client token\": the create is idempotent, got {repeated}"
    );

    // The members a create may leave out are the ones the service derives.
    let (status, _, defaulted) = create_router_input(
        &emulator,
        create_body("defaulted", standard_configuration),
    )
    .await;
    assert_eq!(status, 201, "the minimal create is accepted: {defaulted}");
    let defaulted_arn = defaulted["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        defaulted_arn, arn,
        "case \"defaulted\": a create with no client token mints its own input"
    );
    let (_, _, read_back) = get_router_input(&emulator, &defaulted_arn).await;
    let stored = &read_back["routerInput"];
    assert_eq!(
        (
            stored["regionName"].as_str(),
            stored["availabilityZone"].as_str(),
            stored["maintenanceType"].as_str(),
            stored["transitEncryption"]["encryptionKeyType"].as_str(),
            stored["contentQualityAnalysisType"].as_str(),
        ),
        (
            Some(REGION),
            Some(format!("{REGION}a").as_str()),
            Some("DEFAULT"),
            Some("AUTOMATIC"),
            Some("CONTENT_LEVEL"),
        ),
        "case \"defaulted\": the Region of the request, its first zone, default \
         maintenance and automatic transit encryption, got {stored}"
    );
    assert_eq!(
        stored["tags"],
        json!({}),
        "case \"defaulted\": an input created with no tags carries none, got {stored}"
    );
}

/// `MaximumBitrate` may not exceed what the input's tier allows: "The tier sets the maximum
/// bitrate, in Mbps, for the router input. Valid values include the following: INPUT_20 –
/// 20 Mbps, INPUT_50 – 50 Mbps, INPUT_100 – 100 Mbps, INPUT_250 – 250 Mbps, INPUT_500 – 500
/// Mbps" (`smithy_query(query="doc",shape="CreateRouterInputRequest",member="Tier")`).
///
/// The catalog below is that published list, written out here rather than read from the
/// crate, so a bug in the crate's own encoding of it cannot hide. Every tier is tested at
/// its published bound and one bit per second past it, and a bitrate that names no capacity
/// is refused too.
#[tokio::test]
async fn a_router_input_may_not_exceed_the_bitrate_its_tier_allows() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "tier-bitrate")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    let published_mbps = [
        ("INPUT_20", 20i64),
        ("INPUT_50", 50),
        ("INPUT_100", 100),
        ("INPUT_250", 250),
        ("INPUT_500", 500),
    ];
    for (tier, mbps) in published_mbps {
        let ceiling = mbps * 1_000_000;
        assert_eq!(
            mc::router_input_tier_bitrate_ceiling(tier),
            Some(ceiling),
            "case {tier:?}: the crate's ceiling is the published one"
        );

        let mut at_bound = create_body(&format!("at-{tier}"), configuration.clone());
        at_bound["tier"] = json!(tier);
        at_bound["maximumBitrate"] = json!(ceiling);
        let (status, code, accepted) = create_router_input(&emulator, at_bound).await;
        assert_eq!(
            (status, code.as_deref(), accepted["routerInput"]["maximumBitrate"].as_i64()),
            (201, None, Some(ceiling)),
            "case {tier:?} at its bound: the create is accepted and reports the bitrate it \
             was asked for, got {accepted}"
        );

        for (case, bitrate) in [
            ("one past the bound", ceiling + 1),
            ("ten times the bound", ceiling * 10),
        ] {
            let mut past_bound = create_body(&format!("past-{tier}"), configuration.clone());
            past_bound["tier"] = json!(tier);
            past_bound["maximumBitrate"] = json!(bitrate);
            let (status, code, refused) = create_router_input(&emulator, past_bound).await;
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {tier:?} {case}: a bitrate above the tier's ceiling is refused, got \
                 {refused}"
            );
        }
    }

    for (case, bitrate) in [("zero", 0i64), ("negative", -1)] {
        let mut no_capacity = create_body(&format!("bitrate-{case}"), configuration.clone());
        no_capacity["maximumBitrate"] = json!(bitrate);
        let (status, code, refused) = create_router_input(&emulator, no_capacity).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a bitrate that names no capacity is refused, got {refused}"
        );
    }

    // Nothing a refused create named reached the store: the inputs it holds are exactly the
    // accepted ones, one per tier.
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let held = store.lock().router_inputs.len();
    assert_eq!(
        held,
        published_mbps.len(),
        "a refused create stores nothing: expected one input per tier, got {held}"
    );
}

/// `RouterInput$RoutedOutputs` is "the number of outputs currently routed to this input":
/// a count COMPUTED from the router outputs the store holds whose `RoutedInputArn` is this
/// input, never a constant.
///
/// The rows vary that count over the class: none of the stored outputs routed to the input,
/// one of them, both of them, and then one again — including an output routed to a
/// DIFFERENT input, which this input must not count.
#[tokio::test]
async fn routed_outputs_counts_the_router_outputs_routed_to_the_input() {
    let emulator = Emulator::new();
    let output_interface =
        crate::router_outputs::minted_network_interface(&emulator, "rni-ri-routed").await;
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "routed-outputs")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    let (_, _, counted) =
        create_router_input(&emulator, create_body("counted", configuration.clone())).await;
    let arn = counted["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (_, _, other) =
        create_router_input(&emulator, create_body("other", configuration)).await;
    let other_arn = other["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let first_output =
        create_router_output(&emulator, &output_interface, "ro-counted-1", 5006).await;
    let second_output =
        create_router_output(&emulator, &output_interface, "ro-counted-2", 5008).await;
    let third_output = create_router_output(&emulator, &output_interface, "ro-other", 5010).await;

    // TakeRouterInput is the operation that routes an output to an input, and it is still
    // scaffolded, so the relation is set on the stored output records the way that call
    // would set it. The third output is routed to the OTHER input throughout.
    let route = |routed_to_counted: &[&str]| {
        let store = emulator
            .engine
            .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
        let mut store = store.lock();
        for (output_arn, router_output) in store.router_outputs.iter_mut() {
            router_output.routed_input_arn = if routed_to_counted.contains(&output_arn.as_str()) {
                Some(arn.clone())
            } else if output_arn == &third_output {
                Some(other_arn.clone())
            } else {
                None
            };
        }
    };

    let rows: [(&str, Vec<&str>, i64); 4] = [
        ("no output routed", Vec::new(), 0),
        ("one output routed", vec![first_output.as_str()], 1),
        (
            "both outputs routed",
            vec![first_output.as_str(), second_output.as_str()],
            2,
        ),
        ("one output unrouted again", vec![second_output.as_str()], 1),
    ];
    for (case, routed, expected) in rows {
        route(&routed);
        let (status, _, read_back) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, read_back["routerInput"]["routedOutputs"].as_i64()),
            (200, Some(expected)),
            "case {case:?}: RoutedOutputs counts the outputs routed to this input, got \
             {read_back}"
        );
        // The output routed to the other input is counted there, and never here.
        let (_, _, other_read) = get_router_input(&emulator, &other_arn).await;
        assert_eq!(
            other_read["routerInput"]["routedOutputs"].as_i64(),
            Some(1),
            "case {case:?}: another input counts only its own routed output, got {other_read}"
        );
    }
}

/// A restart is refused while the addressed input is on its way OUT: DeleteRouterInput has
/// accepted a delete, so the clock owes the record's removal and the input is not one a
/// later call can still act on — the same `ConflictException` (409) an input mid-transition
/// is refused with. Once the removal has come due, the ARN addresses nothing and the
/// restart is the modelled `NotFoundException` (404).
#[tokio::test]
async fn a_restart_is_refused_while_the_input_is_being_deleted() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "being-deleted")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    let (_, _, created) =
        create_router_input(&emulator, create_body("deleted", configuration)).await;
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    tokio::time::sleep(past_transition_span()).await;

    let (status, _, deleted) = emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await;
    assert_eq!(
        (status, deleted["state"].as_str()),
        (200, Some(mc::ROUTER_INPUT_STATE_DELETING)),
        "case \"delete accepted\": the input is reported DELETING, got {deleted}"
    );

    let (status, code, refused) = restart_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "case \"restart while deleting\": expected the modelled ConflictException, got {refused}"
    );
    assert_eq!(
        stored_state(&emulator, &arn),
        mc::ROUTER_INPUT_STATE_DELETING,
        "case \"restart while deleting\": the refusal did not cancel the deletion, got {refused}"
    );

    // The deletion comes due: the record stops existing, because `RouterInputState`
    // declares no terminal state a deleted input could be reported in.
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, gone) = restart_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "case \"restart after the removal came due\": expected the modelled \
         NotFoundException, got {gone}"
    );
}

/// CreateRouterInput refuses past the store ceiling with the modelled
/// `RouterInputServiceQuotaExceededException` (420), the only quota error the model declares
/// on it — the bound that keeps a create loop from growing the store without limit (see
/// `mc::MAX_ROUTER_INPUTS`).
///
/// The rows that matter are the one that fills the LAST slot and the one past the bound;
/// the store is filled through the store itself, because a bound on what is STORED needs a
/// full store rather than a thousand round trips.
#[tokio::test]
async fn create_refuses_past_the_router_input_store_ceiling() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "store-ceiling")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    // One slot short of the ceiling, so the next create over the wire fills it.
    seed_router_inputs(&emulator, mc::MAX_ROUTER_INPUTS - 1);

    let (status, code, last) = create_router_input(
        &emulator,
        create_body("fills-the-last-slot", configuration.clone()),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"fills the last slot\": the create that reaches the ceiling is accepted, got \
         {last}"
    );

    for case in ["one past the ceiling", "two past the ceiling"] {
        let (status, code, refused) =
            create_router_input(&emulator, create_body("past-the-ceiling", configuration.clone()))
                .await;
        assert_eq!(
            (status, code.as_deref()),
            (420, Some("RouterInputServiceQuotaExceededException")),
            "case {case:?}: expected the modelled quota error, got {refused}"
        );
    }

    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let held = store.lock().router_inputs.len();
    assert_eq!(
        held,
        mc::MAX_ROUTER_INPUTS,
        "a refused create stores nothing: the store holds the ceiling, got {held}"
    );
}

/// An Availability Zone a create names must belong to the input's Region: "This must be a
/// valid Availability Zone for the region specified by regionName, or the current region if
/// no regionName is provided"
/// (`smithy_query(query="doc",shape="CreateRouterInputRequest",member="AvailabilityZone")`).
///
/// AWS names a Region's Availability Zone by the Region followed by one lowercase letter,
/// which is the shape `mc::is_zone_of_region` encodes for every router resource — so the
/// rows below span a zone of the signing Region, a zone of a named Region, a zone of a
/// different Region, the bare Region name, and a `RegionName` that is not a Region at all.
#[tokio::test]
async fn an_availability_zone_must_belong_to_the_router_inputs_region() {
    let emulator = Emulator::new();
    let (_, _, _, configuration, _) = minted_configuration_rows(&emulator, "zone")
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");

    // (case, regionName, availabilityZone, accepted).
    let rows = [
        ("zone of the signing Region", None, Some(format!("{REGION}c")), true),
        (
            "zone of the named Region",
            Some("eu-west-1".to_string()),
            Some("eu-west-1b".to_string()),
            true,
        ),
        (
            "the Region itself, with no zone letter",
            Some("us-west-2".to_string()),
            Some("us-west-2".to_string()),
            false,
        ),
        (
            "zone of another Region",
            Some("eu-west-1".to_string()),
            Some("us-east-1a".to_string()),
            false,
        ),
        (
            "zone of another Region than the signing one",
            None,
            Some("ap-south-1a".to_string()),
            false,
        ),
        (
            "not a Region name",
            Some("Not A Region".to_string()),
            None,
            false,
        ),
    ];
    for (case, region_name, availability_zone, accepted) in rows {
        let mut body = create_body(&format!("zone-{case}").replace(' ', "-"), configuration.clone());
        if let Some(region_name) = &region_name {
            body["regionName"] = json!(region_name);
        }
        if let Some(availability_zone) = &availability_zone {
            body["availabilityZone"] = json!(availability_zone);
        }
        let (status, code, answered) = create_router_input(&emulator, body).await;
        if accepted {
            assert_eq!(
                (
                    status,
                    code.as_deref(),
                    answered["routerInput"]["availabilityZone"].as_str(),
                    answered["routerInput"]["regionName"].as_str(),
                ),
                (
                    201,
                    None,
                    availability_zone.as_deref(),
                    Some(region_name.as_deref().unwrap_or(REGION)),
                ),
                "case {case:?}: the create is accepted and reports the Region and zone it \
                 was asked for, got {answered}"
            );
            // The ARN's Region segment is the Region the request was SIGNED for — the
            // control-plane endpoint the call reached — whatever Region the input itself
            // lives in, so an input of another Region is still addressed through this
            // endpoint's ARN.
            let arn = answered["routerInput"]["arn"].as_str().unwrap_or_default();
            assert_minted_router_input_arn(
                case,
                arn,
                answered["routerInput"]["id"].as_str().unwrap_or_default(),
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: the create is refused, got {answered}"
            );
        }
    }
}

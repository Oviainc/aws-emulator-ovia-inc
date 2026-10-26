// SPDX-License-Identifier: CC0-1.0

//! GetRouterOutput, end to end over HTTP: `GET /v1/routerOutput/{Arn}`.
//!
//! The rule the cases below pin: this reporting read resolves its `@httpLabel` ARN against
//! the STORE and answers the router output that ARN addresses — "Retrieves information about
//! a specific router output in AWS Elemental MediaConnect"
//! (`smithy_query(query="doc",shape="GetRouterOutput")`, `@readonly`) — every member computed
//! from that one record, so it agrees member for member with the OTHER read side of the same
//! record (BatchGetRouterOutput), and nothing a later caller sees is changed by the read
//! except a state change the CLOCK had already made due.
//!
//! The rows vary what that rule generalises over: which `RouterOutputConfiguration` variant
//! (and so which output type and stream details) the addressed output carries, which output
//! protocol a STANDARD one is configured with, its tier across the published catalog and the
//! bitrate that tier allows, the size of its tag set, how many outputs the store holds beside
//! the addressed one, which state the stored record is in (the whole model-generated
//! `RouterOutputState` catalog), whether a transition the store owes has come due, and how
//! often the read is repeated.
//!
//! Every call travels the whole request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`]. The create helper, the minted configuration targets,
//! the STANDARD configuration fixture, the store seeder and the persisted-member list are the
//! ones `router_outputs` owns — the suite that pins CreateRouterOutput's own rules — so the
//! router output suites cannot drift apart; what this suite layers on top is the agreement
//! between the two read sides, the stored state catalog a reader must report back, and what a
//! read leaves behind.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::ROUTER_OUTPUT_STATE_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_outputs::{
    batch_get_router_outputs, create_router_output, minted_configuration_targets,
    seed_router_outputs, standard, MEDIALIVE_INPUT_ARN, PERSISTED_MEMBERS, ROLE_ARN, SECRET_ARN,
};

/// The published tier catalog, as `RouterOutput$Tier` documents it
/// (`smithy_query(query="doc",shape="RouterOutput",member="Tier")`: "OUTPUT_20 – 20 Mbps,
/// OUTPUT_50 – 50 Mbps, OUTPUT_100 – 100 Mbps, OUTPUT_250 – 250 Mbps, OUTPUT_500 – 500
/// Mbps"), written out here rather than read from the crate so the rows below span the whole
/// published list and a bug in the crate's own encoding of it cannot hide here.
const TIERS: [&str; 5] = [
    "OUTPUT_20",
    "OUTPUT_50",
    "OUTPUT_100",
    "OUTPUT_250",
    "OUTPUT_500",
];

/// Long enough that a transition the clock owes a freshly created output is certainly due:
/// twice `mc::TRANSITION_SPAN`, so a settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

/// Far enough ahead that a transition the store owes is NOT due during a case, and far enough
/// behind that one certainly is — the two sides of the instant a reader compares against.
const NOT_DUE_FOR_A_WHILE: Duration = Duration::from_secs(600);
/// See [`NOT_DUE_FOR_A_WHILE`].
const DUE_A_WHILE_AGO: Duration = Duration::from_secs(600);

/// The operation under test (`smithy_query(query="operation",operation="GetRouterOutput")`:
/// `GET /v1/routerOutput/{Arn}`).
async fn get_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

/// RestartRouterOutput — a call that MUTATES the record, which is what the non-mutation case
/// below contrasts the reads against.
async fn restart_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/routerOutput/restart/{arn}"), None)
        .await
}

/// The whole stored record for `arn` as the store holds it, for comparing a before and an
/// after: a read must leave every member of it exactly as it was.
fn stored_record(emulator: &Emulator, arn: &str) -> Option<String> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .router_outputs
        .get(arn)
        .map(|router_output| format!("{router_output:?}"))
}

/// The state the store holds for `arn`, or `None` when it holds no router output for it.
fn stored_state(emulator: &Emulator, arn: &str) -> Option<String> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .router_outputs
        .get(arn)
        .map(|router_output| router_output.state.clone())
}

/// The instant the store holds as `arn`'s last update — read from the record rather than from
/// the response, so the comparison has the clock's full resolution.
fn stored_updated_at(emulator: &Emulator, arn: &str) -> Option<SystemTime> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .router_outputs
        .get(arn)
        .map(|router_output| router_output.updated_at)
}

/// Whether the store still owes `arn` a state change.
fn transition_is_owed(emulator: &Emulator, arn: &str) -> bool {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store
        .router_outputs
        .get(arn)
        .is_some_and(|router_output| router_output.pending.is_some())
}

/// Every router output ARN the store holds, in the store's own stable order.
fn stored_arns(emulator: &Emulator) -> Vec<String> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store.router_outputs.keys().cloned().collect()
}

/// Owe the stored router output `arn` the state change `settled`, entering `transient` now and
/// due at `due_at` — the record a call that starts a transition leaves behind
/// (`mc::StoredRouterOutput::begin_transition` writes exactly this, at
/// `now + mc::TRANSITION_SPAN`).
///
/// Written onto the record so the cases below do not race the create's own span: what they
/// assert is what the READER does with a transition that is owed and not yet due, and with one
/// the clock has already made due. The real span is exercised over the wire by
/// [`get_reports_the_stored_record_and_agrees_with_the_batch_read_side`].
fn owe_transition(
    emulator: &Emulator,
    arn: &str,
    transient: &str,
    settled: &str,
    due_at: SystemTime,
) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let router_output = store
        .router_outputs
        .get_mut(arn)
        .expect("the store holds the router output under test");
    router_output.state = transient.to_string();
    router_output.pending = Some(mc::PendingStatus {
        status: settled.to_string(),
        due_at,
    });
}

/// Put one router output into the store named `name` and settled in `state`.
///
/// The record itself comes from the shared seeder `router_outputs` owns, so there is one
/// spelling of a seeded router output; the two members this suite varies are written onto it
/// afterwards, because a create only ever answers `CREATING` and no implemented operation
/// reaches most of the modelled state catalog.
fn seed_router_output_in_state(emulator: &Emulator, name: &str, state: &str) -> String {
    let before = stored_arns(emulator);
    seed_router_outputs(emulator, 1);
    let arn = stored_arns(emulator)
        .into_iter()
        .find(|arn| !before.contains(arn))
        .expect("the seeder stored one router output");

    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let router_output = store
        .router_outputs
        .get_mut(&arn)
        .expect("the seeded router output is in the store");
    router_output.name = name.to_string();
    router_output.state = state.to_string();
    arn
}

/// A reported `RouterOutput` without its `State`, so two reports of one record can be compared
/// across an instant at which the clock may have settled a transition it owed.
fn without_state(router_output: &Value) -> Value {
    let mut copy = router_output.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("state");
    }
    copy
}

/// One create, and what the reader must report back for it beyond the create's own answer.
struct Row {
    case: &'static str,
    body: Value,
    /// `RouterOutput$OutputType`, which the service derives from the configuration's union
    /// variant rather than reading from the request.
    output_type: &'static str,
    tier: &'static str,
    maximum_bitrate: i64,
    tags: Value,
    /// The `routerOutput` the create answered, filled in once the row has been created.
    created: Value,
    /// The ARN the create minted for it.
    arn: String,
}

/// The rows: every `RouterOutputConfiguration` variant the model declares, three of the five
/// output protocols for the STANDARD one (including a listener, which names no destination
/// address at all), each at a different tier of the published catalog and at the bitrate that
/// tier allows, with tag sets differing in size.
fn rows(interface_arn: &str, flow_arn: &str, flow_source_arn: &str) -> Vec<Row> {
    let tier_bitrate = |tier: &str| {
        mc::router_output_tier_bitrate_ceiling(tier)
            .expect("every published tier has a published ceiling")
    };
    vec![
        Row {
            case: "standard RTP",
            body: json!({
                "name": "get-ro-rtp",
                "configuration": standard(interface_arn, json!({"Rtp": {
                    "destinationAddress": "198.51.100.20",
                    "destinationPort": 5004,
                    "forwardErrorCorrection": "ENABLED",
                }})),
                "maximumBitrate": tier_bitrate(TIERS[0]),
                "routingScope": "REGIONAL",
                "tier": TIERS[0],
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            tier: TIERS[0],
            maximum_bitrate: tier_bitrate(TIERS[0]),
            tags: json!({}),
            created: Value::Null,
            arn: String::new(),
        },
        Row {
            case: "standard SRT listener, one tag",
            body: json!({
                "name": "get-ro-srt-listener",
                "configuration": standard(interface_arn, json!({"SrtListener": {
                    "port": 3100,
                    "minimumLatencyMilliseconds": 120,
                }})),
                "maximumBitrate": tier_bitrate(TIERS[1]),
                "routingScope": "GLOBAL",
                "tier": TIERS[1],
                "tags": {"stage": "one"},
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            tier: TIERS[1],
            maximum_bitrate: tier_bitrate(TIERS[1]),
            tags: json!({"stage": "one"}),
            created: Value::Null,
            arn: String::new(),
        },
        Row {
            case: "standard SRT caller, two tags and a maintenance preference",
            body: json!({
                "name": "get-ro-srt-caller",
                "configuration": standard(interface_arn, json!({"SrtCaller": {
                    "destinationAddress": "198.51.100.21",
                    "destinationPort": 9000,
                    "minimumLatencyMilliseconds": 150,
                    "streamId": "primary",
                }})),
                "maximumBitrate": tier_bitrate(TIERS[2]),
                "routingScope": "REGIONAL",
                "tier": TIERS[2],
                "availabilityZone": "us-east-1c",
                "tags": {"stage": "two", "team": "video"},
                "maintenanceConfiguration": {"PreferredDayTime": {"day": "FRIDAY",
                    "time": "02:00"}},
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            tier: TIERS[2],
            maximum_bitrate: tier_bitrate(TIERS[2]),
            tags: json!({"stage": "two", "team": "video"}),
            created: Value::Null,
            arn: String::new(),
        },
        Row {
            case: "MediaConnect flow destination",
            body: json!({
                "name": "get-ro-mediaconnect-flow",
                "configuration": {"MediaConnectFlow": {
                    "flowArn": flow_arn,
                    "flowSourceArn": flow_source_arn,
                    "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
                        "encryptionKeyConfiguration": {"Automatic": {}}},
                }},
                "maximumBitrate": tier_bitrate(TIERS[3]),
                "routingScope": "REGIONAL",
                "tier": TIERS[3],
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW,
            tier: TIERS[3],
            maximum_bitrate: tier_bitrate(TIERS[3]),
            tags: json!({}),
            created: Value::Null,
            arn: String::new(),
        },
        Row {
            case: "MediaLive input destination, one tag",
            body: json!({
                "name": "get-ro-medialive-input",
                "configuration": {"MediaLiveInput": {
                    "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
                    "mediaLivePipelineId": "PIPELINE_0",
                    "destinationTransitEncryption": {"encryptionKeyType": "SECRETS_MANAGER",
                        "encryptionKeyConfiguration": {"SecretsManager": {
                            "secretArn": SECRET_ARN, "roleArn": ROLE_ARN}}},
                }},
                "maximumBitrate": tier_bitrate(TIERS[4]),
                "routingScope": "GLOBAL",
                "tier": TIERS[4],
                "tags": {"team": "video"},
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_MEDIALIVE_INPUT,
            tier: TIERS[4],
            maximum_bitrate: tier_bitrate(TIERS[4]),
            tags: json!({"team": "video"}),
            created: Value::Null,
            arn: String::new(),
        },
    ]
}

/// Create every row on `emulator` and fill in what each create answered.
async fn created_rows(emulator: &Emulator, prefix: &str) -> Vec<Row> {
    let targets = minted_configuration_targets(emulator, prefix).await;
    let mut built = rows(
        &targets.interface_arn,
        &targets.flow_arn,
        &targets.flow_source_arn,
    );
    for row in &mut built {
        let case = row.case;
        let (status, code, created) = create_router_output(emulator, row.body.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: CreateRouterOutput answers 201, got {status} {code:?}: {created}"
        );
        row.arn = created["routerOutput"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create must report an ARN, got {created}"))
            .to_string();
        row.created = created["routerOutput"].clone();
    }
    built
}

/// The reader answers the record its ARN addresses — for every configuration variant, tier and
/// tag set — and that answer is the SAME record the batch read side reports for the same ARN,
/// member for member, because both are computed from the one stored router output.
#[tokio::test]
async fn get_reports_the_stored_record_and_agrees_with_the_batch_read_side() {
    let emulator = Emulator::new();
    let built = created_rows(&emulator, "get-ro").await;

    // Past the span the create's own CREATING -> STANDBY transition is owed at, so every read
    // below sees one settled record rather than a state the clock is still moving.
    tokio::time::sleep(past_transition_span()).await;

    for row in &built {
        let case = row.case;
        let arn = row.arn.as_str();
        let (status, code, body) = get_router_output(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: GetRouterOutput answers 200 for a router output the store holds, \
             got {status} {code:?}: {body}"
        );
        let reported = &body["routerOutput"];
        assert_eq!(
            (
                reported["arn"].as_str(),
                reported["name"].as_str(),
                reported["outputType"].as_str(),
                reported["tier"].as_str(),
                reported["maximumBitrate"].as_i64(),
                reported["regionName"].as_str(),
                reported["state"].as_str(),
            ),
            (
                Some(arn),
                row.body["name"].as_str(),
                Some(row.output_type),
                Some(row.tier),
                Some(row.maximum_bitrate),
                Some(REGION),
                Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
            ),
            "case {case:?}: the reader reports this output's own identity, configuration and \
             settled state, got {reported}"
        );
        assert_eq!(
            reported["id"].as_str(),
            arn.split(':').next_back(),
            "case {case:?}: Id is the id the addressing ARN carries, got {reported}"
        );
        assert_eq!(
            reported["tags"], row.tags,
            "case {case:?}: the tags this output was created with, got {reported}"
        );
        for member in PERSISTED_MEMBERS {
            assert_eq!(
                reported[member], row.created[member],
                "case {case:?}: the reader reports {member} as the create stored it, got {:?}",
                reported[member]
            );
        }

        // The other read side of the same record, in one request naming every stored ARN: a
        // reader that computed anything of its own would disagree with it here.
        let every_arn: Vec<&str> = built.iter().map(|row| row.arn.as_str()).collect();
        let (status, code, batch) = batch_get_router_outputs(&emulator, &every_arn).await;
        assert_eq!(
            (status, code.as_deref(), &batch["errors"]),
            (200, None, &json!([])),
            "case {case:?}: BatchGetRouterOutput serves every stored ARN, got {status} \
             {code:?}: {batch}"
        );
        let same_record = batch["routerOutputs"]
            .as_array()
            .and_then(|outputs| {
                outputs
                    .iter()
                    .find(|output| output["arn"].as_str() == Some(arn))
            })
            .unwrap_or_else(|| panic!("case {case:?}: the batch read reports {arn}, got {batch}"));
        assert_eq!(
            reported, same_record,
            "case {case:?}: the two read sides of one record report the same router output, \
             got {reported} and {same_record}"
        );
    }

    // No two reads answered the same record: every ARN addresses its own router output, though
    // the store holds one per row.
    let mut reported_ids = Vec::new();
    for row in &built {
        let (_, _, body) = get_router_output(&emulator, &row.arn).await;
        reported_ids.push(body["routerOutput"]["id"].as_str().unwrap_or("").to_string());
    }
    let mut distinct = reported_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        built.len(),
        "every ARN addresses a distinct record, got {reported_ids:?}"
    );
}

/// The `State` the reader reports is the one the STORE holds — for every value of the modelled
/// `RouterOutputState` enum, including the states no implemented operation reaches yet — and
/// reading them all moves none of them.
///
/// The catalog is the model's own, taken from the constant this crate GENERATES from the model
/// (`ops::types::ROUTER_OUTPUT_STATE_VALUES`), so a value the model adds is a row here without
/// an edit. The records are seeded because a create only ever answers `CREATING`.
#[tokio::test]
async fn the_reader_reports_the_state_the_store_holds() {
    let emulator = Emulator::new();

    assert!(
        ROUTER_OUTPUT_STATE_VALUES.len() >= 2,
        "the modelled state catalog must have rows, got {ROUTER_OUTPUT_STATE_VALUES:?}"
    );
    let mut seeded: Vec<(&str, String, String)> = Vec::new();
    for state in ROUTER_OUTPUT_STATE_VALUES.iter().copied() {
        let name = format!("state-{state}");
        let arn = seed_router_output_in_state(&emulator, &name, state);
        seeded.push((state, name, arn));
    }

    for (state, name, arn) in &seeded {
        let (status, code, body) = get_router_output(&emulator, arn).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                body["routerOutput"]["state"].as_str(),
                body["routerOutput"]["name"].as_str(),
                body["routerOutput"]["arn"].as_str(),
            ),
            (200, None, Some(*state), Some(name.as_str()), Some(arn.as_str())),
            "case {state:?}: the reader reports the stored state, name and ARN, got {body}"
        );
    }

    // Reading them all left every record in the state it was seeded in: a read reports a
    // state, it never moves one.
    for (state, _, arn) in &seeded {
        assert_eq!(
            stored_state(&emulator, arn).as_deref(),
            Some(*state),
            "case {state:?}: the stored state is unchanged by the reads, got {:?}",
            stored_state(&emulator, arn)
        );
    }
}

/// A read changes nothing a later caller sees: repeated reads of one output answer the same
/// record and leave the stored record byte for byte as it was, a read of one output leaves
/// another's report alone, and the store keeps exactly the records the creates minted.
///
/// The one write the reporting operation does make is the state change the CLOCK had already
/// made due — owed at its own instant whether or not anybody called — which is why a read
/// taken before the due instant still reports the transient state and leaves it owed, and one
/// taken after it materialises the settled state. Moving `UpdatedAt` is what a MUTATION does,
/// which no read above did.
#[tokio::test]
async fn reading_a_router_output_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let built = created_rows(&emulator, "read-ro").await;
    let subject = built[0].arn.clone();
    let neighbour = built[1].arn.clone();
    let neighbour_created = built[1].created.clone();

    // A transition the store owes and the clock has NOT made due: every read reports the
    // transient state, and none of them settles the record early.
    owe_transition(
        &emulator,
        &subject,
        mc::ROUTER_OUTPUT_STATE_CREATING,
        mc::ROUTER_OUTPUT_STATE_STANDBY,
        SystemTime::now() + NOT_DUE_FOR_A_WHILE,
    );
    let before = stored_record(&emulator, &subject);
    let (_, _, first) = get_router_output(&emulator, &subject).await;
    let (_, _, second) = get_router_output(&emulator, &subject).await;
    assert_eq!(
        first, second,
        "case \"two reads mid-transition\": the second read answers what the first did, got \
         {second}"
    );
    assert_eq!(
        first["routerOutput"]["state"].as_str(),
        Some(mc::ROUTER_OUTPUT_STATE_CREATING),
        "case \"two reads mid-transition\": the transient state is observable, got {first}"
    );
    assert_eq!(
        stored_record(&emulator, &subject),
        before,
        "case \"two reads mid-transition\": the reads left the stored record exactly as it \
         was, got {:?}",
        stored_record(&emulator, &subject)
    );
    assert!(
        transition_is_owed(&emulator, &subject),
        "case \"two reads mid-transition\": the owed transition is still owed after the reads"
    );

    // The same transition, now one the clock HAS made due: the read materialises it, so the
    // next caller sees the settled state although nothing but the clock moved it.
    owe_transition(
        &emulator,
        &subject,
        mc::ROUTER_OUTPUT_STATE_CREATING,
        mc::ROUTER_OUTPUT_STATE_STANDBY,
        SystemTime::now() - DUE_A_WHILE_AGO,
    );
    let (_, _, settled) = get_router_output(&emulator, &subject).await;
    assert_eq!(
        settled["routerOutput"]["state"].as_str(),
        Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
        "case \"read after the change came due\": the clock's change is materialised, got \
         {settled}"
    );
    assert_eq!(
        stored_state(&emulator, &subject).as_deref(),
        Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
        "case \"read after the change came due\": the settled state is PERSISTED, not just \
         reported, got {:?}",
        stored_state(&emulator, &subject)
    );
    assert!(
        !transition_is_owed(&emulator, &subject),
        "case \"read after the change came due\": the store owes the output nothing further"
    );

    // From here the record is settled, so repeated reads must be identical answers over an
    // untouched record — including the two timestamps.
    let settled_record = stored_record(&emulator, &subject);
    let settled_updated_at = stored_updated_at(&emulator, &subject);
    for attempt in 1..=3 {
        let (_, _, again) = get_router_output(&emulator, &subject).await;
        assert_eq!(
            again, settled,
            "case \"repeated reads of a settled record\" (attempt {attempt}): one record, \
             reported identically, got {again}"
        );
        assert_eq!(
            stored_record(&emulator, &subject),
            settled_record,
            "case \"repeated reads of a settled record\" (attempt {attempt}): the stored \
             record is untouched, got {:?}",
            stored_record(&emulator, &subject)
        );
        assert_eq!(
            stored_updated_at(&emulator, &subject),
            settled_updated_at,
            "case \"repeated reads of a settled record\" (attempt {attempt}): a read never \
             moves UpdatedAt, got {:?}",
            stored_updated_at(&emulator, &subject)
        );
    }

    // Reading one output left another one's record alone.
    let (_, _, neighbour_read) = get_router_output(&emulator, &neighbour).await;
    assert_eq!(
        without_state(&neighbour_read["routerOutput"]),
        without_state(&neighbour_created),
        "case \"the neighbour\": the reads of another output changed nothing here, got \
         {neighbour_read}"
    );

    // The store holds exactly the records the creates minted: no read added, removed or
    // duplicated one.
    let mut held = stored_arns(&emulator);
    held.sort();
    let mut expected: Vec<String> = built.iter().map(|row| row.arn.clone()).collect();
    expected.sort();
    assert_eq!(
        held, expected,
        "the reads left the store holding exactly the created outputs, got {held:?}"
    );

    // A MUTATION is what moves the record, which no read above did.
    let (status, code, restarted) = restart_router_output(&emulator, &subject).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "the restart that mutates the record is accepted, got {status} {code:?}: {restarted}"
    );
    assert!(
        stored_updated_at(&emulator, &subject) > settled_updated_at,
        "case \"read after a mutation\": a mutation moves UpdatedAt, got {:?} after {:?}",
        stored_updated_at(&emulator, &subject),
        settled_updated_at
    );
    let (_, _, after_restart) = get_router_output(&emulator, &subject).await;
    assert_eq!(
        after_restart["routerOutput"]["state"].as_str(),
        Some(mc::ROUTER_OUTPUT_STATE_RECOVERING),
        "case \"read after a mutation\": the reader reports the change, got {after_restart}"
    );
    assert_eq!(
        after_restart["routerOutput"]["createdAt"],
        built[0].created["createdAt"],
        "case \"read after a mutation\": CreatedAt is the instant the output was created, got \
         {after_restart}"
    );
}

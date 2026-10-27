// SPDX-License-Identifier: CC0-1.0

//! GetRouterInputThumbnail, end to end over HTTP.
//!
//! The rule the cases below pin: a reporting `GET /v1/routerInput/{Arn}/thumbnail` resolves
//! its `@httpLabel` ARN against the STORE and reports the thumbnail this service holds for
//! the router input that ARN addresses — the stored input's own ARN and name, and one coded
//! `thumbnailMessages` entry per condition that stands between that input and a thumbnail,
//! computed from its STORED state and configuration. An ARN the store holds no input for is
//! refused with the modelled `NotFoundException` (404). Nothing in this emulator ingests
//! media, so no frame has ever been received: `thumbnail`, `timecode` and `timestamp` are
//! absent for every input rather than carrying a synthesised picture, a timecode of a frame
//! nothing decoded or a generation instant for work nothing did, and the read leaves what a
//! later caller sees unchanged except for a state change the clock had already made due.
//!
//! Each table's rows differ along what the rule generalises over: which kind of
//! `RouterInputConfiguration` the addressed input carries (all five union members), whether
//! that configuration is CONNECTED to the source it takes content from, which state the
//! stored record is in (the whole modelled `RouterInputState` catalog), how many inputs the
//! store holds beside the addressed one, how often the read is repeated, and the way an ARN
//! can address nothing.
//!
//! Every call travels the whole request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`]; records whose state a create cannot reach are put
//! there through the shared [`crate::router_input_deletion::seed_router_input`] and
//! [`crate::router_input_restart::settle_router_input_in`], read back through
//! [`crate::router_input_restart::stored_state`] and taken away through
//! [`crate::get_router_input::owe_removal`], the way the other router input suites reach
//! them.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::flow_lifecycle::{Emulator, REGION};
use crate::get_router_input::owe_removal;
use crate::router_input_configuration_targets::minted_flow_output;
use crate::router_input_deletion::seed_router_input;
use crate::router_input_restart::{
    create_body, minted_configuration_rows, settle_router_input_in, stored_state, union,
};

/// The `RouterInputMessage$Code` values this operation reports, written out here rather than
/// read from the crate so that a change to the handler's constants cannot quietly move what
/// these tests assert.
///
/// `ROUTER_INPUT_NOT_RUNNING` is the code for an input that is not `ACTIVE`,
/// `SOURCE_NOT_CONNECTED` the code for an input whose configuration is not connected to the
/// source it takes its content from, and `NO_THUMBNAIL_AVAILABLE` the code for a running,
/// connected input nothing has arrived on.
const CODE_ROUTER_INPUT_NOT_RUNNING: &str = "ROUTER_INPUT_NOT_RUNNING";
/// See [`CODE_ROUTER_INPUT_NOT_RUNNING`].
const CODE_SOURCE_NOT_CONNECTED: &str = "SOURCE_NOT_CONNECTED";
/// See [`CODE_ROUTER_INPUT_NOT_RUNNING`].
const CODE_NO_THUMBNAIL_AVAILABLE: &str = "NO_THUMBNAIL_AVAILABLE";

/// Every value of the modelled `RouterInputState` enum
/// (`smithy_query(query="shape",shape="RouterInputState")`: CREATING | STANDBY | STARTING |
/// ACTIVE | STOPPING | DELETING | UPDATING | ERROR | RECOVERING | MIGRATING).
///
/// The reporting rule reads the input's state, so the whole catalog is a row of the state
/// table below — including the states no implemented operation reaches yet.
const ROUTER_INPUT_STATES: [&str; 10] = [
    "CREATING",
    "STANDBY",
    "STARTING",
    "ACTIVE",
    "STOPPING",
    "DELETING",
    "UPDATING",
    "ERROR",
    "RECOVERING",
    "MIGRATING",
];

/// The one state in which an input is carrying content, so the one in which a thumbnail could
/// be reported at all (`smithy_query(query="shape",shape="RouterInputState")`).
const STATE_ACTIVE: &str = "ACTIVE";
/// The settled state a created input waits in, used as the non-running row of the tables
/// below.
const STATE_STANDBY: &str = "STANDBY";

/// Far enough in the past that a removal the store owes is certainly due.
const DUE_A_WHILE_AGO: Duration = Duration::from_secs(600);

/// The operation under test: `GET /v1/routerInput/{Arn}/thumbnail`
/// (`smithy_query(query="operation",operation="GetRouterInputThumbnail")`).
async fn thumbnail(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerInput/{arn}/thumbnail"), None)
        .await
}

/// `GET /v1/routerInput/{Arn}` — the implemented reader this suite reads a created input
/// back through (`smithy_query(query="operation",operation="GetRouterInput")`).
async fn get_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerInput/{arn}"), None)
        .await
}

/// Create one router input and answer its minted ARN.
async fn create_router_input(emulator: &Emulator, case: &str, body: Value) -> String {
    let (status, code, created) = emulator.send("POST", "/v1/routerInput", Some(body)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture router input must be created, got {status} {code:?}: \
         {created}"
    );
    created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a created router input carries its ARN"))
        .to_string()
}

/// The `code` of every entry of a `thumbnailMessages` list, in the order it reports them.
fn codes(messages: &Value) -> Vec<String> {
    messages
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert that every entry of `messages` carries wording beside its code, so the list is a
/// status report rather than bare codes.
fn assert_messages_are_worded(case: &str, messages: &Value) {
    let entries = messages.as_array().cloned().unwrap_or_default();
    assert!(
        !entries.is_empty(),
        "case {case:?}: the required message list states why there is no thumbnail, got \
         {messages}"
    );
    for message in entries {
        assert!(
            message["message"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
            "case {case:?}: every message carries wording, got {message}"
        );
    }
}

/// Assert that no image, timecode or generation instant is claimed for work nothing did.
fn assert_no_image_is_claimed(case: &str, details: &Value) {
    assert!(
        details["thumbnail"].is_null()
            && details["timecode"].is_null()
            && details["timestamp"].is_null(),
        "case {case:?}: no frame has been received, so the image, its timecode and its \
         generation instant stay absent, got {details}"
    );
}

/// The automatic transit decryption every source-taking configuration below carries
/// (`sourceTransitDecryption` is required on both of them).
fn automatic_decryption() -> Value {
    json!({
        "encryptionKeyType": "AUTOMATIC",
        "encryptionKeyConfiguration": union("Automatic", "automatic", json!({})),
    })
}

/// A MEDIACONNECT FLOW configuration naming `flow_arn` and `flow_output_arn`, each only
/// where the case carries one.
///
/// "To connect the router input to the flow output, provide both `flowArn` and
/// `flowOutputArn`. To disconnect it, call the update operation and omit both fields while
/// keeping this configuration object"
/// (`smithy_query(query="shape",shape="MediaConnectFlowRouterInputConfiguration")`), so the
/// rows below vary exactly those two members.
fn mediaconnect_flow(flow_arn: Option<&str>, flow_output_arn: Option<&str>) -> Value {
    let mut payload = json!({ "sourceTransitDecryption": automatic_decryption() });
    if let Some(flow_arn) = flow_arn {
        payload["flowArn"] = json!(flow_arn);
    }
    if let Some(flow_output_arn) = flow_output_arn {
        payload["flowOutputArn"] = json!(flow_output_arn);
    }
    union("MediaConnectFlow", "mediaConnectFlow", payload)
}

/// A MEDIALIVE CHANNEL configuration carrying the channel and pipeline, and the channel
/// output name only where the case carries one.
///
/// "To connect the router input to the MediaLive channel output, provide all three of
/// `mediaLiveChannelArn`, `mediaLivePipelineId`, and `mediaLiveChannelOutputName`. To
/// disconnect it, call the update operation and omit all three fields while keeping this
/// configuration object"
/// (`smithy_query(query="shape",shape="MediaLiveChannelRouterInputConfiguration")`).
fn medialive_channel(output_name: Option<&str>) -> Value {
    let mut payload = json!({
        "mediaLiveChannelArn": "arn:aws:medialive:us-east-1:000000000000:channel:1234567",
        "mediaLivePipelineId": "PIPELINE_0",
        "sourceTransitDecryption": automatic_decryption(),
    });
    if let Some(output_name) = output_name {
        payload["mediaLiveChannelOutputName"] = json!(output_name);
    }
    union("MediaLiveChannel", "mediaLiveChannel", payload)
}

/// The report answers the router input its ARN addresses — for every kind of router input the
/// model declares — and claims no image for any of them.
///
/// The rows are the five `RouterInputConfiguration` union members (the create table
/// `router_input_restart` owns), each created with its own name, so every member of the
/// response has to be computed per record. A created input is not running yet, so the rule
/// says every row's message list states that condition.
#[tokio::test]
async fn the_thumbnail_report_answers_the_router_input_its_arn_addresses() {
    let emulator = Emulator::new();

    let mut rows: Vec<(String, String, String)> = Vec::new();
    for (kind, _member_name, _json_name, configuration, _input_type) in
        minted_configuration_rows(&emulator, "thumb-kinds").await
    {
        let name = format!("thumb-{kind}").replace('/', "-");
        let arn = create_router_input(&emulator, kind, create_body(&name, configuration)).await;
        rows.push((kind.to_string(), name, arn));
    }

    for (case, name, arn) in &rows {
        let (status, code, body) = thumbnail(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the report answers 200 for an input the store holds, got {status} \
             {code:?}: {body}"
        );
        assert_eq!(
            (body["arn"].as_str(), body["name"].as_str()),
            (Some(arn.as_str()), Some(name.as_str())),
            "case {case:?}: the report names the stored input's own ARN and name, got {body}"
        );
        let details = &body["thumbnailDetails"];
        assert_no_image_is_claimed(case, details);
        assert_messages_are_worded(case, &details["thumbnailMessages"]);
        assert!(
            codes(&details["thumbnailMessages"])
                .iter()
                .any(|code| code == CODE_ROUTER_INPUT_NOT_RUNNING),
            "case {case:?}: a created input is not running, so the report states that \
             condition, got {body}"
        );
    }

    // No two reports answered the same record, though the store holds one input per kind.
    let mut reported: Vec<String> = Vec::new();
    for (_, _, arn) in &rows {
        let (_, _, body) = thumbnail(&emulator, arn).await;
        reported.push(body["arn"].as_str().unwrap_or_default().to_string());
    }
    let mut distinct = reported.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        rows.len(),
        "every ARN addresses a distinct record, got {reported:?}"
    );
}

/// The condition the report states follows the STATE the store holds: an input that is not
/// `ACTIVE` is not carrying content, and the one that is has had nothing arrive on it.
///
/// The rows are the model's own state catalog, including the states no implemented operation
/// reaches yet; the records are seeded because a create only ever answers `CREATING`. Each
/// seeded input ingests on a router network interface, so its configuration is connected and
/// the state is the only thing the rows vary.
#[tokio::test]
async fn the_thumbnail_report_follows_the_state_the_store_holds() {
    let emulator = Emulator::new();

    let mut seeded: Vec<(&str, String)> = Vec::new();
    for state in ROUTER_INPUT_STATES {
        let arn = seed_router_input(&emulator, &format!("thumb-state-{state}"), state);
        seeded.push((state, arn));
    }

    for (state, arn) in &seeded {
        let (status, code, body) = thumbnail(&emulator, arn).await;
        let expected = if *state == STATE_ACTIVE {
            vec![CODE_NO_THUMBNAIL_AVAILABLE.to_string()]
        } else {
            vec![CODE_ROUTER_INPUT_NOT_RUNNING.to_string()]
        };
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {state:?}: the report answers 200, got {status} {code:?}: {body}"
        );
        assert_eq!(
            codes(&body["thumbnailDetails"]["thumbnailMessages"]),
            expected,
            "case {state:?}: the codes follow the stored state, got {body}"
        );
        assert_messages_are_worded(state, &body["thumbnailDetails"]["thumbnailMessages"]);
        assert_no_image_is_claimed(state, &body["thumbnailDetails"]);
        assert_eq!(
            body["name"].as_str(),
            Some(format!("thumb-state-{state}").as_str()),
            "case {state:?}: the report names the stored input, got {body}"
        );
    }

    // Reading them all left every record in the state it was seeded in: the report reports a
    // state, it never moves one.
    for (state, arn) in &seeded {
        assert_eq!(
            stored_state(&emulator, arn),
            *state,
            "case {state:?}: the report leaves the stored state alone"
        );
    }
}

/// The condition the report states also follows whether the stored CONFIGURATION is connected
/// to the source it takes its content from — so an input of the same state reports a different
/// list depending on what its configuration names, and an input that is both stopped and
/// disconnected reports both conditions.
///
/// The rows vary the configuration kind, which of its connecting members it carries (the
/// documented connect/disconnect rule of each source-taking configuration), and the state the
/// record is settled in — so they differ in the codes AND in how many messages the report
/// carries.
#[tokio::test]
async fn the_thumbnail_report_follows_whether_the_configuration_is_connected_to_a_source() {
    let emulator = Emulator::new();

    let interface_arn =
        crate::router_outputs::minted_network_interface(&emulator, "rni-thumb-conn").await;
    let (flow_arn, flow_output_arn) = minted_flow_output(&emulator, "flow-thumb-conn").await;

    /// One configuration, and whether it is connected to the source it takes content from.
    struct Row {
        case: &'static str,
        configuration: Value,
        connected: bool,
    }

    let rows = [
        Row {
            case: "STANDARD ingesting on a router network interface",
            configuration: union(
                "Standard",
                "standard",
                json!({
                    "networkInterfaceArn": interface_arn,
                    "protocol": "RTP",
                    "protocolConfiguration": union("Rtp", "rtp", json!({ "port": 5000 })),
                }),
            ),
            connected: true,
        },
        Row {
            case: "MEDIACONNECT FLOW naming both the flow and its output",
            configuration: mediaconnect_flow(Some(&flow_arn), Some(&flow_output_arn)),
            connected: true,
        },
        Row {
            case: "MEDIACONNECT FLOW naming the flow alone",
            configuration: mediaconnect_flow(Some(&flow_arn), None),
            connected: false,
        },
        Row {
            case: "MEDIACONNECT FLOW naming neither, the disconnected form",
            configuration: mediaconnect_flow(None, None),
            connected: false,
        },
        Row {
            case: "MEDIALIVE CHANNEL naming the channel, pipeline and output",
            configuration: medialive_channel(Some("thumb-output")),
            connected: true,
        },
        Row {
            case: "MEDIALIVE CHANNEL naming no channel output",
            configuration: medialive_channel(None),
            connected: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let name = format!("thumb-conn-{index}");
        let arn = create_router_input(
            &emulator,
            row.case,
            create_body(&name, row.configuration.clone()),
        )
        .await;

        // Running: the only condition left for a connected input is that nothing has
        // arrived; a disconnected one has no source to decode a thumbnail from at all.
        settle_router_input_in(&emulator, &arn, STATE_ACTIVE);
        let (status, code, body) = thumbnail(&emulator, &arn).await;
        let expected = if row.connected {
            vec![CODE_NO_THUMBNAIL_AVAILABLE.to_string()]
        } else {
            vec![CODE_SOURCE_NOT_CONNECTED.to_string()]
        };
        assert_eq!(
            (
                status,
                code.as_deref(),
                codes(&body["thumbnailDetails"]["thumbnailMessages"]),
            ),
            (200, None, expected),
            "case {:?} (running): the codes follow what the stored configuration connects, \
             got {body}",
            row.case
        );
        assert_messages_are_worded(row.case, &body["thumbnailDetails"]["thumbnailMessages"]);

        // Not running: the state condition is reported first, and a disconnected input
        // reports BOTH conditions.
        settle_router_input_in(&emulator, &arn, STATE_STANDBY);
        let (status, code, body) = thumbnail(&emulator, &arn).await;
        let expected = if row.connected {
            vec![CODE_ROUTER_INPUT_NOT_RUNNING.to_string()]
        } else {
            vec![
                CODE_ROUTER_INPUT_NOT_RUNNING.to_string(),
                CODE_SOURCE_NOT_CONNECTED.to_string(),
            ]
        };
        assert_eq!(
            (
                status,
                code.as_deref(),
                codes(&body["thumbnailDetails"]["thumbnailMessages"]),
            ),
            (200, None, expected),
            "case {:?} (not running): every condition that holds is reported, got {body}",
            row.case
        );
        assert_no_image_is_claimed(row.case, &body["thumbnailDetails"]);
    }
}

/// The report is a READ: repeating it answers the same body, and it changes neither the state
/// nor the `updatedAt` instant a later caller sees.
///
/// The record is settled first, so no transition the clock owes can move anything underneath
/// the repeats; the rows are the number of repeats.
#[tokio::test]
async fn the_thumbnail_report_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    let interface_arn =
        crate::router_outputs::minted_network_interface(&emulator, "rni-thumb-read").await;
    let configuration = union(
        "Standard",
        "standard",
        json!({
            "networkInterfaceArn": interface_arn,
            "protocol": "RTP",
            "protocolConfiguration": union("Rtp", "rtp", json!({ "port": 5000 })),
        }),
    );
    let arn = create_router_input(
        &emulator,
        "repeated reads",
        create_body("thumb-read", configuration),
    )
    .await;
    settle_router_input_in(&emulator, &arn, STATE_STANDBY);

    let (_, _, before) = get_router_input(&emulator, &arn).await;
    let (_, _, first) = thumbnail(&emulator, &arn).await;
    for repeat in 1..=3 {
        let (status, code, again) = thumbnail(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case \"repeat {repeat}\": the report answers 200, got {status} {code:?}: {again}"
        );
        assert_eq!(
            again, first,
            "case \"repeat {repeat}\": a read answers the same report, got {again}"
        );
    }

    let (_, _, after) = get_router_input(&emulator, &arn).await;
    assert_eq!(
        (
            after["routerInput"]["state"].as_str(),
            after["routerInput"]["updatedAt"].as_str(),
        ),
        (
            before["routerInput"]["state"].as_str(),
            before["routerInput"]["updatedAt"].as_str(),
        ),
        "case \"repeated reads\": the reports left the record exactly as it was, got {after}"
    );
}

/// An ARN the store holds no router input for is the modelled `NotFoundException` (404) in
/// this service's not-found envelope — never a 501, never a success.
///
/// The rows differ along the way the ARN addresses nothing: never minted, another account,
/// another Region and partition, and an input this store did hold whose removal the clock has
/// already made due.
#[tokio::test]
async fn an_arn_the_store_holds_no_router_input_for_has_no_thumbnail() {
    let emulator = Emulator::new();
    // One live input, so a refusal below cannot be an empty store answering everything the
    // same way.
    let live = seed_router_input(&emulator, "thumb-live", STATE_ACTIVE);

    let mut unresolved = vec![
        (
            "a router input ARN of the minted form that was never minted",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:routerInput:ri0000beef31"),
        ),
        (
            "a router input ARN of another account",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:routerInput:ri0000beef32"),
        ),
        (
            "a router input ARN of another Region and partition",
            "arn:aws-cn:mediaconnect:cn-north-1:000000000000:routerInput:ri0000beef33".to_string(),
        ),
    ];

    let deleted = seed_router_input(&emulator, "thumb-deleted", STATE_STANDBY);
    owe_removal(&emulator, &deleted, SystemTime::now() - DUE_A_WHILE_AGO);
    unresolved.push(("an input whose removal the clock has made due", deleted));

    for (case, arn) in unresolved {
        let (status, code, body) = thumbnail(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected the modelled NotFoundException, got {status} {code:?}: \
             {body}"
        );
        // The message is built from the request: it names the identifier the caller asked
        // about, whatever the wording around it.
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(arn.as_str())),
            "case {case:?}: the message names the identifier the request carried, got {body}"
        );
    }

    // The live input is still reported, so the refusals above were about the ARNs.
    let (status, code, body) = thumbnail(&emulator, &live).await;
    assert_eq!(
        (status, code.as_deref(), body["name"].as_str()),
        (200, None, Some("thumb-live")),
        "case \"the live input beside them\": still reported, got {status} {code:?}: {body}"
    );
}

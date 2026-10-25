// SPDX-License-Identifier: CC0-1.0

//! What `DescribeContact` DERIVES about a contact: the dataflows of its pass, and the
//! stage of its pass the clock has carried it to.
//!
//! Two RULES are under test, each over a SET of inputs rather than one recorded call.
//!
//! * A contact's `dataflowList` is one `DataflowDetail` per dataflow edge of the mission
//!   profile it is reserved against, in that profile's edge order, each end reported by
//!   the type and the id of the config the edge named there, with the `configDetails` the
//!   `ConfigDetails` union has a member for (the dataflow endpoint of this account the
//!   config names, the output node an `antenna-downlink-demod-decode` edge was wired to,
//!   the bucket and key template of an `s3-recording` config) and the region that end of
//!   the dataflow sits in. The table below spans config types at either end, an edge
//!   naming one output node of a config and the same config named whole, a destination in
//!   a region of its own, several edges in one profile and a profile with none.
//! * A contact's `contactStatus` follows the clock across the contact's OWN timestamps:
//!   `SCHEDULED` while its pre-pass period is ahead, `PREPASS` from `prePassStartTime`,
//!   `PASS` from `startTime`, `POSTPASS` from `endTime` and `COMPLETED` from
//!   `postPassEndTime`. The table spans every stage, each stage boundary just after it is
//!   crossed, and a contact whose profile widens its window by nothing (so its pre-pass
//!   and post-pass periods have no duration at all). `ListContacts` filtering on the same
//!   statuses sees what `DescribeContact` reports, and a second read reports the same
//!   stage as the first.
//!
//! Every HTTP request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    now_timestamp, service_state, shift_seconds, CONTACT_COMPLETED, CONTACT_PASS,
    CONTACT_POSTPASS, CONTACT_PREPASS, CONTACT_SCHEDULED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_groups::{create_group, details, endpoint_kinds};
use crate::scheduling_loop::{
    create_config, create_tracking_config, harness_with_engine, send, spectrum_config,
    AFTER_SETTLE, RECORDING_BUCKET, REGION,
};

/// The endpoint of this account the `dataflowEndpointConfig` below names.
const ENDPOINT_NAME: &str = "downlink-endpoint";

/// The region that endpoint is placed in — deliberately NOT the region the contact is
/// reserved in, so the reported `dataflowDestinationRegion` can only come from the
/// config's own `dataflowEndpointRegion`.
const ENDPOINT_REGION: &str = "us-west-2";

/// The output node an `antenna-downlink-demod-decode` edge is wired to: the trailing
/// segment the modelled `ConfigArn` pattern allows after a config's uuid
/// (`^arn:aws:groundstation:…:config/…/<uuid>(/.{1,256})?$` — the pattern on
/// `GetConfig`'s `configArn`).
const OUTPUT_NODE: &str = "I-Egress";

/// The key prefix the `s3RecordingConfig` below records under.
const RECORDING_PREFIX: &str = "data/{satellite_id}/{year}/{month}/{day}";

/// One dataflow edge of the mission profile under test: the two `ConfigArn`s it wires
/// together, and the `source`/`destination` a contact reserved against that profile must
/// report for it.
struct Edge {
    case: &'static str,
    source_arn: String,
    destination_arn: String,
    expected_source: Value,
    expected_destination: Value,
}

/// `CreateMissionProfile` with `edges`, answering the profile's ARN.
async fn create_profile_with_edges(
    router: &MultiServiceRouter,
    name: &str,
    pre_pass: Option<i64>,
    post_pass: Option<i64>,
    edges: Vec<Value>,
) -> String {
    let tracking_arn = create_tracking_config(router, &format!("{name} tracking")).await;
    let mut body = json!({
        "name": name,
        "minimumViableContactDurationSeconds": 60,
        "dataflowEdges": edges,
        "trackingConfigArn": tracking_arn,
    });
    if let Some(pre) = pre_pass {
        body["contactPrePassDurationSeconds"] = json!(pre);
    }
    if let Some(post) = post_pass {
        body["contactPostPassDurationSeconds"] = json!(post);
    }
    let created = send(router, "POST", "/missionprofile", Some(body)).await;
    assert_eq!(
        created.status, 200,
        "CreateMissionProfile {name}: expected 200, got {} {}",
        created.status, created.body
    );
    let id = created.string("missionProfileId");
    let got = send(router, "GET", &format!("/missionprofile/{id}"), None).await;
    assert_eq!(
        got.status, 200,
        "GetMissionProfile {name}: expected 200, got {} {}",
        got.status, got.body
    );
    got.string("missionProfileArn")
}

/// `ReserveContact` for a window, asserting it was accepted, and answering the
/// `contactId`.
async fn reserve(
    router: &MultiServiceRouter,
    profile_arn: &str,
    case: &str,
    station: &str,
    window: (i64, i64),
) -> String {
    let reply = send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": station,
            "startTime": window.0,
            "endTime": window.1,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: ReserveContact expected 200, got {} {}",
        reply.status, reply.body
    );
    reply.string("contactId")
}

/// `DescribeContact`, asserting it answered.
async fn describe(router: &MultiServiceRouter, contact_id: &str, case: &str) -> Value {
    let reply = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeContact expected 200, got {} {}",
        reply.status, reply.body
    );
    reply.body
}

#[tokio::test]
async fn describe_contact_derives_one_dataflow_detail_per_mission_profile_edge() {
    let (router, _engine) = harness_with_engine();

    // The dataflow endpoint of this account the `dataflowEndpointConfig` names, in a group
    // of its own — a DATAFLOW endpoint, so the group carries the security details Ground
    // Station connects its stream with.
    let endpoint_body = endpoint_kinds()
        .into_iter()
        .find(|kind| kind.member == "endpoint")
        .map(|kind| {
            let mut body = kind.body.clone();
            body["name"] = json!(ENDPOINT_NAME);
            body
        })
        .expect("the dataflow-endpoint row of the endpoint-kind table");
    let group = create_group(
        &router,
        json!({"endpointDetails": [details("endpoint", &endpoint_body, true)]}),
    )
    .await;
    assert_eq!(
        group.status, 200,
        "CreateDataflowEndpointGroup expected 200, got {} {}",
        group.status, group.body
    );
    let group_id = group.string("dataflowEndpointGroupId");

    let (downlink_id, _, downlink_arn) = create_config(
        &router,
        "downlink",
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let (demod_id, _, demod_arn) = create_config(
        &router,
        "demod-decode",
        json!({"antennaDownlinkDemodDecodeConfig": {
            "spectrumConfig": spectrum_config(7820.5, 40.5),
            "demodulationConfig": {"unvalidatedJSON": "{}"},
            "decodeConfig": {"unvalidatedJSON": "{}"},
        }}),
        None,
    )
    .await;
    let (endpoint_config_id, _, endpoint_config_arn) = create_config(
        &router,
        "endpoint",
        json!({"dataflowEndpointConfig": {
            "dataflowEndpointName": ENDPOINT_NAME,
            "dataflowEndpointRegion": ENDPOINT_REGION,
        }}),
        None,
    )
    .await;
    let (recording_id, _, recording_arn) = create_config(
        &router,
        "recording",
        json!({"s3RecordingConfig": {
            "bucketArn": format!("arn:aws:s3:::{RECORDING_BUCKET}"),
            "roleArn": format!(
                "arn:aws:iam::{}:role/GroundStationS3Role",
                engine::emulator_account_id()
            ),
            "prefix": RECORDING_PREFIX,
        }}),
        None,
    )
    .await;

    // The dataflow endpoint's details as the group itself reports them — what the contact
    // must report for the end of a dataflow wired to that endpoint. Read first, so both
    // reads are taken after the endpoint's `creating` -> `created` move is due.
    std::thread::sleep(AFTER_SETTLE);
    let group_read = send(
        &router,
        "GET",
        &format!("/dataflowEndpointGroup/{group_id}"),
        None,
    )
    .await;
    assert_eq!(
        group_read.status, 200,
        "GetDataflowEndpointGroup expected 200, got {} {}",
        group_read.status, group_read.body
    );
    let stored_endpoint = group_read
        .field("endpointsDetails")
        .as_array()
        .and_then(|items| items.first())
        .expect("the group's one endpoint")
        .clone();

    let endpoint_destination = json!({
        "configType": "dataflow-endpoint",
        "configId": endpoint_config_id,
        "configDetails": {"endpointDetails": stored_endpoint},
        "dataflowDestinationRegion": ENDPOINT_REGION,
    });

    // Rows differ along the config type at each end, whether the end names one OUTPUT NODE
    // of a config or the config whole, whether the `ConfigDetails` union has a member for
    // that type at all, and whether the end sits in the contact's region or in one of its
    // own.
    let edges = [
        Edge {
            case: "an antenna-downlink into a dataflow endpoint in another region",
            source_arn: downlink_arn.clone(),
            destination_arn: endpoint_config_arn.clone(),
            // `ConfigDetails` has no member for an antenna-downlink config, so there are
            // no additional details to report for that end.
            expected_source: json!({
                "configType": "antenna-downlink",
                "configId": downlink_id,
                "dataflowSourceRegion": REGION,
            }),
            expected_destination: endpoint_destination.clone(),
        },
        Edge {
            case: "one output node of a demod-decode config into an S3 recording",
            source_arn: format!("{demod_arn}/{OUTPUT_NODE}"),
            destination_arn: recording_arn.clone(),
            expected_source: json!({
                "configType": "antenna-downlink-demod-decode",
                "configId": demod_id,
                "configDetails": {"antennaDemodDecodeDetails": {"outputNode": OUTPUT_NODE}},
                "dataflowSourceRegion": REGION,
            }),
            expected_destination: json!({
                "configType": "s3-recording",
                "configId": recording_id,
                "configDetails": {"s3RecordingDetails": {
                    "bucketArn": format!("arn:aws:s3:::{RECORDING_BUCKET}"),
                    "keyTemplate": RECORDING_PREFIX,
                }},
                "dataflowDestinationRegion": REGION,
            }),
        },
        Edge {
            case: "the same demod-decode config named whole into the dataflow endpoint",
            source_arn: demod_arn.clone(),
            destination_arn: endpoint_config_arn.clone(),
            // The edge named no node of the config, so there is no output node to report.
            expected_source: json!({
                "configType": "antenna-downlink-demod-decode",
                "configId": demod_id,
                "dataflowSourceRegion": REGION,
            }),
            expected_destination: endpoint_destination.clone(),
        },
    ];

    let profile_arn = create_profile_with_edges(
        &router,
        "dataflows",
        Some(120),
        Some(180),
        edges
            .iter()
            .map(|edge| json!([edge.source_arn, edge.destination_arn]))
            .collect(),
    )
    .await;

    let start = 1_900_000_000_i64;
    let contact_id = reserve(
        &router,
        &profile_arn,
        "dataflows",
        "Ohio-1",
        (start, start + 600),
    )
    .await;
    let described = describe(&router, &contact_id, "dataflows").await;
    let reported = described
        .get("dataflowList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case dataflows: dataflowList must be a list, got {described}"))
        .clone();

    assert_eq!(
        reported.len(),
        edges.len(),
        "case dataflows: expected one DataflowDetail per dataflow edge ({}), got {reported:?}",
        edges.len()
    );
    for (position, edge) in edges.iter().enumerate() {
        let detail = &reported[position];
        assert_eq!(
            detail.get("source").unwrap_or(&Value::Null),
            &edge.expected_source,
            "case {}: the source of the edge at position {position} must be the config it \
             named there, got {detail:?}",
            edge.case
        );
        assert_eq!(
            detail.get("destination").unwrap_or(&Value::Null),
            &edge.expected_destination,
            "case {}: the destination of the edge at position {position} must be the config \
             it named there, got {detail:?}",
            edge.case
        );
        assert!(
            detail.get("errorMessage").is_none(),
            "case {}: no dataflow has failed, so the detail carries no errorMessage, got \
             {detail:?}",
            edge.case
        );
    }

    // A profile that wires up NO dataflow is a contact with an empty dataflow list — the
    // member is answered, with nothing in it, rather than left out.
    let bare_profile = create_profile_with_edges(&router, "no dataflows", None, None, vec![]).await;
    let bare_contact = reserve(
        &router,
        &bare_profile,
        "no dataflows",
        "Ohio-2",
        (start, start + 600),
    )
    .await;
    let bare = describe(&router, &bare_contact, "no dataflows").await;
    assert_eq!(
        bare.get("dataflowList"),
        Some(&json!([])),
        "case no dataflows: a profile with no dataflow edge gives the contact an empty \
         dataflowList, got {bare}"
    );
}

/// One row of the pass-lifecycle table: where the clock sits inside the contact's own
/// window, as offsets in seconds from now for its four timestamps, and the
/// `contactStatus` that window then puts the contact in.
struct Stage {
    case: &'static str,
    /// `prePassStartTime`, `startTime`, `endTime`, `postPassEndTime`, each relative to the
    /// instant the row is placed.
    offsets: (i64, i64, i64, i64),
    expected: &'static str,
}

#[tokio::test]
async fn contact_status_follows_the_clock_through_the_contacts_own_pass_window() {
    // Rows differ along which stage of its window the clock has reached, including each
    // boundary just after it is crossed, and along whether the window is widened by a
    // pre-pass and post-pass period at all (the last two rows share their boundaries, so
    // those stages have no duration and are never reported).
    let rows = [
        Stage {
            case: "the whole window still ahead",
            offsets: (300, 420, 1020, 1200),
            expected: CONTACT_SCHEDULED,
        },
        Stage {
            case: "shortly before the pre-pass period",
            offsets: (30, 150, 750, 930),
            expected: CONTACT_SCHEDULED,
        },
        Stage {
            case: "inside the pre-pass period",
            offsets: (-30, 90, 690, 870),
            expected: CONTACT_PREPASS,
        },
        Stage {
            case: "just past the start of the pre-pass period",
            offsets: (-1, 119, 719, 899),
            expected: CONTACT_PREPASS,
        },
        Stage {
            case: "inside the pass",
            offsets: (-150, -30, 570, 750),
            expected: CONTACT_PASS,
        },
        Stage {
            case: "just past the start of the pass",
            offsets: (-121, -1, 599, 779),
            expected: CONTACT_PASS,
        },
        Stage {
            case: "inside the post-pass period",
            offsets: (-750, -630, -30, 150),
            expected: CONTACT_POSTPASS,
        },
        Stage {
            case: "just past the end of the pass",
            offsets: (-721, -601, -1, 179),
            expected: CONTACT_POSTPASS,
        },
        Stage {
            case: "the whole window elapsed",
            offsets: (-1200, -1080, -480, -300),
            expected: CONTACT_COMPLETED,
        },
        Stage {
            case: "just past the end of the post-pass period",
            offsets: (-900, -780, -180, -1),
            expected: CONTACT_COMPLETED,
        },
        Stage {
            case: "a window with no widening, under way",
            offsets: (-30, -30, 570, 570),
            expected: CONTACT_PASS,
        },
        Stage {
            case: "a window with no widening, elapsed",
            offsets: (-600, -600, -30, -30),
            expected: CONTACT_COMPLETED,
        },
    ];

    let (router, engine) = harness_with_engine();
    let profile_arn =
        create_profile_with_edges(&router, "pass stages", Some(120), Some(180), vec![]).await;

    // Antenna time that has already elapsed cannot be RESERVED, so each row reserves a
    // window a month out — one of its own, since an antenna serves one pass at a time —
    // and the stored contact's window is then placed where the row wants it relative to
    // the clock. That is what a contact whose pass has begun, or run, looks like in the
    // store; no request drives a pass, and nothing else here reads the store.
    let booked = now_timestamp().as_secs_f64() as i64 + 30 * 86_400;
    let mut contacts: Vec<(&Stage, String)> = Vec::new();
    for (slot, row) in rows.iter().enumerate() {
        let start = booked + i64::try_from(slot).expect("a row index fits in i64") * 86_400;
        let contact_id = reserve(
            &router,
            &profile_arn,
            row.case,
            "Ohio-1",
            (start, start + 600),
        )
        .await;
        {
            let state = service_state(&*engine).expect("this service's state store");
            let mut guard = state.lock();
            let contact = guard
                .contacts
                .get_mut(&contact_id)
                .expect("the reserved contact");
            let now = now_timestamp();
            contact.pre_pass_start_time = shift_seconds(&now, row.offsets.0);
            contact.start_time = shift_seconds(&now, row.offsets.1);
            contact.end_time = shift_seconds(&now, row.offsets.2);
            contact.post_pass_end_time = shift_seconds(&now, row.offsets.3);
        }
        contacts.push((row, contact_id));
    }

    // Past the span the scheduler owes a reservation its `SCHEDULING` -> `SCHEDULED` move,
    // so what every row then reports is the stage of its own window and nothing else.
    std::thread::sleep(AFTER_SETTLE);

    for (row, contact_id) in &contacts {
        let described = describe(&router, contact_id, row.case).await;
        let status = described
            .get("contactStatus")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            status, row.expected,
            "case {}: expected contactStatus {}, got {described}",
            row.case, row.expected
        );

        // A read reports a stage; it does not carry the contact to one. Reading again
        // answers the same stage, so nothing a later caller sees was changed by asking.
        let again = describe(&router, contact_id, row.case).await;
        assert_eq!(
            again.get("contactStatus").and_then(Value::as_str),
            Some(row.expected),
            "case {}: a second read must report the same stage, got {again}",
            row.case
        );
    }

    // And the listing's status filter sees exactly what the description reports: each row's
    // contact is listed under the status it reached, over a window spanning every row.
    let now_seconds = now_timestamp().as_secs_f64() as i64;
    let spanning_window = (now_seconds - 86_400, now_seconds + 86_400);
    let expected_statuses: BTreeSet<&str> = rows.iter().map(|row| row.expected).collect();
    for status in expected_statuses {
        let listed = listed_under(&router, status, spanning_window).await;
        for (row, contact_id) in &contacts {
            let listed_here = listed.contains(contact_id);
            assert_eq!(
                listed_here,
                row.expected == status,
                "case {}: a contact in {} must be listed under {status} only when that is \
                 its status, got listed={listed_here}",
                row.case,
                row.expected
            );
        }
    }
}

/// Every `contactId` `ListContacts` reports in `status` over `window`.
async fn listed_under(
    router: &MultiServiceRouter,
    status: &str,
    window: (i64, i64),
) -> BTreeSet<String> {
    let reply = send(
        router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": [status],
            "startTime": window.0,
            "endTime": window.1,
            "maxResults": 100,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {status}: ListContacts expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("contactList")
        .as_array()
        .unwrap_or_else(|| panic!("case {status}: contactList must be a list, got {}", reply.body))
        .iter()
        .filter_map(|item| item.get("contactId").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

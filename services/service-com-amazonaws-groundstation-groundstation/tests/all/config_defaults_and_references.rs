// SPDX-License-Identifier: CC0-1.0

//! Four rules about a `Config`'s stored shape and the references pointing at it, each
//! asserted end to end over HTTP through [`crate::scheduling_loop`]'s front door and over
//! a TABLE of inputs rather than one recorded call:
//!
//! * a member the model gives a `default` is materialised by the server: a `configData`
//!   that omits `SpectrumConfig$polarization` is stored, and read back, carrying the
//!   modelled `NONE`, while an explicit value persists unchanged — and the spectrum shape
//!   whose `polarization` carries NO default keeps the member absent;
//! * a resource another live resource references is not deleted: while any mission
//!   profile names a `Config` through any of its `ConfigArn`-valued members,
//!   `DeleteConfig` is refused with `InvalidParameterException` (HTTP 431) naming the
//!   referencing profiles, and both resources survive;
//! * a reference member admitting ONE capability type tells the two ways it can fail
//!   apart: an ARN resolving to no config is a missing resource (434), one resolving to a
//!   config of another type is a bad parameter (431);
//! * a mutation path composes its OWN not-found text from the request's identifiers plus
//!   the caller's account, which is not the text the matching read path answers.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    config_in_use_message, config_mutation_not_found_message, config_not_found_message,
    mission_profile_mutation_not_found_message, typed_reference_absent_message,
    typed_reference_wrong_type_message, MISSION_PROFILE_NOT_FOUND,
    SPECTRUM_CONFIG_POLARIZATION_DEFAULT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{
    antenna_uplink_config_data, create_config, create_tracking_config, harness, send, Reply,
    ABSENT_UUID, REGION,
};

/// Every value of the model's `Polarization` enum.
///
/// Source: `smithy_query(query="shape", shape="Polarization",
/// model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/\
/// com-amazonaws-groundstation-groundstation.json")` — `RIGHT_HAND`, `LEFT_HAND`, `NONE`.
/// All three are exercised, including `NONE` explicitly, so an assertion about the
/// default cannot be passing because the member was never set.
const POLARIZATIONS: &[&str] = &["RIGHT_HAND", "LEFT_HAND", "NONE"];

/// A downlink center frequency inside the documented 7750–8400 MHz band, and a bandwidth
/// inside both documented bandwidth ranges, so a row varying `polarization` alone is
/// accepted.
const DOWNLINK_MHZ: f64 = 7812.5;
const BANDWIDTH_MHZ: f64 = 20.0;

/// `GET /config/{configType}/{configId}`.
async fn get_config(router: &MultiServiceRouter, config_type: &str, config_id: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await
}

/// `PUT /config/{configType}/{configId}` — `UpdateConfig`.
async fn update_config(
    router: &MultiServiceRouter,
    config_type: &str,
    config_id: &str,
    name: &str,
    config_data: &Value,
) -> Reply {
    send(
        router,
        "PUT",
        &format!("/config/{config_type}/{config_id}"),
        Some(json!({"name": name, "configData": config_data})),
    )
    .await
}

/// A `SpectrumConfig` body, with `polarization` set only when `polarization` is `Some` —
/// an omitted member being exactly what makes the modelled default observable.
fn spectrum(polarization: Option<&str>) -> Value {
    let mut spectrum = json!({
        "centerFrequency": {"value": DOWNLINK_MHZ, "units": "MHz"},
        "bandwidth": {"value": BANDWIDTH_MHZ, "units": "MHz"},
    });
    if let Some(polarization) = polarization {
        spectrum["polarization"] = json!(polarization);
    }
    spectrum
}

/// The two `ConfigTypeData` members that carry a `SpectrumConfig` — the shape whose
/// `polarization` has the modelled default — each as `(configType, configData)`.
///
/// Source: `smithy_query(query="closure", operation="CreateConfig")`:
/// `AntennaDownlinkConfig$spectrumConfig` and
/// `AntennaDownlinkDemodDecodeConfig$spectrumConfig` both target `SpectrumConfig`, and no
/// other member of the closure does.
fn spectrum_carriers(polarization: Option<&str>) -> Vec<(&'static str, Value)> {
    vec![
        (
            "antenna-downlink",
            json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum(polarization)}}),
        ),
        (
            "antenna-downlink-demod-decode",
            json!({"antennaDownlinkDemodDecodeConfig": {
                "spectrumConfig": spectrum(polarization),
                "demodulationConfig": {"unvalidatedJSON": "{}"},
                "decodeConfig": {"unvalidatedJSON": "{}"},
            }}),
        ),
    ]
}

/// The `polarization` a stored config reports, from the `spectrumConfig` of whichever
/// union member the config is of.
fn reported_polarization(reply: &Reply, member: &str) -> Option<String> {
    reply
        .field("configData")
        .get(member)
        .and_then(|data| data.get("spectrumConfig"))
        .and_then(|spectrum| spectrum.get("polarization"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The union member name a `configData` carries.
fn union_member(config_data: &Value) -> String {
    config_data
        .as_object()
        .expect("a configData is a JSON object")
        .keys()
        .next()
        .expect("a configData sets exactly one union member")
        .clone()
}

#[tokio::test]
async fn the_modelled_polarization_default_is_materialised_and_an_explicit_one_persists() {
    let router = harness();

    // The value the model declares, pinned as the literal the model carries
    // (`smithy_query(query="shape", shape="SpectrumConfig")`: `default: "NONE"`), so the
    // rows below cannot pass on a constant that drifted away from it.
    assert_eq!(
        SPECTRUM_CONFIG_POLARIZATION_DEFAULT, "NONE",
        "the modelled default for SpectrumConfig$polarization is NONE, got {SPECTRUM_CONFIG_POLARIZATION_DEFAULT:?}"
    );

    // Rows: the member omitted, then each of the enum's three values set explicitly,
    // crossed with both shapes that carry a `SpectrumConfig`. The omitted row must read
    // back as the modelled default; an explicit row must read back as itself.
    let mut rows: Vec<(String, Option<&str>)> = vec![("omitted".to_string(), None)];
    rows.extend(
        POLARIZATIONS
            .iter()
            .map(|polarization| ((*polarization).to_string(), Some(*polarization))),
    );

    for (label, sent) in &rows {
        for (config_type, config_data) in spectrum_carriers(*sent) {
            let case = format!("{config_type} {label}");
            let member = union_member(&config_data);
            let expected = sent.unwrap_or(SPECTRUM_CONFIG_POLARIZATION_DEFAULT);

            let (config_id, created_type, _) =
                create_config(&router, &format!("polarization {case}"), config_data, None).await;
            assert_eq!(
                created_type, config_type,
                "case {case}: expected configType {config_type}, got {created_type}"
            );

            let read = get_config(&router, &created_type, &config_id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected GetConfig 200, got {} {}",
                read.status, read.body
            );
            let actual = reported_polarization(&read, &member);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case}: expected polarization {expected:?}, got {actual:?} in {}",
                read.field("configData")
            );

            // The same rule holds of the parameters an UPDATE stores: a member the update
            // omits is materialised exactly as a create's is, so the config never ends up
            // with the member missing.
            let (_, updated_data) = spectrum_carriers(None)
                .into_iter()
                .find(|(candidate, _)| *candidate == config_type)
                .expect("the row's own config type");
            let updated = update_config(
                &router,
                &created_type,
                &config_id,
                &format!("polarization {case} updated"),
                &updated_data,
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case}: expected UpdateConfig 200, got {} {}",
                updated.status, updated.body
            );
            let after = get_config(&router, &created_type, &config_id).await;
            let after_polarization = reported_polarization(&after, &member);
            assert_eq!(
                after_polarization.as_deref(),
                Some(SPECTRUM_CONFIG_POLARIZATION_DEFAULT),
                "case {case}: an update that omits the member stores the modelled \
                 default, got {after_polarization:?} in {}",
                after.field("configData")
            );
        }
    }
}

#[tokio::test]
async fn the_uplink_spectrum_polarization_has_no_default_and_stays_absent() {
    let router = harness();

    // `UplinkSpectrumConfig$polarization` carries NO `default` (smithy_query: shape
    // UplinkSpectrumConfig), so nothing is materialised for it: the member the request
    // omitted is absent from the stored config, and one it set persists.
    let mut omitted = antenna_uplink_config_data();
    omitted["spectrumConfig"]
        .as_object_mut()
        .expect("the uplink spectrumConfig is an object")
        .remove("polarization");

    let (config_id, config_type, _) = create_config(
        &router,
        "uplink polarization omitted",
        json!({"antennaUplinkConfig": omitted}),
        None,
    )
    .await;
    let read = get_config(&router, &config_type, &config_id).await;
    let reported = reported_polarization(&read, "antennaUplinkConfig");
    assert_eq!(
        reported, None,
        "case omitted: an uplink spectrum's polarization has no modelled default, so it \
         must stay absent, got {reported:?} in {}",
        read.field("configData")
    );

    for polarization in POLARIZATIONS {
        let mut uplink = antenna_uplink_config_data();
        uplink["spectrumConfig"]["polarization"] = json!(polarization);
        let (config_id, config_type, _) = create_config(
            &router,
            &format!("uplink polarization {polarization}"),
            json!({"antennaUplinkConfig": uplink}),
            None,
        )
        .await;
        let read = get_config(&router, &config_type, &config_id).await;
        let reported = reported_polarization(&read, "antennaUplinkConfig");
        assert_eq!(
            reported.as_deref(),
            Some(*polarization),
            "case {polarization}: an explicit uplink polarization persists, got \
             {reported:?} in {}",
            read.field("configData")
        );
    }
}

/// A telemetry-sink `configData`, the second reference member a mission profile carries.
fn telemetry_sink_config_data() -> Value {
    json!({"telemetrySinkConfig": {
        "telemetrySinkType": "KINESIS_DATA_STREAM",
        "telemetrySinkData": {"kinesisDataStreamData": {
            "kinesisRoleArn": "arn:aws:iam::000000000000:role/GroundStationTelemetryRole",
            "kinesisDataStreamArn":
                format!("arn:aws:kinesis:{REGION}:000000000000:stream/telemetry"),
        }},
    }})
}

/// `DELETE /config/{configType}/{configId}`.
async fn delete_config(router: &MultiServiceRouter, config_type: &str, config_id: &str) -> Reply {
    send(
        router,
        "DELETE",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await
}

/// The `configId` segment of a `ConfigArn`.
fn config_id_of(config_arn: &str) -> String {
    config_arn
        .rsplit('/')
        .next()
        .expect("a ConfigArn ends in its configId")
        .to_string()
}

#[tokio::test]
async fn a_config_a_mission_profile_references_cannot_be_deleted() {
    let router = harness();

    // One row per member of a mission profile that carries a `ConfigArn`
    // (smithy_query: shape CreateMissionProfileRequest) — the tracking config the
    // recording used, the telemetry sink, and each end of a dataflow edge. The rule is
    // about an inbound reference, not about which member holds it.
    struct Row {
        case: &'static str,
        /// The config whose deletion must be refused, as `(configType, configData)`.
        referenced: (&'static str, Value),
        /// Builds the profile request that references `referenced`'s ARN.
        profile: fn(&str, &str, &str) -> Value,
    }

    fn profile_via_tracking(referenced: &str, _endpoint: &str, _downlink: &str) -> Value {
        json!({
            "name": "profile via tracking",
            "minimumViableContactDurationSeconds": 30,
            "dataflowEdges": [],
            "trackingConfigArn": referenced,
        })
    }

    fn profile_via_telemetry(referenced: &str, tracking: &str, _downlink: &str) -> Value {
        json!({
            "name": "profile via telemetry sink",
            "minimumViableContactDurationSeconds": 30,
            "dataflowEdges": [],
            "trackingConfigArn": tracking,
            "telemetrySinkConfigArn": referenced,
        })
    }

    fn profile_via_edge_source(referenced: &str, tracking: &str, endpoint: &str) -> Value {
        json!({
            "name": "profile via edge source",
            "minimumViableContactDurationSeconds": 30,
            "dataflowEdges": [[referenced, endpoint]],
            "trackingConfigArn": tracking,
        })
    }

    fn profile_via_edge_destination(referenced: &str, tracking: &str, endpoint: &str) -> Value {
        json!({
            "name": "profile via edge destination",
            "minimumViableContactDurationSeconds": 30,
            "dataflowEdges": [[endpoint, referenced]],
            "trackingConfigArn": tracking,
        })
    }

    let rows = vec![
        Row {
            case: "trackingConfigArn",
            referenced: ("tracking", json!({"trackingConfig": {"autotrack": "REQUIRED"}})),
            profile: profile_via_tracking,
        },
        Row {
            case: "telemetrySinkConfigArn",
            referenced: ("telemetry-sink", telemetry_sink_config_data()),
            profile: profile_via_telemetry,
        },
        Row {
            case: "dataflowEdges-source",
            referenced: (
                "antenna-downlink",
                json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum(Some("RIGHT_HAND"))}}),
            ),
            profile: profile_via_edge_source,
        },
        Row {
            case: "dataflowEdges-destination",
            referenced: (
                "dataflow-endpoint",
                json!({"dataflowEndpointConfig": {"dataflowEndpointName": "downlink"}}),
            ),
            profile: profile_via_edge_destination,
        },
    ];

    for row in rows {
        let case = row.case;
        let (referenced_type, referenced_data) = row.referenced;
        let (referenced_id, _, referenced_arn) = create_config(
            &router,
            &format!("referenced {case}"),
            referenced_data,
            None,
        )
        .await;
        // The members a profile needs beside the referenced one: its own tracking config
        // (every profile has one) and a second config to put at the other end of an edge.
        let tracking_arn = create_tracking_config(&router, &format!("own tracking {case}")).await;
        let (_, _, endpoint_arn) = create_config(
            &router,
            &format!("edge endpoint {case}"),
            json!({"dataflowEndpointConfig": {"dataflowEndpointName": "edge"}}),
            None,
        )
        .await;

        let created = send(
            &router,
            "POST",
            "/missionprofile",
            Some((row.profile)(&referenced_arn, &tracking_arn, &endpoint_arn)),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateMissionProfile 200, got {} {}",
            created.status, created.body
        );
        let profile_id = created.string("missionProfileId");
        let profile_arn = send(
            &router,
            "GET",
            &format!("/missionprofile/{profile_id}"),
            None,
        )
        .await
        .string("missionProfileArn");

        let refused = delete_config(&router, referenced_type, &referenced_id).await;
        assert_eq!(
            refused.status, 431,
            "case {case}: deleting a referenced config must answer 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {case}: wrong error shape, got {}",
            refused.body
        );
        let message = refused.string("message");
        assert_eq!(
            message,
            config_in_use_message(std::slice::from_ref(&profile_arn)),
            "case {case}: the refusal names the referencing mission profile, got \
             {message:?}"
        );
        assert!(
            message.contains(&profile_arn),
            "case {case}: the refusal must name the profile's ARN, got {message:?}"
        );

        // Nothing moved: the config and the profile are both still there.
        let config_still_there = get_config(&router, referenced_type, &referenced_id).await;
        assert_eq!(
            config_still_there.status, 200,
            "case {case}: a refused delete must leave the config readable, got {} {}",
            config_still_there.status, config_still_there.body
        );
        let profile_still_there = send(
            &router,
            "GET",
            &format!("/missionprofile/{profile_id}"),
            None,
        )
        .await;
        assert_eq!(
            profile_still_there.status, 200,
            "case {case}: a refused delete must leave the profile readable, got {} {}",
            profile_still_there.status, profile_still_there.body
        );

        // And once the only profile holding the reference is gone, the delete goes
        // through — the refusal is about the live reference, not about the config.
        let profile_deleted = send(
            &router,
            "DELETE",
            &format!("/missionprofile/{profile_id}"),
            None,
        )
        .await;
        assert_eq!(
            profile_deleted.status, 200,
            "case {case}: expected DeleteMissionProfile 200, got {} {}",
            profile_deleted.status, profile_deleted.body
        );
        let deleted = delete_config(&router, referenced_type, &referenced_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: an unreferenced config deletes, got {} {}",
            deleted.status, deleted.body
        );
    }
}

#[tokio::test]
async fn the_refusal_names_every_mission_profile_holding_a_reference() {
    let router = harness();
    let (shared_id, _, shared_arn) = create_config(
        &router,
        "shared tracking",
        json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        None,
    )
    .await;

    // Three profiles on the one config, so the refusal is asserted to list ALL of them
    // rather than the first one found.
    let mut profile_arns: Vec<String> = Vec::new();
    for index in 0..3 {
        let created = send(
            &router,
            "POST",
            "/missionprofile",
            Some(json!({
                "name": format!("sharing profile {index}"),
                "minimumViableContactDurationSeconds": 30,
                "dataflowEdges": [],
                "trackingConfigArn": shared_arn,
            })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "profile {index}: expected 200, got {} {}",
            created.status, created.body
        );
        let id = created.string("missionProfileId");
        profile_arns.push(
            send(&router, "GET", &format!("/missionprofile/{id}"), None)
                .await
                .string("missionProfileArn"),
        );
    }
    profile_arns.sort();

    let refused = delete_config(&router, "tracking", &shared_id).await;
    let message = refused.string("message");
    assert_eq!(
        refused.status, 431,
        "expected 431 while three profiles reference the config, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        message,
        config_in_use_message(&profile_arns),
        "the refusal lists every referencing profile, got {message:?}"
    );
    for arn in &profile_arns {
        assert!(
            message.contains(arn),
            "the refusal must name {arn}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_echo_reference_tells_a_missing_config_from_one_of_the_wrong_type() {
    let router = harness();

    // The config an `uplinkEchoConfig` may echo, and one config of every OTHER capability
    // type to point the reference at instead. A config that EXISTS under another type is
    // a bad parameter, not a missing resource — the ARN resolved; what is wrong is the
    // type it names.
    let (_, _, uplink_arn) = create_config(
        &router,
        "echo source",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    let wrong_type_rows: Vec<(&str, Value)> = vec![
        (
            "antenna-downlink",
            json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum(Some("LEFT_HAND"))}}),
        ),
        ("tracking", json!({"trackingConfig": {"autotrack": "REQUIRED"}})),
        (
            "dataflow-endpoint",
            json!({"dataflowEndpointConfig": {"dataflowEndpointName": "downlink"}}),
        ),
        ("telemetry-sink", telemetry_sink_config_data()),
        (
            "uplink-echo",
            json!({"uplinkEchoConfig": {
                "enabled": false,
                "antennaUplinkConfigArn": uplink_arn,
            }}),
        ),
    ];

    for (config_type, config_data) in wrong_type_rows {
        let (_, _, wrong_arn) = create_config(
            &router,
            &format!("echo target {config_type}"),
            config_data,
            None,
        )
        .await;
        let refused = send(
            &router,
            "POST",
            "/config",
            Some(json!({
                "name": format!("echo of a {config_type} config"),
                "configData": {"uplinkEchoConfig": {
                    "enabled": true,
                    "antennaUplinkConfigArn": wrong_arn,
                }},
            })),
        )
        .await;
        assert_eq!(
            refused.status, 431,
            "case {config_type}: a reference to a config of another type must answer 431, \
             got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {config_type}: wrong error shape, got {}",
            refused.body
        );
        let message = refused.string("message");
        assert_eq!(
            message,
            typed_reference_wrong_type_message("antenna-uplink"),
            "case {config_type}: the refusal states the type the member requires, got \
             {message:?}"
        );
        assert!(
            !message.contains(&wrong_arn),
            "case {config_type}: the refusal is about the type, not about the resource, \
             got {message:?}"
        );
    }

    // The OTHER way the reference fails is unchanged: an ARN of the right type naming no
    // config is a missing resource, with this operation's not-found text.
    let absent_arn = format!(
        "arn:aws:groundstation:{REGION}:{}:config/antenna-uplink/{ABSENT_UUID}",
        engine::EMULATOR_ACCOUNT_ID
    );
    let missing = send(
        &router,
        "POST",
        "/config",
        Some(json!({
            "name": "echo of nothing",
            "configData": {"uplinkEchoConfig": {
                "enabled": true,
                "antennaUplinkConfigArn": absent_arn,
            }},
        })),
    )
    .await;
    assert_eq!(
        missing.status, 434,
        "case absent: a reference to no config must answer 434, got {} {}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.string("message"),
        typed_reference_absent_message("antenna-uplink", &absent_arn),
        "case absent: the not-found text names the capability and the ARN, got {}",
        missing.body
    );

    // And the update path is held to the same two answers as the create.
    let (echo_id, echo_type, _) = create_config(
        &router,
        "echo to retarget",
        json!({"uplinkEchoConfig": {
            "enabled": true,
            "antennaUplinkConfigArn": uplink_arn,
        }}),
        None,
    )
    .await;
    let tracking_arn = create_tracking_config(&router, "retarget tracking").await;
    let retyped = update_config(
        &router,
        &echo_type,
        &echo_id,
        "echo to retarget",
        &json!({"uplinkEchoConfig": {
            "enabled": true,
            "antennaUplinkConfigArn": tracking_arn,
        }}),
    )
    .await;
    assert_eq!(
        retyped.status, 431,
        "case update-wrong-type: expected 431, got {} {}",
        retyped.status, retyped.body
    );
    assert_eq!(
        retyped.string("message"),
        typed_reference_wrong_type_message("antenna-uplink"),
        "case update-wrong-type: wrong refusal text, got {}",
        retyped.body
    );
    let unchanged = get_config(&router, &echo_type, &echo_id).await;
    assert_eq!(
        unchanged.field("configData"),
        &json!({"uplinkEchoConfig": {
            "enabled": true,
            "antennaUplinkConfigArn": uplink_arn,
        }}),
        "case update-wrong-type: a refused update must change nothing, got {}",
        unchanged.field("configData")
    );
    let dangling = update_config(
        &router,
        &echo_type,
        &echo_id,
        "echo to retarget",
        &json!({"uplinkEchoConfig": {
            "enabled": true,
            "antennaUplinkConfigArn": absent_arn,
        }}),
    )
    .await;
    assert_eq!(
        dangling.status, 434,
        "case update-absent: expected 434, got {} {}",
        dangling.status, dangling.body
    );
}

#[tokio::test]
async fn an_update_of_a_dead_resource_answers_its_own_not_found_text() {
    let router = harness();
    let account = engine::EMULATOR_ACCOUNT_ID;

    // Configs of several types, each created, deleted and then both READ and UPDATED, so
    // the two texts are compared for the same dead id — plus an id that never existed, so
    // the text cannot depend on the resource having been there.
    let rows: Vec<(&str, Value)> = vec![
        ("tracking", json!({"trackingConfig": {"autotrack": "REQUIRED"}})),
        (
            "antenna-downlink",
            json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum(Some("NONE"))}}),
        ),
        (
            "dataflow-endpoint",
            json!({"dataflowEndpointConfig": {"dataflowEndpointName": "downlink"}}),
        ),
        ("telemetry-sink", telemetry_sink_config_data()),
    ];

    for (config_type, config_data) in rows {
        let (config_id, _, config_arn) = create_config(
            &router,
            &format!("doomed {config_type}"),
            config_data.clone(),
            None,
        )
        .await;
        assert_eq!(
            config_id_of(&config_arn),
            config_id,
            "case {config_type}: the ARN's last segment is the configId, got {config_arn}"
        );
        let deleted = delete_config(&router, config_type, &config_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {config_type}: expected DeleteConfig 200, got {} {}",
            deleted.status, deleted.body
        );

        let read = get_config(&router, config_type, &config_id).await;
        assert_eq!(
            read.status, 434,
            "case {config_type}: a read of the dead id answers 434, got {} {}",
            read.status, read.body
        );
        let read_message = read.string("message");
        assert_eq!(
            read_message,
            config_not_found_message(config_type, &config_id),
            "case {config_type}: the READ path names the pair it was asked for, got \
             {read_message:?}"
        );

        let updated = update_config(
            &router,
            config_type,
            &config_id,
            "resurrected",
            &config_data,
        )
        .await;
        assert_eq!(
            updated.status, 434,
            "case {config_type}: an update of the dead id answers 434, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_code(),
            "ResourceNotFoundException",
            "case {config_type}: wrong error shape, got {}",
            updated.body
        );
        let update_message = updated.string("message");
        assert_eq!(
            update_message,
            config_mutation_not_found_message(config_type, &config_id),
            "case {config_type}: the MUTATION path names the pair and the account, got \
             {update_message:?}"
        );
        assert!(
            update_message.contains(&config_id)
                && update_message.contains(config_type)
                && update_message.contains(account),
            "case {config_type}: the mutation text is composed from the request's \
             identifiers and the caller's account, got {update_message:?}"
        );
        assert_ne!(
            update_message, read_message,
            "case {config_type}: the read and the mutation paths answer different texts"
        );
    }

    // An id that was never minted, under a type the store holds nothing of.
    let never = update_config(
        &router,
        "s3-recording",
        ABSENT_UUID,
        "nothing to rename",
        &json!({"s3RecordingConfig": {
            "bucketArn": "arn:aws:s3:::ground-station-downlink",
            "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
        }}),
    )
    .await;
    assert_eq!(
        never.string("message"),
        config_mutation_not_found_message("s3-recording", ABSENT_UUID),
        "case never-created: same composition for an id that never existed, got {}",
        never.body
    );

    // A mission profile: the same split between the read path's literal and the mutation
    // path's composed text, over ids that were deleted and one that never existed.
    let tracking_arn = create_tracking_config(&router, "profile tracking").await;
    let created = send(
        &router,
        "POST",
        "/missionprofile",
        Some(json!({
            "name": "doomed profile",
            "minimumViableContactDurationSeconds": 30,
            "dataflowEdges": [],
            "trackingConfigArn": tracking_arn,
        })),
    )
    .await;
    let profile_id = created.string("missionProfileId");
    let profile_deleted = send(
        &router,
        "DELETE",
        &format!("/missionprofile/{profile_id}"),
        None,
    )
    .await;
    assert_eq!(
        profile_deleted.status, 200,
        "expected DeleteMissionProfile 200, got {} {}",
        profile_deleted.status, profile_deleted.body
    );

    for (case, id) in [
        ("deleted-profile", profile_id.as_str()),
        ("never-created-profile", ABSENT_UUID),
    ] {
        let read = send(&router, "GET", &format!("/missionprofile/{id}"), None).await;
        assert_eq!(
            read.status, 434,
            "case {case}: a read of the dead id answers 434, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("message"),
            MISSION_PROFILE_NOT_FOUND,
            "case {case}: the READ path answers the service's literal, got {}",
            read.body
        );

        let updated = send(
            &router,
            "PUT",
            &format!("/missionprofile/{id}"),
            Some(json!({"name": "resurrected"})),
        )
        .await;
        assert_eq!(
            updated.status, 434,
            "case {case}: an update of the dead id answers 434, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_code(),
            "ResourceNotFoundException",
            "case {case}: wrong error shape, got {}",
            updated.body
        );
        let message = updated.string("message");
        assert_eq!(
            message,
            mission_profile_mutation_not_found_message(id),
            "case {case}: the MUTATION path names the id and the account, got {message:?}"
        );
        assert!(
            message.contains(id) && message.contains(account),
            "case {case}: the mutation text is composed from the request's id and the \
             caller's account, got {message:?}"
        );
        assert_ne!(
            message, MISSION_PROFILE_NOT_FOUND,
            "case {case}: the mutation path does not reuse the read path's literal"
        );
    }
}

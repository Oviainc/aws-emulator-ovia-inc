// SPDX-License-Identifier: CC0-1.0

//! Ground Station LIFECYCLE WRITES, end to end over HTTP: what a delete, an update and
//! a read of a taggable resource do to this service's state.
//!
//! Every request goes through the same front door the emulator binary uses — the
//! harness, the signed `restJson1` requests and the response reader are
//! [`crate::scheduling_loop`]'s, reused rather than rebuilt — and each suite asserts a
//! RULE over a table of inputs, not one recorded call:
//!
//! * a successful delete REMOVES the resource, so every later read of it, every repeat
//!   of the delete and every ARN-valued reference to it answers
//!   `ResourceNotFoundException` (HTTP 434) — asserted for all eight config types and
//!   for a mission profile;
//! * an update REPLACES the members it carries and leaves every member it omits alone,
//!   and a refused update changes nothing;
//! * a read of a taggable resource serializes `tags` as a map, `{}` when the resource
//!   carries no tag.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{
    antenna_uplink_config_data, config_data_table, create_config, create_tracking_config, harness,
    send, spectrum_config, Reply, ABSENT_CONFIG_ARN, ABSENT_MISSION_PROFILE_ARN, ABSENT_UUID,
    REGION,
};

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

/// `GET /missionprofile/{missionProfileId}`.
async fn get_mission_profile(router: &MultiServiceRouter, id: &str) -> Reply {
    send(router, "GET", &format!("/missionprofile/{id}"), None).await
}

/// `CreateMissionProfile` from `body`, asserting it succeeded, and answering the id it
/// minted.
async fn create_mission_profile(router: &MultiServiceRouter, body: Value) -> String {
    let created = send(router, "POST", "/missionprofile", Some(body.clone())).await;
    assert_eq!(
        created.status, 200,
        "CreateMissionProfile {body}: expected 200, got {} {}",
        created.status, created.body
    );
    created.string("missionProfileId")
}

/// The members a `CreateMissionProfile` request carries, as a JSON object a row can
/// extend. `tracking_config_arn` is the one reference every profile needs.
fn base_profile_request(name: &str, tracking_config_arn: &str) -> Map<String, Value> {
    json!({
        "name": name,
        "minimumViableContactDurationSeconds": 30,
        "contactPrePassDurationSeconds": 120,
        "contactPostPassDurationSeconds": 60,
        "dataflowEdges": [],
        "trackingConfigArn": tracking_config_arn,
        "tags": {"stage": "created"},
    })
    .as_object()
    .expect("a JSON object")
    .clone()
}

#[tokio::test]
async fn a_deleted_config_is_gone_from_every_read_and_every_reference() {
    let router = harness();

    // `uplinkEchoConfig` references an antenna-uplink config, so that one exists first.
    let (_, _, antenna_uplink_arn) = create_config(
        &router,
        "echo source uplink",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    // The table spans every `ConfigTypeData` member (source:
    // `crate::scheduling_loop::config_data_table`, built from
    // `smithy_query(query="shape", shape="ConfigTypeData")`), so the removal is asserted
    // for the whole config store rather than for the one type the recording used.
    for (config_type, config_data) in config_data_table(&antenna_uplink_arn) {
        let (config_id, _, config_arn) = create_config(
            &router,
            &format!("doomed {config_type}"),
            config_data.clone(),
            Some(json!({"stage": "created"})),
        )
        .await;

        let path = format!("/config/{config_type}/{config_id}");
        let deleted = send(&router, "DELETE", &path, None).await;
        assert_eq!(
            deleted.status, 200,
            "case {config_type}: expected DeleteConfig 200, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.string("configId"),
            config_id,
            "case {config_type}: DeleteConfig must answer the config it removed, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.field("configType"),
            &json!(config_type),
            "case {config_type}: DeleteConfig must answer the removed config's type, got {}",
            deleted.field("configType")
        );
        assert_eq!(
            deleted.string("configArn"),
            config_arn,
            "case {config_type}: DeleteConfig must answer the removed config's ARN, got {}",
            deleted.body
        );

        let read = get_config(&router, config_type, &config_id).await;
        assert_eq!(
            read.status, 434,
            "case {config_type}: a deleted config must read 434, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "ResourceNotFoundException",
            "case {config_type}: wrong error shape for a deleted config, got {}",
            read.body
        );

        // The delete has nothing left to remove the second time, and says so rather
        // than reporting a config it no longer holds.
        let again = send(&router, "DELETE", &path, None).await;
        assert_eq!(
            again.status, 434,
            "case {config_type}: deleting a config twice must answer 434, got {} {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ResourceNotFoundException",
            "case {config_type}: wrong error shape on the repeated delete, got {}",
            again.body
        );

        // Updating what is no longer there is refused the same way — with the very
        // `configData` the config was created with, so the only thing wrong with the
        // request is that its config is gone.
        let updated = send(
            &router,
            "PUT",
            &path,
            Some(json!({"name": "resurrected", "configData": config_data})),
        )
        .await;
        assert_eq!(
            updated.status, 434,
            "case {config_type}: updating a deleted config must answer 434, got {} {}",
            updated.status, updated.body
        );
    }

    // And a deleted config's ARN resolves to nothing: a profile cannot be wired to it,
    // exactly as it cannot be wired to a config that never existed.
    let tracking_arn = create_tracking_config(&router, "doomed tracking").await;
    let tracking_id = tracking_arn
        .rsplit('/')
        .next()
        .expect("the configId segment of the ARN")
        .to_string();
    let deleted = send(
        &router,
        "DELETE",
        &format!("/config/tracking/{tracking_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case reference: expected DeleteConfig 200, got {} {}",
        deleted.status, deleted.body
    );
    let refused = send(
        &router,
        "POST",
        "/missionprofile",
        Some(Value::Object(base_profile_request(
            "profile on a deleted config",
            &tracking_arn,
        ))),
    )
    .await;
    assert_eq!(
        refused.status, 434,
        "case reference: a profile referencing a deleted config must be refused 434, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ResourceNotFoundException",
        "case reference: wrong error shape, got {}",
        refused.body
    );
}

#[tokio::test]
async fn a_deleted_mission_profile_is_gone_from_every_read_and_every_reference() {
    let router = harness();
    let tracking_arn = create_tracking_config(&router, "profile tracking").await;

    // Two profiles, so the listing is asserted to lose exactly the deleted one.
    let kept = create_mission_profile(
        &router,
        Value::Object(base_profile_request("kept profile", &tracking_arn)),
    )
    .await;
    let doomed = create_mission_profile(
        &router,
        Value::Object(base_profile_request("doomed profile", &tracking_arn)),
    )
    .await;
    let doomed_arn = get_mission_profile(&router, &doomed)
        .await
        .string("missionProfileArn");

    let path = format!("/missionprofile/{doomed}");
    let deleted = send(&router, "DELETE", &path, None).await;
    assert_eq!(
        deleted.status, 200,
        "case delete: expected DeleteMissionProfile 200, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.string("missionProfileId"),
        doomed,
        "case delete: DeleteMissionProfile must answer the profile it removed, got {}",
        deleted.body
    );

    let read = get_mission_profile(&router, &doomed).await;
    assert_eq!(
        read.status, 434,
        "case read: a deleted mission profile must read 434, got {} {}",
        read.status, read.body
    );
    assert_eq!(
        read.error_code(),
        "ResourceNotFoundException",
        "case read: wrong error shape for a deleted mission profile, got {}",
        read.body
    );

    let again = send(&router, "DELETE", &path, None).await;
    assert_eq!(
        again.status, 434,
        "case repeat: deleting a mission profile twice must answer 434, got {} {}",
        again.status, again.body
    );

    let updated = send(&router, "PUT", &path, Some(json!({"name": "resurrected"}))).await;
    assert_eq!(
        updated.status, 434,
        "case update: updating a deleted mission profile must answer 434, got {} {}",
        updated.status, updated.body
    );

    // The listing is the store, so the deleted profile is gone from it and the other
    // one is untouched.
    let listed = send(&router, "GET", "/missionprofile", None).await;
    assert_eq!(
        listed.status, 200,
        "case list: expected ListMissionProfiles 200, got {} {}",
        listed.status, listed.body
    );
    let ids: Vec<String> = listed
        .field("missionProfileList")
        .as_array()
        .expect("missionProfileList is a list")
        .iter()
        .map(|item| {
            item.get("missionProfileId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        ids,
        vec![kept.clone()],
        "case list: the listing must hold exactly the profile that was not deleted, got {ids:?}"
    );

    // A reservation against a deleted profile's ARN is refused as one against an ARN
    // that never named a profile is.
    for (case, arn) in [
        ("deleted-profile-arn", doomed_arn.as_str()),
        ("absent-profile-arn", ABSENT_MISSION_PROFILE_ARN),
    ] {
        let reserved = send(
            &router,
            "POST",
            "/contact",
            Some(json!({
                "missionProfileArn": arn,
                "groundStation": "Ohio-1",
                "startTime": 1_800_000_000_i64,
                "endTime": 1_800_000_600_i64,
            })),
        )
        .await;
        assert_eq!(
            reserved.status, 434,
            "case {case}: reserving against it must answer 434, got {} {}",
            reserved.status, reserved.body
        );
        assert_eq!(
            reserved.error_code(),
            "ResourceNotFoundException",
            "case {case}: wrong error shape, got {}",
            reserved.body
        );
    }
}

#[tokio::test]
async fn update_config_replaces_the_name_and_the_parameters_of_the_stored_config() {
    let router = harness();
    let (_, _, antenna_uplink_arn) = create_config(
        &router,
        "echo source uplink",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    // One row per shape of change an update can make to a config's parameters: an enum
    // value, a measured quantity, a string, a boolean and a nested reference — across
    // five of the eight `ConfigTypeData` members, the first row being the recorded case.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "tracking",
            json!({"trackingConfig": {"autotrack": "PREFERRED"}}),
            json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        ),
        (
            "antenna-downlink",
            json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
            json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(8100.5, 50.5)}}),
        ),
        (
            "dataflow-endpoint",
            json!({"dataflowEndpointConfig": {
                "dataflowEndpointName": "downlink-endpoint",
                "dataflowEndpointRegion": REGION,
            }}),
            json!({"dataflowEndpointConfig": {
                "dataflowEndpointName": "uplink-endpoint",
                "dataflowEndpointRegion": REGION,
            }}),
        ),
        (
            "s3-recording",
            json!({"s3RecordingConfig": {
                "bucketArn": "arn:aws:s3:::ground-station-downlink",
                "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
                "prefix": "data/{satellite_id}/{year}",
            }}),
            json!({"s3RecordingConfig": {
                "bucketArn": "arn:aws:s3:::ground-station-downlink",
                "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
                "prefix": "archive/{satellite_id}/{year}/{month}/{day}",
            }}),
        ),
        (
            "uplink-echo",
            json!({"uplinkEchoConfig": {
                "enabled": true,
                "antennaUplinkConfigArn": antenna_uplink_arn,
            }}),
            json!({"uplinkEchoConfig": {
                "enabled": false,
                "antennaUplinkConfigArn": antenna_uplink_arn,
            }}),
        ),
    ];

    for (config_type, created_data, updated_data) in rows {
        let name = format!("cfg {config_type}");
        let renamed = format!("cfg {config_type} renamed");
        let tags = json!({"stage": "created"});
        let (config_id, _, config_arn) = create_config(
            &router,
            &name,
            created_data.clone(),
            Some(tags.clone()),
        )
        .await;

        let path = format!("/config/{config_type}/{config_id}");
        let updated = send(
            &router,
            "PUT",
            &path,
            Some(json!({"name": renamed, "configData": updated_data})),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {config_type}: expected UpdateConfig 200, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("configId"),
            config_id,
            "case {config_type}: an update must not move the config's id, got {}",
            updated.body
        );
        assert_eq!(
            updated.string("configArn"),
            config_arn,
            "case {config_type}: an update must not move the config's ARN, got {}",
            updated.body
        );

        let read = get_config(&router, config_type, &config_id).await;
        assert_eq!(
            read.status, 200,
            "case {config_type}: expected GetConfig 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("name"),
            renamed,
            "case {config_type}: GetConfig must return the updated name, got {}",
            read.body
        );
        assert_eq!(
            read.field("configData"),
            &updated_data,
            "case {config_type}: GetConfig must return the updated configData, got {}",
            read.field("configData")
        );
        assert_eq!(
            read.field("tags"),
            &tags,
            "case {config_type}: an update carries no tags, so the config keeps its own, got {}",
            read.field("tags")
        );
        assert_eq!(
            read.field("configType"),
            &json!(config_type),
            "case {config_type}: an update must not retype the config, got {}",
            read.field("configType")
        );

        // Refusals: each one must leave the config exactly as the update above left it.
        let other_type_data = if config_type == "tracking" {
            json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}})
        } else {
            json!({"trackingConfig": {"autotrack": "REQUIRED"}})
        };
        let retyped = send(
            &router,
            "PUT",
            &path,
            Some(json!({"name": "retyped", "configData": other_type_data})),
        )
        .await;
        assert_eq!(
            retyped.status, 431,
            "case {config_type}: a configData of another type must be refused 431, got {} {}",
            retyped.status, retyped.body
        );
        assert_eq!(
            retyped.error_code(),
            "InvalidParameterException",
            "case {config_type}: wrong error shape for a retyping update, got {}",
            retyped.body
        );
        assert_eq!(
            retyped.field("parameterName").as_str(),
            Some("configData"),
            "case {config_type}: the refusal must name the member at fault, got {}",
            retyped.body
        );

        let after = get_config(&router, config_type, &config_id).await;
        assert_eq!(
            after.string("name"),
            renamed,
            "case {config_type}: a refused update must change nothing, got {}",
            after.body
        );
        assert_eq!(
            after.field("configData"),
            &updated_data,
            "case {config_type}: a refused update must change nothing, got {}",
            after.field("configData")
        );
    }

    // An update of a config that was never created names no config to update.
    let unknown = send(
        &router,
        "PUT",
        &format!("/config/tracking/{ABSENT_UUID}"),
        Some(json!({
            "name": "nothing to rename",
            "configData": {"trackingConfig": {"autotrack": "REQUIRED"}},
        })),
    )
    .await;
    assert_eq!(
        unknown.status, 434,
        "case unknown-config-id: expected 434, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "case unknown-config-id: wrong error shape, got {}",
        unknown.body
    );

    // An `uplinkEchoConfig` update is held to the create's reference rule: the config it
    // echoes must exist and be an antenna-uplink one.
    let (echo_id, _, _) = create_config(
        &router,
        "echo config",
        json!({"uplinkEchoConfig": {
            "enabled": true,
            "antennaUplinkConfigArn": antenna_uplink_arn,
        }}),
        None,
    )
    .await;
    let dangling = send(
        &router,
        "PUT",
        &format!("/config/uplink-echo/{echo_id}"),
        Some(json!({
            "name": "echo config",
            "configData": {"uplinkEchoConfig": {
                "enabled": true,
                "antennaUplinkConfigArn": ABSENT_CONFIG_ARN,
            }},
        })),
    )
    .await;
    assert_eq!(
        dangling.status, 434,
        "case dangling-echo-reference: expected 434, got {} {}",
        dangling.status, dangling.body
    );
}

#[tokio::test]
async fn update_mission_profile_replaces_every_member_it_carries_and_no_other() {
    let router = harness();
    let tracking_arn = create_tracking_config(&router, "profile tracking").await;
    let other_tracking_arn = create_tracking_config(&router, "profile tracking alternate").await;
    let (_, _, telemetry_arn) = create_config(
        &router,
        "profile telemetry sink",
        json!({"telemetrySinkConfig": {
            "telemetrySinkType": "KINESIS_DATA_STREAM",
            "telemetrySinkData": {"kinesisDataStreamData": {
                "kinesisRoleArn": "arn:aws:iam::000000000000:role/GroundStationTelemetryRole",
                "kinesisDataStreamArn": format!("arn:aws:kinesis:{REGION}:000000000000:stream/telemetry"),
            }},
        }}),
        None,
    )
    .await;
    let (_, _, downlink_arn) = create_config(
        &router,
        "profile downlink",
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7900.5, 20.5)}}),
        None,
    )
    .await;
    let (_, _, endpoint_arn) = create_config(
        &router,
        "profile endpoint",
        json!({"dataflowEndpointConfig": {
            "dataflowEndpointName": "downlink-endpoint",
            "dataflowEndpointRegion": REGION,
        }}),
        None,
    )
    .await;

    // One row per member the request can carry, plus the combinations that matter: the
    // members are `smithy_query(query="operation", operation="UpdateMissionProfile")`'s,
    // and the two duration rows sit on the published bounds of
    // `PositiveDurationInSeconds` (range 1..21600) and `DurationInSeconds` (0..21600).
    let rows: Vec<(&str, Value)> = vec![
        ("nothing-at-all", json!({})),
        ("name-only", json!({"name": "profile renamed"})),
        (
            "minimum-viable-only",
            json!({"minimumViableContactDurationSeconds": 45}),
        ),
        (
            "minimum-viable-at-lower-bound",
            json!({"minimumViableContactDurationSeconds": 1}),
        ),
        (
            "minimum-viable-at-upper-bound",
            json!({"minimumViableContactDurationSeconds": 21600}),
        ),
        (
            "pre-and-post-pass-at-their-bounds",
            json!({
                "contactPrePassDurationSeconds": 0,
                "contactPostPassDurationSeconds": 21600,
            }),
        ),
        (
            "dataflow-edges",
            json!({"dataflowEdges": [[downlink_arn.clone(), endpoint_arn.clone()]]}),
        ),
        (
            "tracking-config-reference",
            json!({"trackingConfigArn": other_tracking_arn.clone()}),
        ),
        (
            "telemetry-sink-reference",
            json!({"telemetrySinkConfigArn": telemetry_arn.clone()}),
        ),
        (
            "streams-encryption",
            json!({
                "streamsKmsKey": {"kmsAliasName": "alias/groundstation-streams"},
                "streamsKmsRole": "arn:aws:iam::000000000000:role/GroundStationStreamsRole",
            }),
        ),
        (
            "name-and-duration-together",
            json!({
                "name": "profile renamed twice",
                "minimumViableContactDurationSeconds": 600,
            }),
        ),
    ];

    for (case, update) in rows {
        // A fresh profile per row, so each row observes only its own update.
        let created = base_profile_request(&format!("profile {case}"), &tracking_arn);
        let id = create_mission_profile(&router, Value::Object(created.clone())).await;

        let updated = send(
            &router,
            "PUT",
            &format!("/missionprofile/{id}"),
            Some(update.clone()),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateMissionProfile 200, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("missionProfileId"),
            id,
            "case {case}: UpdateMissionProfile must answer the profile it updated, got {}",
            updated.body
        );

        // The rule, as arithmetic on the request: the profile a read reports is the one
        // that was created with every member this update carried written over it. Each
        // update member is named exactly as the read reports it.
        let mut expected = created.clone();
        for (member, value) in update.as_object().expect("the update is a JSON object") {
            expected.insert(member.clone(), value.clone());
        }

        let read = get_mission_profile(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetMissionProfile 200, got {} {}",
            read.status, read.body
        );
        for (member, value) in &expected {
            assert_eq!(
                read.field(member),
                value,
                "case {case}: member {member} must read back as {value}, got {}",
                read.field(member)
            );
        }
        assert_eq!(
            read.string("missionProfileId"),
            id,
            "case {case}: an update must not move the profile's id, got {}",
            read.body
        );
        assert_eq!(
            read.field("region"),
            &json!(REGION),
            "case {case}: an update must not move the profile's region, got {}",
            read.field("region")
        );
    }

    // Refusals: a reference that resolves to nothing, a reference of the wrong config
    // type, and a duration outside the modelled range. None of them may change the
    // stored profile.
    let base = base_profile_request("profile refusals", &tracking_arn);
    let id = create_mission_profile(&router, Value::Object(base.clone())).await;
    let refusals: Vec<(&str, Value)> = vec![
        (
            "absent-tracking-config",
            json!({"trackingConfigArn": ABSENT_CONFIG_ARN}),
        ),
        (
            "tracking-reference-naming-another-type",
            json!({"trackingConfigArn": downlink_arn.clone()}),
        ),
        (
            "telemetry-reference-naming-another-type",
            json!({"telemetrySinkConfigArn": downlink_arn.clone()}),
        ),
        (
            "dataflow-edge-to-an-absent-config",
            json!({"dataflowEdges": [[downlink_arn.clone(), ABSENT_CONFIG_ARN]]}),
        ),
        (
            "minimum-viable-below-the-range",
            json!({"minimumViableContactDurationSeconds": 0}),
        ),
        (
            "minimum-viable-above-the-range",
            json!({"minimumViableContactDurationSeconds": 21601}),
        ),
        (
            "pre-pass-above-the-range",
            json!({"contactPrePassDurationSeconds": 21601}),
        ),
    ];
    for (case, update) in refusals {
        let refused = send(
            &router,
            "PUT",
            &format!("/missionprofile/{id}"),
            Some(update.clone()),
        )
        .await;
        assert!(
            (400..500).contains(&refused.status),
            "case {case}: expected a client refusal, got {} {}",
            refused.status,
            refused.body
        );
        let read = get_mission_profile(&router, &id).await;
        for (member, value) in &base {
            assert_eq!(
                read.field(member),
                value,
                "case {case}: a refused update must leave {member} at {value}, got {}",
                read.field(member)
            );
        }
    }

    // And an update of a profile that was never created names no profile to update.
    let unknown = send(
        &router,
        "PUT",
        &format!("/missionprofile/{ABSENT_UUID}"),
        Some(json!({"name": "nothing to rename"})),
    )
    .await;
    assert_eq!(
        unknown.status, 434,
        "case unknown-mission-profile-id: expected 434, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "case unknown-mission-profile-id: wrong error shape, got {}",
        unknown.body
    );
}

#[tokio::test]
async fn a_read_serializes_tags_as_a_map_even_when_the_resource_carries_none() {
    let router = harness();
    let tracking_arn = create_tracking_config(&router, "tags tracking").await;

    // The rule generalises over how many tags a resource carries, so the table runs
    // none, one and several against each kind of taggable resource this service reads
    // back: a config, a mission profile and a contact.
    let tag_sets: Vec<(&str, Value)> = vec![
        ("no-tags", json!({})),
        ("one-tag", json!({"mission": "demo"})),
        (
            "several-tags",
            json!({"mission": "demo", "stage": "probe", "owner": "ops"}),
        ),
    ];

    for (case, tags) in tag_sets {
        // A config: created with the tags, or with the member omitted entirely for the
        // untagged row — an omitted member is how an untagged resource is created.
        let sent_tags = (!tags.as_object().expect("a JSON object").is_empty())
            .then(|| tags.clone());
        let (config_id, config_type, _) = create_config(
            &router,
            &format!("tags config {case}"),
            json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
            sent_tags.clone(),
        )
        .await;
        let config = get_config(&router, &config_type, &config_id).await;
        assert_eq!(
            config.field("tags"),
            &tags,
            "case {case}: GetConfig must serialize tags as a map, got {}",
            config.field("tags")
        );

        // A mission profile.
        let mut request = base_profile_request(&format!("tags profile {case}"), &tracking_arn);
        match &sent_tags {
            Some(tags) => {
                request.insert("tags".to_string(), tags.clone());
            }
            None => {
                request.remove("tags");
            }
        }
        let profile_id = create_mission_profile(&router, Value::Object(request)).await;
        let profile = get_mission_profile(&router, &profile_id).await;
        assert_eq!(
            profile.field("tags"),
            &tags,
            "case {case}: GetMissionProfile must serialize tags as a map, got {}",
            profile.field("tags")
        );

        // A contact, read through `DescribeContact`.
        let profile_arn = profile.string("missionProfileArn");
        let mut reserve = json!({
            "missionProfileArn": profile_arn,
            "groundStation": "Ohio-1",
            "startTime": 1_800_000_000_i64,
            "endTime": 1_800_000_600_i64,
        });
        if let Some(tags) = &sent_tags {
            reserve["tags"] = tags.clone();
        }
        let reserved = send(&router, "POST", "/contact", Some(reserve)).await;
        assert_eq!(
            reserved.status, 200,
            "case {case}: expected ReserveContact 200, got {} {}",
            reserved.status, reserved.body
        );
        let contact_id = reserved.string("contactId");
        let contact = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
        assert_eq!(
            contact.field("tags"),
            &tags,
            "case {case}: DescribeContact must serialize tags as a map, got {}",
            contact.field("tags")
        );
    }
}

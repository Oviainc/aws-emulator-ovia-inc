// SPDX-License-Identifier: CC0-1.0

//! What this service says when a resource a request names does not exist.
//!
//! The RULE under test: a `ResourceNotFoundException` carries the text of the OPERATION
//! that raised it — a per-operation template, built from the identifiers the request
//! itself carried where that operation's text names them and a fixed literal where it
//! does not — never one template shared across operations. So the suites below assert,
//! for each operation, the whole class of inputs that reach its not-found branch:
//!
//! * `GetConfig` names the id and the type its two path labels carried, for every one of
//!   the model's eight `ConfigCapabilityType` values;
//! * `CreateConfig` names the capability its reference required, spelled as the model
//!   spells that capability's shape, plus the ARN the request gave;
//! * `DescribeContact` and `CancelContact` answer DIFFERENT texts for the SAME missing
//!   contact — one a bare literal, one quoting the id;
//! * `GetMissionProfile` and `ReserveContact` answer the SAME literal, which echoes
//!   neither the id nor the ARN that was asked for;
//! * `CreateMissionProfile` names the id and config type its lookup searched for and the
//!   account it was scoped to, for every ARN-valued reference a profile carries.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — using the harness
//! `scheduling_loop.rs` already stands up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    capability_display_name, MISSION_PROFILE_NOT_FOUND,
};

use crate::scheduling_loop::{
    antenna_uplink_config_data, create_config, create_mission_profile, create_tracking_config,
    harness, send, Reply, ABSENT_CONFIG_ARN, ABSENT_MISSION_PROFILE_ARN, ABSENT_UUID, REGION,
};

/// Every value of the model's `ConfigCapabilityType`, beside the name of the shape the
/// matching `ConfigTypeData` member targets.
///
/// Source: `smithy_query(query="shape", shape="ConfigCapabilityType,ConfigTypeData",
/// model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/\
/// com-amazonaws-groundstation-groundstation.json")` — the enum's eight values and the
/// union's eight member targets. All eight are listed, including the six the issue did
/// not cite, because the user-facing spelling is derived from the token and the derivation
/// has to hold for each of them.
const CAPABILITY_TYPES: &[(&str, &str)] = &[
    ("antenna-downlink", "AntennaDownlinkConfig"),
    (
        "antenna-downlink-demod-decode",
        "AntennaDownlinkDemodDecodeConfig",
    ),
    ("antenna-uplink", "AntennaUplinkConfig"),
    ("dataflow-endpoint", "DataflowEndpointConfig"),
    ("s3-recording", "S3RecordingConfig"),
    ("telemetry-sink", "TelemetrySinkConfig"),
    ("tracking", "TrackingConfig"),
    ("uplink-echo", "UplinkEchoConfig"),
];

/// Well-formed `Uuid`s (model pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) that name nothing:
/// a run of zeros, a run of `f`s and a mixed one, so an assertion about a message cannot
/// be passing on one id's shape.
const ABSENT_UUIDS: &[&str] = &[
    ABSENT_UUID,
    "ffffffff-ffff-4fff-bfff-ffffffffffff",
    "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
];

/// A well-formed `ConfigArn` for `(config_type, config_id)` in `region` under `account`.
///
/// Pattern (from `configArn` in `smithy_query(query="operation", operation="GetConfig")`):
/// `^arn:aws:groundstation:[-a-z0-9]{1,50}:[0-9]{12}:config/[a-z0-9]+(-[a-z0-9]+){0,4}/<uuid>…$`.
fn config_arn(region: &str, account: &str, config_type: &str, config_id: &str) -> String {
    format!("arn:aws:groundstation:{region}:{account}:config/{config_type}/{config_id}")
}

/// A well-formed `MissionProfileArn` for `mission_profile_id` in `region`.
fn mission_profile_arn(region: &str, mission_profile_id: &str) -> String {
    format!(
        "arn:aws:groundstation:{region}:{}:mission-profile/{mission_profile_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `message` of a not-found answer, after asserting the answer really is this
/// service's `ResourceNotFoundException`: HTTP 434 and that error shape, per
/// `smithy_query` (`@error("client")`, `@httpError(434)`).
fn not_found_message(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 434,
        "case {case}: expected the modelled ResourceNotFoundException status 434, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case}: wrong error shape, got {}",
        reply.body
    );
    reply.string("message")
}

#[tokio::test]
async fn capability_spelling_is_the_model_shape_name_without_its_config_suffix() {
    for (token, shape) in CAPABILITY_TYPES {
        let expected = shape
            .strip_suffix("Config")
            .expect("every ConfigTypeData member targets a shape named <Capability>Config");
        let actual = capability_display_name(token);
        assert_eq!(
            &actual, expected,
            "case {token}: the user-facing capability name is {shape}'s name without its \
             Config suffix, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn get_config_not_found_names_the_id_and_the_type_its_path_asked_for() {
    let router = harness();

    // A config that DOES exist, so a request under another type is a miss on the pair
    // rather than on the id alone — the same not-found branch, reached differently.
    let (present_id, present_type, _) = create_config(
        &router,
        "present tracking",
        json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        None,
    )
    .await;

    // The class: every modelled config type, crossed with ids that name nothing.
    for (config_type, _) in CAPABILITY_TYPES {
        for config_id in ABSENT_UUIDS {
            let case = format!("{config_type}/{config_id}");
            let reply = send(
                &router,
                "GET",
                &format!("/config/{config_type}/{config_id}"),
                None,
            )
            .await;
            let message = not_found_message(&case, &reply);
            assert_eq!(
                message,
                format!("No config found with id {config_id} and type {config_type}"),
                "case {case}: GetConfig names the id then the type its path carried, got \
                 {message:?}"
            );
        }
    }

    // The id exists, but not under the type the path named: the message reports what the
    // request asked for, not what is stored.
    for (config_type, _) in CAPABILITY_TYPES {
        if *config_type == present_type {
            continue;
        }
        let case = format!("stored-under-another-type {config_type}");
        let reply = send(
            &router,
            "GET",
            &format!("/config/{config_type}/{present_id}"),
            None,
        )
        .await;
        let message = not_found_message(&case, &reply);
        assert_eq!(
            message,
            format!("No config found with id {present_id} and type {config_type}"),
            "case {case}: GetConfig reports the pair the request asked for, got {message:?}"
        );
    }
}

#[tokio::test]
async fn create_config_names_the_capability_its_reference_required() {
    let router = harness();

    // The config an `uplinkEchoConfig` may legitimately echo, and one of another type,
    // so both ways of failing the reference are covered.
    let (_, _, uplink_arn) = create_config(
        &router,
        "echo source",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;
    let tracking_arn = create_tracking_config(&router, "echo wrong type").await;

    // Rows differ in WHY the reference does not resolve and in the ARN's own spelling
    // (region, account, config type, id): the message names the capability the member
    // required, which is none of those.
    let rows: Vec<(&str, String)> = vec![
        ("absent-shared-arn", ABSENT_CONFIG_ARN.to_string()),
        (
            "absent-antenna-uplink-arn",
            config_arn(
                REGION,
                engine::EMULATOR_ACCOUNT_ID,
                "antenna-uplink",
                ABSENT_UUIDS[1],
            ),
        ),
        (
            "absent-in-another-region",
            config_arn(
                "eu-west-1",
                engine::EMULATOR_ACCOUNT_ID,
                "antenna-uplink",
                ABSENT_UUIDS[2],
            ),
        ),
        (
            "absent-in-another-account",
            config_arn("us-west-2", "123456789012", "s3-recording", ABSENT_UUIDS[0]),
        ),
        ("resolves-to-another-type", tracking_arn),
    ];

    for (case, arn) in rows {
        let reply = send(
            &router,
            "POST",
            "/config",
            Some(json!({
                "name": "echo of nothing",
                "configData": {"uplinkEchoConfig": {
                    "enabled": true,
                    "antennaUplinkConfigArn": arn,
                }},
            })),
        )
        .await;
        let message = not_found_message(case, &reply);
        assert_eq!(
            message,
            format!("Unable to find AntennaUplink Config with ARN: {arn}"),
            "case {case}: CreateConfig names the required capability as the model spells \
             its shape, got {message:?}"
        );
        // The capability the member required is named in the text BEFORE the ARN, so
        // an ARN that happens to spell the wire token cannot mask a leak of it.
        let prefix = message.split(" with ARN: ").next().unwrap_or_default();
        assert!(
            !prefix.contains("antenna-uplink"),
            "case {case}: the wire enum token must not leak into user-facing text, got \
             {prefix:?}"
        );
    }

    // And the reference that DOES resolve is not refused at all, so the branch above is
    // reached only by a reference that fails.
    let accepted = send(
        &router,
        "POST",
        "/config",
        Some(json!({
            "name": "echo of the uplink",
            "configData": {"uplinkEchoConfig": {
                "enabled": true,
                "antennaUplinkConfigArn": uplink_arn,
            }},
        })),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case resolves: a reference to an antenna-uplink config is accepted, got {} {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn the_two_contact_readers_answer_their_own_texts_for_the_same_missing_contact() {
    let router = harness();

    for contact_id in ABSENT_UUIDS {
        let describe = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
        let described = not_found_message(&format!("describe {contact_id}"), &describe);
        assert_eq!(
            described, "Contact not found",
            "case describe {contact_id}: DescribeContact answers a bare literal, got \
             {described:?}"
        );
        assert!(
            !described.contains(*contact_id),
            "case describe {contact_id}: DescribeContact must not disclose the requested \
             id, got {described:?}"
        );

        let cancel = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
        let cancelled = not_found_message(&format!("cancel {contact_id}"), &cancel);
        assert_eq!(
            cancelled,
            format!("Contact with id '{contact_id}' does not exist."),
            "case cancel {contact_id}: CancelContact quotes the id and ends the sentence, \
             got {cancelled:?}"
        );

        assert_ne!(
            described, cancelled,
            "case {contact_id}: the two operations' texts for the same missing contact are \
             not the same text"
        );
    }
}

#[tokio::test]
async fn both_mission_profile_resolvers_answer_the_one_mission_profile_literal() {
    let router = harness();

    // Resolved by `{missionProfileId}`.
    for mission_profile_id in ABSENT_UUIDS {
        let reply = send(
            &router,
            "GET",
            &format!("/missionprofile/{mission_profile_id}"),
            None,
        )
        .await;
        let message = not_found_message(&format!("get {mission_profile_id}"), &reply);
        assert_eq!(
            message, MISSION_PROFILE_NOT_FOUND,
            "case get {mission_profile_id}: GetMissionProfile answers the service's \
             mission-profile literal, got {message:?}"
        );
        assert!(
            !message.contains(*mission_profile_id),
            "case get {mission_profile_id}: the literal discloses no id, got {message:?}"
        );
    }

    // Resolved by `missionProfileArn`, over ARNs differing in region and id.
    let arns = vec![
        ("shared-arn", ABSENT_MISSION_PROFILE_ARN.to_string()),
        (
            "other-region",
            mission_profile_arn("ap-southeast-2", ABSENT_UUIDS[1]),
        ),
        ("same-region", mission_profile_arn(REGION, ABSENT_UUIDS[2])),
    ];
    for (case, arn) in arns {
        let reply = send(
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
        let message = not_found_message(case, &reply);
        assert_eq!(
            message, MISSION_PROFILE_NOT_FOUND,
            "case {case}: ReserveContact answers the service's mission-profile literal, \
             got {message:?}"
        );
        assert!(
            !message.contains(&arn),
            "case {case}: the literal echoes no ARN back, got {message:?}"
        );
    }
}

#[tokio::test]
async fn create_mission_profile_names_the_config_its_lookup_searched_for() {
    let router = harness();
    let tracking_arn = create_tracking_config(&router, "reference tracking").await;
    let (endpoint_id, _, endpoint_arn) = create_config(
        &router,
        "reference endpoint",
        json!({"dataflowEndpointConfig": {"dataflowEndpointName": "downlink"}}),
        None,
    )
    .await;

    /// One row: the case name, the request, and the `(configId, configType)` the lookup
    /// that failed searched under.
    struct Row {
        case: &'static str,
        request: Value,
        config_id: String,
        config_type: &'static str,
    }

    let profile = |extra: Value| -> Value {
        let mut body = json!({
            "name": "reference probe",
            "minimumViableContactDurationSeconds": 180,
            "dataflowEdges": [],
            "trackingConfigArn": tracking_arn,
        });
        for (key, value) in extra.as_object().expect("the row's overrides are an object") {
            body[key.as_str()] = value.clone();
        }
        body
    };

    let rows = vec![
        // The member names ONE type, so that is the type the lookup searched under —
        // whatever the ARN itself spells.
        Row {
            case: "absent-trackingConfigArn",
            request: profile(json!({"trackingConfigArn": config_arn(
                REGION, engine::EMULATOR_ACCOUNT_ID, "tracking", ABSENT_UUIDS[0])})),
            config_id: ABSENT_UUIDS[0].to_string(),
            config_type: "tracking",
        },
        Row {
            case: "absent-telemetrySinkConfigArn",
            request: profile(json!({"telemetrySinkConfigArn": config_arn(
                REGION, engine::EMULATOR_ACCOUNT_ID, "telemetry-sink", ABSENT_UUIDS[1])})),
            config_id: ABSENT_UUIDS[1].to_string(),
            config_type: "telemetry-sink",
        },
        // A reference in another region or account resolves to nothing here, and the
        // account the message names is the CALLER's — the account the lookup was scoped
        // to — not the one spelled in the ARN.
        Row {
            case: "trackingConfigArn-of-another-account",
            request: profile(json!({"trackingConfigArn": config_arn(
                "us-west-2", "123456789012", "tracking", ABSENT_UUIDS[2])})),
            config_id: ABSENT_UUIDS[2].to_string(),
            config_type: "tracking",
        },
        // A dataflow edge joins configs of whatever types the dataflow runs between, so
        // its endpoints are searched under the type their own ARN names.
        Row {
            case: "absent-edge-source",
            request: profile(json!({"dataflowEdges": [[
                config_arn(REGION, engine::EMULATOR_ACCOUNT_ID, "s3-recording", ABSENT_UUIDS[1]),
                endpoint_arn,
            ]]})),
            config_id: ABSENT_UUIDS[1].to_string(),
            config_type: "s3-recording",
        },
        Row {
            case: "absent-edge-destination",
            request: profile(json!({"dataflowEdges": [[
                endpoint_arn,
                config_arn(REGION, engine::EMULATOR_ACCOUNT_ID, "antenna-downlink-demod-decode",
                    ABSENT_UUIDS[2]),
            ]]})),
            config_id: ABSENT_UUIDS[2].to_string(),
            config_type: "antenna-downlink-demod-decode",
        },
        // The ARN resolves to a config that EXISTS but is of another type: the lookup
        // still searched under the type the member requires, so that is what it reports.
        Row {
            case: "trackingConfigArn-resolves-to-another-type",
            request: profile(json!({"trackingConfigArn": endpoint_arn})),
            config_id: endpoint_id.clone(),
            config_type: "tracking",
        },
    ];

    for row in rows {
        let reply = send(&router, "POST", "/missionprofile", Some(row.request)).await;
        let message = not_found_message(row.case, &reply);
        assert_eq!(
            message,
            format!(
                "No config found with ID {} and config type: {} for customer {}",
                row.config_id,
                row.config_type,
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {}: CreateMissionProfile names the id and type it searched for and the \
             account it was scoped to, got {message:?}",
            row.case
        );
    }

    // A profile whose references all resolve is created, so the branch above is reached
    // only by a reference that fails.
    let accepted = create_mission_profile(&router, "all references resolve", 180, None, None).await;
    assert!(
        accepted.contains(":mission-profile/"),
        "case resolves: a profile with resolvable references is created, got {accepted:?}"
    );
}

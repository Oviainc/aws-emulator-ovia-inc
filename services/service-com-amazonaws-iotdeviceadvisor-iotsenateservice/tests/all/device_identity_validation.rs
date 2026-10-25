// SPDX-License-Identifier: CC0-1.0

//! The device a request names, over the wire.
//!
//! Wherever this service accepts a device — a suite run's `primaryDevice`, a suite
//! definition's `devices`, and the role a definition lets Device Advisor assume — the
//! request has to NAME the device the way the member documents, and every ARN it carries
//! has to BE an Amazon Resource Name of the service that member identifies. The tables
//! below span the class the rule generalises over: which naming member is present, how many
//! are present, and each way a value can fail to be an ARN of the right service (not an ARN
//! at all, an ARN of another service, an empty partition, an empty resource, too few
//! fields), asserted on both operations that accept a definition configuration and on the
//! one that accepts a run configuration.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket (the harness `suite_lifecycle` owns), so what is asserted is the
//! service's wire behaviour, including that a refused request stored nothing.

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::suite_lifecycle::{thing_arn, Emulator, ROLE_ARN};

/// The region every case below signs for.
const REGION: &str = "us-east-1";

/// A certificate ARN for a device under test: `certificateArn` names an IoT resource, the
/// same service `thingArn` does (source: `DeviceUnderTest`, whose members are a thing, a
/// certificate and the role the device assumes).
fn certificate_arn(name: &str) -> String {
    format!("arn:aws:iot:us-east-1:000000000000:cert/{name}")
}

/// Values that are NOT an ARN of `iot`, each with the way it fails: the class the ARN-form
/// rule generalises over, independent of which member carries the value.
fn non_iot_arns() -> Vec<(&'static str, String)> {
    vec![
        // Every value here is at least the modelled 20 characters (`AmazonResourceName` is
        // `len 20..2048`), so what is under test is the ARN form, not the length bound.
        ("not an ARN at all", "my-favourite-thing-name12345".to_string()),
        ("an IAM role ARN", ROLE_ARN.to_string()),
        (
            "an ARN of another service",
            "arn:aws:iotwireless:us-east-1:000000000000:thing/t".to_string(),
        ),
        (
            "an empty partition",
            "arn::iot:us-east-1:000000000000:thing/t".to_string(),
        ),
        (
            "an empty resource",
            "arn:aws:iot:us-east-1:000000000000:".to_string(),
        ),
        (
            "too few fields",
            "arn:aws:iot:thing/my-thing-name-here".to_string(),
        ),
    ]
}

/// How many suite runs and suite definitions the shared store actually holds.
fn stored_counts(emulator: &Emulator) -> (usize, usize) {
    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let store = state.lock();
    (store.runs.len(), store.definitions.len())
}

/// Create one suite definition and hand back its id.
async fn definition(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": name,
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [{ "thingArn": thing_arn(0) }],
            }),
            REGION,
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: setup CreateSuiteDefinition failed with {} {}",
        created.status, created.body
    );
    created.string("suiteDefinitionId")
}

/// Start a run of `definition_id` with `configuration`.
async fn start_run(emulator: &Emulator, definition_id: &str, configuration: &Value) -> super::suite_lifecycle::Answer {
    emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            REGION,
            Some(json!({ "suiteRunConfiguration": configuration })),
        )
        .await
}

/// The `primaryDevice` values a run configuration is refused for: one that names no device
/// at all, one that names only the role (the member requires a thing or a certificate), and
/// one row per way an ARN can fail to be one of the member's service, on each naming member.
fn refused_primary_devices() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        ("names no device at all".to_string(), json!({})),
        (
            // `SuiteRunConfiguration$primaryDevice` requires a thing ARN or a certificate
            // ARN, so the device role alone does not name the run's primary device.
            "names only the device role".to_string(),
            json!({ "deviceRoleArn": ROLE_ARN }),
        ),
    ];
    for (failure, value) in non_iot_arns() {
        rows.push((
            format!("thingArn is {failure}"),
            json!({ "thingArn": value }),
        ));
        rows.push((
            format!("certificateArn is {failure}"),
            json!({ "certificateArn": value }),
        ));
    }
    rows.push((
        "a valid thingArn beside a certificateArn that is not an ARN".to_string(),
        json!({
            "thingArn": thing_arn(1),
            "certificateArn": "certificate-name-not-an-arn",
        }),
    ));
    rows.push((
        "deviceRoleArn is an IoT thing ARN".to_string(),
        json!({ "thingArn": thing_arn(2), "deviceRoleArn": thing_arn(3) }),
    ));
    rows
}

/// A run configuration that does not name a device the way the member requires is refused
/// with the modelled client-input error, and the refusal stores NOTHING: no run becomes
/// visible, and the next start of the same definition is not blocked by a phantom run.
#[tokio::test]
async fn a_run_configuration_that_does_not_name_a_device_is_refused() {
    let emulator = Emulator::new();
    let definition_id = definition(&emulator, "primary-device-refusals").await;

    for (case, primary_device) in refused_primary_devices() {
        let configuration = json!({ "primaryDevice": primary_device });
        let answer = start_run(&emulator, &definition_id, &configuration).await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException from StartSuiteRun, got {} {:?} {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        assert!(
            answer.member("suiteRunId").is_null() && answer.member("suiteRunArn").is_null(),
            "case {case:?}: a refused StartSuiteRun must mint no run, got {}",
            answer.body
        );

        let (runs, _) = stored_counts(&emulator);
        assert_eq!(
            runs, 0,
            "case {case:?}: a refused StartSuiteRun must store no run, the store holds {runs}"
        );
        let listed = emulator.call("GET", "/suiteRuns", REGION, None).await;
        assert_eq!(
            listed.member("suiteRunsList"),
            &json!([]),
            "case {case:?}: a refused StartSuiteRun must be invisible to ListSuiteRuns, got {}",
            listed.body
        );
    }

    // No phantom run is in progress, so a start that DOES name a device still succeeds.
    let accepted = start_run(
        &emulator,
        &definition_id,
        &json!({ "primaryDevice": { "thingArn": thing_arn(0) } }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a valid start after the refusals\": expected HTTP 200, got {} {}",
        accepted.status, accepted.body
    );
}

/// The `primaryDevice` values a run configuration is ACCEPTED for: each naming member the
/// shape documents, alone and together, so the rule refuses only what it must.
fn accepted_primary_devices() -> Vec<(&'static str, Value)> {
    vec![
        ("thingArn alone", json!({ "thingArn": thing_arn(4) })),
        (
            "certificateArn alone",
            json!({ "certificateArn": certificate_arn("primary-cert") }),
        ),
        (
            "thingArn and certificateArn",
            json!({
                "thingArn": thing_arn(5),
                "certificateArn": certificate_arn("both-cert"),
            }),
        ),
        (
            "every naming member",
            json!({
                "thingArn": thing_arn(6),
                "certificateArn": certificate_arn("all-cert"),
                "deviceRoleArn": ROLE_ARN,
            }),
        ),
    ]
}

/// A run configuration that names its primary device is accepted and read back verbatim
/// through `GetSuiteRun`.
#[tokio::test]
async fn a_run_configuration_that_names_a_device_round_trips() {
    let emulator = Emulator::new();

    for (case, primary_device) in accepted_primary_devices() {
        // One run at a time per definition, so each row runs against its own definition.
        let definition_id = definition(&emulator, &format!("accepted-{case}")).await;
        let configuration = json!({ "primaryDevice": primary_device });
        let started = start_run(&emulator, &definition_id, &configuration).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
            started.status, started.body
        );
        let run_id = started.string("suiteRunId");

        let read = emulator
            .call(
                "GET",
                &format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}"),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetSuiteRun, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.member("suiteRunConfiguration"),
            &configuration,
            "case {case:?}: GetSuiteRun must answer with the stored configuration, got {}",
            read.body
        );
    }
}

/// The definition configurations refused for the ARNs they point at: the role Device
/// Advisor assumes, and each device under test at either list position.
fn refused_definition_configurations() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = Vec::new();
    for (failure, value) in non_iot_arns() {
        rows.push((
            format!("devices[0].thingArn is {failure}"),
            json!({
                "suiteDefinitionName": "bad-device",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [{ "thingArn": value }],
            }),
        ));
    }
    rows.push((
        // `DeviceUnderTestList` is `len 0..2`, so the second entry is the last position.
        "devices[1].certificateArn is not an ARN".to_string(),
        json!({
            "suiteDefinitionName": "bad-second-device",
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
            "devices": [
                { "thingArn": thing_arn(0) },
                { "certificateArn": "certificate-name-not-an-arn" },
            ],
        }),
    ));
    rows.push((
        // `DeviceUnderTest`: "A thing ARN, certificate ARN or device role ARN is required".
        "a device that names nothing".to_string(),
        json!({
            "suiteDefinitionName": "empty-device",
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
            "devices": [{}],
        }),
    ));
    rows.push((
        "devicePermissionRoleArn is not an ARN at all".to_string(),
        json!({
            "suiteDefinitionName": "bad-role",
            "rootGroup": "g",
            "devicePermissionRoleArn": "DeviceAdvisorRole-not-an-arn",
        }),
    ));
    rows.push((
        "devicePermissionRoleArn is an IoT thing ARN".to_string(),
        json!({
            "suiteDefinitionName": "role-of-wrong-service",
            "rootGroup": "g",
            "devicePermissionRoleArn": thing_arn(9),
        }),
    ));
    rows
}

/// Both operations that accept a suite definition configuration refuse one whose device or
/// role ARNs are not ARNs of the service the member names — and the refusal changes
/// nothing: no definition is created, and an existing one keeps the configuration it had.
#[tokio::test]
async fn a_definition_configuration_with_a_bad_device_or_role_arn_is_refused() {
    let emulator = Emulator::new();

    let live = json!({
        "suiteDefinitionName": "update-target",
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
        "devices": [{ "deviceRoleArn": ROLE_ARN }],
    });
    let created = emulator.create_definition(&live, REGION).await;
    assert_eq!(
        created.status, 200,
        "setup CreateSuiteDefinition failed with {} {}",
        created.status, created.body
    );
    let target_id = created.string("suiteDefinitionId");
    let (_, definitions_before) = stored_counts(&emulator);

    for (case, configuration) in refused_definition_configurations() {
        let answer = emulator.create_definition(&configuration, REGION).await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException from CreateSuiteDefinition, got \
             {} {:?} {}",
            answer.status,
            answer.error_code,
            answer.body
        );

        let updated = emulator
            .call(
                "PATCH",
                &format!("/suiteDefinitions/{target_id}"),
                REGION,
                Some(json!({ "suiteDefinitionConfiguration": configuration })),
            )
            .await;
        assert_eq!(
            (updated.status, updated.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException from UpdateSuiteDefinition, got \
             {} {:?} {}",
            updated.status,
            updated.error_code,
            updated.body
        );

        let (_, definitions_now) = stored_counts(&emulator);
        assert_eq!(
            definitions_now, definitions_before,
            "case {case:?}: a refused call must not change how many definitions are stored, \
             expected {definitions_before}, got {definitions_now}"
        );
        let read = emulator
            .call("GET", &format!("/suiteDefinitions/{target_id}"), REGION, None)
            .await;
        assert_eq!(
            read.member("suiteDefinitionConfiguration"),
            &live,
            "case {case:?}: a refused update must leave the live configuration alone, got {}",
            read.body
        );
        assert_eq!(
            read.string("latestVersion"),
            "v1",
            "case {case:?}: a refused update must mint no version, got {}",
            read.body
        );
    }
}

/// A device under test of a suite definition may be named by ANY of the three ARNs the
/// shape documents, so the rule accepts each of them where the shape allows it.
#[tokio::test]
async fn a_definition_device_may_be_named_by_any_of_its_arns() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, Value)> = vec![
        ("thingArn", json!([{ "thingArn": thing_arn(0) }])),
        (
            "certificateArn",
            json!([{ "certificateArn": certificate_arn("definition-cert") }]),
        ),
        // The device role alone names a device under test, though it does not name a suite
        // run's primary device.
        ("deviceRoleArn", json!([{ "deviceRoleArn": ROLE_ARN }])),
        ("an empty device list", json!([])),
        (
            "two devices, each named differently",
            json!([
                { "thingArn": thing_arn(1) },
                { "deviceRoleArn": ROLE_ARN },
            ]),
        ),
    ];

    for (case, devices) in rows {
        let configuration = json!({
            "suiteDefinitionName": format!("devices-{case}"),
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
            "devices": devices,
        });
        let created = emulator.create_definition(&configuration, REGION).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
            created.status, created.body
        );
        let id = created.string("suiteDefinitionId");
        let read = emulator
            .call("GET", &format!("/suiteDefinitions/{id}"), REGION, None)
            .await;
        assert_eq!(
            read.member("suiteDefinitionConfiguration"),
            &configuration,
            "case {case:?}: GetSuiteDefinition must answer with the stored configuration, got {}",
            read.body
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The ARN rules a suite definition configuration carries, over the wire.
//!
//! `SuiteDefinitionConfiguration` points at three kinds of ARN: the IAM role Device Advisor
//! assumes for the devices (`devicePermissionRoleArn`), and, per device under test, the
//! thing, the certificate or the role the device assumes. The model states the device rule
//! in prose on the shape — "Information of a test device. A thing ARN, certificate ARN or
//! device role ARN is required" — with all three members optional, and types every member
//! as `AmazonResourceName`, so neither the naming requirement nor the ARN FORM is a
//! constraint the framework can refuse before a handler runs.
//!
//! The tables below pin that rule over the class it generalises over rather than over one
//! configuration: which member carries the value, which list position the device sits at,
//! how many devices the list holds, and each way a value fails to be an ARN of the service
//! the member names. Every row is asserted on BOTH operations that accept a definition
//! configuration (`CreateSuiteDefinition` and `UpdateSuiteDefinition`), and every refusal is
//! asserted to have stored nothing: no definition is minted, the live configuration of an
//! existing definition is untouched, and no listing reports the refused configuration.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket (the harness `suite_lifecycle` owns).

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::suite_lifecycle::{thing_arn, Answer, Emulator, ROLE_ARN};

/// The region every case below signs for.
const REGION: &str = "us-east-1";

/// The configuration an existing definition holds while the refused calls are made, so a
/// refused update can be shown to have left it alone.
fn live_configuration() -> Value {
    json!({
        "suiteDefinitionName": "arn-rules-live",
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
        "devices": [{ "thingArn": thing_arn(0) }],
    })
}

/// How many suite definitions the shared store actually holds.
fn stored_definitions(emulator: &Emulator) -> usize {
    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let store = state.lock();
    store.definitions.len()
}

/// The `message` an error answer carries, or the empty string when it carries none.
fn message(answer: &Answer) -> String {
    answer
        .member("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every way a value fails to be an Amazon Resource Name of `service`, each paired with the
/// failure it exhibits: the class the ARN-form rule generalises over, independent of which
/// member carries the value.
///
/// Every value here is at least the modelled 20 characters (`AmazonResourceName` is
/// `len 20..2048`, source: `smithy_query(query="shape",shape="DeviceUnderTest")`), so what a
/// refusal proves is the ARN FORM and not the length bound the framework enforces before the
/// handler runs.
fn malformed_arns_of(service: &str) -> Vec<(String, String)> {
    vec![
        (
            "not an ARN at all".to_string(),
            format!("{service}-resource-name-not-an-arn"),
        ),
        (
            "an ARN of another service".to_string(),
            format!("arn:aws:not{service}:us-east-1:000000000000:resource/r"),
        ),
        (
            "an empty partition".to_string(),
            format!("arn::{service}:us-east-1:000000000000:resource/r"),
        ),
        (
            "an empty service field".to_string(),
            "arn:aws::us-east-1:000000000000:resource/r".to_string(),
        ),
        (
            "an empty resource".to_string(),
            format!("arn:aws:{service}:us-east-1:000000000000:"),
        ),
        (
            "too few fields".to_string(),
            format!("arn:aws:{service}:resource/the-resource-name"),
        ),
        (
            "an upper-case scheme".to_string(),
            format!("ARN:aws:{service}:us-east-1:000000000000:resource/r"),
        ),
    ]
}

/// The AWS service whose ARNs name a thing and a certificate.
const IOT: &str = "iot";
/// The AWS service whose ARNs name a role: the device's, and the one Device Advisor assumes.
const IAM: &str = "iam";

/// A configuration carrying `devices`, with the rest of the required members filled in.
fn with_devices(name: &str, devices: Value) -> Value {
    json!({
        "suiteDefinitionName": name,
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
        "devices": devices,
    })
}

/// The refused rows: `(case, configuration, the member path the refusal has to name)`.
///
/// Three groups, each a property of the SHAPE rather than of one operation: a device under
/// test that names no device at all (at either list position, and in a list of either
/// permitted size), a device whose ARN is not one of the service its member names, and a
/// `devicePermissionRoleArn` that is not an IAM ARN.
fn refused_rows() -> Vec<(String, Value, String)> {
    let mut rows: Vec<(String, Value, String)> = Vec::new();

    // `DeviceUnderTest`: "A thing ARN, certificate ARN or device role ARN is required", and
    // `DeviceUnderTestList` is len 0..2, so the second entry is the last position.
    rows.push((
        "the only device names nothing".to_string(),
        with_devices("unnamed-only-device", json!([{}])),
        "devices[0]".to_string(),
    ));
    rows.push((
        "the second of two devices names nothing".to_string(),
        with_devices(
            "unnamed-second-device",
            json!([{ "thingArn": thing_arn(1) }, {}]),
        ),
        "devices[1]".to_string(),
    ));
    rows.push((
        "the first of two devices names nothing".to_string(),
        with_devices(
            "unnamed-first-device",
            json!([{}, { "certificateArn": "arn:aws:iot:us-east-1:000000000000:cert/aabbcc" }]),
        ),
        "devices[0]".to_string(),
    ));
    rows.push((
        "both devices name nothing".to_string(),
        with_devices("unnamed-devices", json!([{}, {}])),
        "devices[0]".to_string(),
    ));

    // Each ARN-typed member of a device, against the service that member names.
    for (member, service) in [
        ("thingArn", IOT),
        ("certificateArn", IOT),
        ("deviceRoleArn", IAM),
    ] {
        for (failure, value) in malformed_arns_of(service) {
            rows.push((
                format!("devices[0].{member} is {failure}"),
                with_devices(
                    &format!("bad-{member}"),
                    json!([{ member: value }]),
                ),
                format!("devices[0].{member}"),
            ));
        }
        // The same member at the last list position, beside a device that is named well.
        rows.push((
            format!("devices[1].{member} is not an ARN"),
            with_devices(
                &format!("bad-second-{member}"),
                json!([
                    { "thingArn": thing_arn(2) },
                    { member: format!("{service}-resource-name-not-an-arn") },
                ]),
            ),
            format!("devices[1].{member}"),
        ));
    }

    // A device named well by one member and badly by another: the rule holds for EVERY ARN
    // the device carries, not only for the one that names it.
    rows.push((
        "a valid thingArn beside a deviceRoleArn that is an IoT ARN".to_string(),
        with_devices(
            "bad-role-beside-good-thing",
            json!([{ "thingArn": thing_arn(3), "deviceRoleArn": thing_arn(4) }]),
        ),
        "devices[0].deviceRoleArn".to_string(),
    ));

    // `devicePermissionRoleArn`, documented as the device permission ARN: an IAM role's.
    for (failure, value) in malformed_arns_of(IAM) {
        rows.push((
            format!("devicePermissionRoleArn is {failure}"),
            json!({
                "suiteDefinitionName": "bad-permission-role",
                "rootGroup": "g",
                "devicePermissionRoleArn": value,
                "devices": [{ "thingArn": thing_arn(5) }],
            }),
            "devicePermissionRoleArn".to_string(),
        ));
    }

    rows
}

/// A definition configuration that breaks one of the shape's ARN rules is refused with the
/// modelled `ValidationException` by both operations that accept one, the refusal names the
/// offending member, and nothing is stored: no definition is minted, the live configuration
/// and version of an existing definition are untouched, and no listing reports it.
#[tokio::test]
async fn a_configuration_breaking_an_arn_rule_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    let live = live_configuration();
    let created = emulator.create_definition(&live, REGION).await;
    assert_eq!(
        created.status, 200,
        "setup CreateSuiteDefinition failed with {} {}",
        created.status, created.body
    );
    let target_id = created.string("suiteDefinitionId");
    let definitions_before = stored_definitions(&emulator);

    for (case, configuration, member) in refused_rows() {
        let refused = emulator.create_definition(&configuration, REGION).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException from CreateSuiteDefinition, got \
             {} {:?} {}",
            refused.status,
            refused.error_code,
            refused.body
        );
        let refusal = message(&refused);
        assert!(
            refusal.contains(&member),
            "case {case:?}: the refusal must name the member {member:?}, got {refusal:?}"
        );
        assert!(
            refused.member("suiteDefinitionId").is_null()
                && refused.member("suiteDefinitionArn").is_null(),
            "case {case:?}: a refused CreateSuiteDefinition must mint no definition, got {}",
            refused.body
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
        let refusal = message(&updated);
        assert!(
            refusal.contains(&member),
            "case {case:?}: the update's refusal must name the member {member:?}, got {refusal:?}"
        );

        let definitions_now = stored_definitions(&emulator);
        assert_eq!(
            definitions_now, definitions_before,
            "case {case:?}: a refused call must not change how many definitions are stored, \
             expected {definitions_before}, got {definitions_now}"
        );

        let read = emulator
            .call(
                "GET",
                &format!("/suiteDefinitions/{target_id}"),
                REGION,
                None,
            )
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

        let listed = emulator.call("GET", "/suiteDefinitions", REGION, None).await;
        let information = listed
            .member("suiteDefinitionInformationList")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            information.len(),
            definitions_before,
            "case {case:?}: ListSuiteDefinitions must report only the stored definitions, got {}",
            listed.body
        );
        assert!(
            information
                .iter()
                .flat_map(|entry| entry["defaultDevices"].as_array().cloned().unwrap_or_default())
                .all(|device| device.as_object().is_some_and(|members| !members.is_empty())),
            "case {case:?}: ListSuiteDefinitions must report no device without an identity, got {}",
            listed.body
        );
    }
}

/// The `devicePermissionRoleArn` values the rule ACCEPTS: every form an IAM role ARN takes,
/// so the ARN-form rule refuses only what it must. The published lower length bound of
/// `AmazonResourceName` (20) is one of the rows.
fn accepted_permission_roles() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a role ARN in the default partition", ROLE_ARN),
        (
            // An IAM ARN is a global service's: its region field is empty.
            "a role ARN with a path",
            "arn:aws:iam::000000000000:role/service-role/advisor/DeviceAdvisorRole",
        ),
        (
            "a role ARN in another partition",
            "arn:aws-us-gov:iam::000000000000:role/DeviceAdvisorRole",
        ),
        (
            // Exactly 20 characters: the published minimum length of `AmazonResourceName`.
            "a role ARN at the minimum length",
            "arn:aws:iam:::role/r",
        ),
        (
            "a role ARN carrying a region",
            "arn:aws:iam:us-east-1:000000000000:role/RegionalDeviceAdvisorRole",
        ),
    ]
}

/// A configuration whose ARNs satisfy the rule is accepted and reads back through
/// `GetSuiteDefinition` — on the create path and on the update path alike.
#[tokio::test]
async fn a_configuration_whose_arns_satisfy_the_rule_round_trips() {
    let emulator = Emulator::new();

    for (case, role) in accepted_permission_roles() {
        let configuration = json!({
            "suiteDefinitionName": format!("accepted-{case}"),
            "rootGroup": "g",
            "devicePermissionRoleArn": role,
            "devices": [{ "thingArn": thing_arn(6) }],
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

        // The same value on the update path, which shares the rule.
        let next = json!({
            "suiteDefinitionName": format!("accepted-{case}-updated"),
            "rootGroup": "g",
            "devicePermissionRoleArn": role,
            "devices": [{ "deviceRoleArn": role }],
        });
        let updated = emulator
            .call(
                "PATCH",
                &format!("/suiteDefinitions/{id}"),
                REGION,
                Some(json!({ "suiteDefinitionConfiguration": next })),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateSuiteDefinition, got {} {}",
            updated.status, updated.body
        );
        let read = emulator
            .call("GET", &format!("/suiteDefinitions/{id}"), REGION, None)
            .await;
        assert_eq!(
            read.member("suiteDefinitionConfiguration"),
            &next,
            "case {case:?}: GetSuiteDefinition must answer with the updated configuration, got {}",
            read.body
        );
    }
}

/// The modelled length constraint is refused before the handler's ARN rule runs: an ARN one
/// character below the member's published minimum is a 400, not a stored configuration.
#[tokio::test]
async fn an_arn_below_the_modelled_minimum_length_is_refused() {
    let emulator = Emulator::new();

    // `AmazonResourceName` is len 20..2048; each value here is 19 characters.
    let rows: Vec<(&str, Value)> = vec![
        (
            "devicePermissionRoleArn one character below the minimum",
            json!({
                "suiteDefinitionName": "short-role",
                "rootGroup": "g",
                "devicePermissionRoleArn": "arn:aws:iam::0:role",
                "devices": [{ "thingArn": thing_arn(7) }],
            }),
        ),
        (
            "devices[0].thingArn one character below the minimum",
            with_devices("short-thing", json!([{ "thingArn": "arn:aws:iot:::thin" }])),
        ),
    ];

    for (case, configuration) in rows {
        let refused = emulator.create_definition(&configuration, REGION).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a value outside a modelled constraint must be refused, got {} {}",
            refused.status, refused.body
        );
        let stored = stored_definitions(&emulator);
        assert_eq!(
            stored, 0,
            "case {case:?}: a refused CreateSuiteDefinition must store no definition, the store \
             holds {stored}"
        );
    }
}

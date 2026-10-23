// SPDX-License-Identifier: CC0-1.0

//! Service-linked configuration recorders, end to end over the wire.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — so each
//! expectation is about what `PutServiceLinkedConfigurationRecorder` PERSISTED, read
//! back through the operations that report configuration recorders.
//!
//! The rule under test: `PutServiceLinkedConfigurationRecorder` creates ONE
//! configuration recorder per Amazon Web Services service principal, whose name Config
//! assigns itself from that service principal (the reserved
//! `AWSConfigurationRecorderFor` prefix plus the principal's leading DNS label,
//! upper-camel-cased) and whose configuration is set by the linked service rather than
//! by the caller: it records every supported resource type, continuously, in the
//! `INTERNAL` recording scope, with Config's own service-linked role, recording from
//! creation because the caller cannot start or stop it. A second create for the same
//! service is the modelled `ConflictException`, a value that names no service principal
//! the modelled `ValidationException`, one past the account's service-linked recorder
//! limit the modelled `LimitExceededException`, and the recorder it creates is no
//! customer managed recorder: it does not consume that quota and
//! `StartConfigurationRecorder`, `StopConfigurationRecorder` and
//! `DeleteConfigurationRecorder` do not affect it.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    config_service_linked_role_arn, RECORDING_FREQUENCY_CONTINUOUS,
    RECORDING_SCOPE_INTERNAL, RECORDING_SCOPE_PAID, RECORDING_STRATEGY_ALL_SUPPORTED,
    SERVICE_LINKED_RECORDERS_QUOTA, SERVICE_LINKED_RECORDER_NAME_PREFIX,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The operation under test.
const PUT: &str = "PutServiceLinkedConfigurationRecorder";

/// Service principals and the name Config assigns the service-linked configuration
/// recorder of each: `(case, the service principal, the assigned recorder name)`.
///
/// The rows differ along what the naming rule generalises over — how many labels the
/// principal carries, whether the service's own label is hyphenated, underscored,
/// digit-bearing or already capitalised — and none of them is a principal this issue's
/// recording used. The prefix is the one
/// `smithy_query(query="doc", shape="PutServiceLinkedConfigurationRecorderResponse",
/// member="Name")` publishes: "Config automatically assigns a name that has the prefix
/// \"AWSConfigurationRecorderFor\"".
const NAMING: [(&str, &str, &str); 8] = [
    (
        "a single-label service",
        "ssm.amazonaws.com",
        "AWSConfigurationRecorderForSsm",
    ),
    (
        "a service name carrying a digit",
        "s3.amazonaws.com",
        "AWSConfigurationRecorderForS3",
    ),
    (
        "a hyphenated service name",
        "config-multiaccountsetup.amazonaws.com",
        "AWSConfigurationRecorderForConfigMultiaccountsetup",
    ),
    (
        "several hyphens",
        "a-b-c.amazonaws.com",
        "AWSConfigurationRecorderForABC",
    ),
    (
        "an underscored service name",
        "my_service.amazonaws.com",
        "AWSConfigurationRecorderForMyService",
    ),
    (
        "a principal with a sub-label",
        "azure.partner.amazonaws.com",
        "AWSConfigurationRecorderForAzure",
    ),
    (
        "a regional principal",
        "logs.us-east-1.amazonaws.com",
        "AWSConfigurationRecorderForLogs",
    ),
    (
        "a service name that is already capitalised",
        "SecurityHub.amazonaws.com",
        "AWSConfigurationRecorderForSecurityHub",
    ),
];

/// `key` as a bool, defaulting to false the way the modelled `recording` member does.
fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The `PutServiceLinkedConfigurationRecorder` body for `service_principal`, with the
/// tags the request carries at creation.
fn put_request(service_principal: &str, tags: &[(&str, &str)]) -> Value {
    let mut request = json!({ "ServicePrincipal": service_principal });
    if !tags.is_empty() {
        request["Tags"] = tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect();
    }
    request
}

/// The one recorder `DescribeConfigurationRecorders` reports for `service_principal`.
async fn described(case: &str, emulator: &Emulator, service_principal: &str) -> Value {
    let body = emulator
        .ok(
            "DescribeConfigurationRecorders",
            &json!({ "ServicePrincipal": service_principal }),
        )
        .await;
    let recorders = body["ConfigurationRecorders"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a recorder list, got {body}"))
        .clone();
    assert_eq!(
        recorders.len(),
        1,
        "case {case:?}: expected exactly one recorder linked to {service_principal:?}, \
         got {body}"
    );
    recorders[0].clone()
}

/// The status `DescribeConfigurationRecorderStatus` reports for `service_principal`.
async fn status_of(case: &str, emulator: &Emulator, service_principal: &str) -> Value {
    let body = emulator
        .ok(
            "DescribeConfigurationRecorderStatus",
            &json!({ "ServicePrincipal": service_principal }),
        )
        .await;
    let statuses = body["ConfigurationRecordersStatus"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a status list, got {body}"))
        .clone();
    assert_eq!(
        statuses.len(),
        1,
        "case {case:?}: expected one status for {service_principal:?}, got {body}"
    );
    statuses[0].clone()
}

#[tokio::test]
async fn a_service_linked_recorder_is_named_by_its_service_and_read_back_from_state() {
    for (case, service_principal, want_name) in NAMING {
        let emulator = Emulator::new();
        let created = emulator
            .ok(PUT, &put_request(service_principal, &[("owner", "platform")]))
            .await;

        assert_eq!(
            created["Name"], want_name,
            "case {case:?}: expected the assigned name {want_name:?}, got {created}"
        );
        let name = created["Name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an assigned Name, got {created}"));
        assert!(
            name.starts_with(SERVICE_LINKED_RECORDER_NAME_PREFIX),
            "case {case:?}: the assigned name must carry the reserved prefix \
             {SERVICE_LINKED_RECORDER_NAME_PREFIX:?}, got {name:?}"
        );

        // The ARN is the one Config mints for a configuration recorder of this Region,
        // carrying the assigned name and the server-assigned recorder id.
        let arn = created["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an Arn, got {created}"))
            .to_string();
        let prefix = emulator_server::emulator_arn(
            "config",
            &emulator.region,
            &format!("configuration-recorder/{name}/"),
        );
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected an ARN starting with {prefix:?}, got {arn:?}"
        );
        assert!(
            !arn[prefix.len()..].is_empty(),
            "case {case:?}: expected the ARN to carry the assigned recorder id, got {arn:?}"
        );

        // The recorder reads back as the linked service configured it.
        let recorder = described(case, &emulator, service_principal).await;
        assert_eq!(
            recorder,
            json!({
                "arn": arn,
                "name": name,
                "roleARN": config_service_linked_role_arn(),
                "servicePrincipal": service_principal,
                "recordingScope": RECORDING_SCOPE_INTERNAL,
                "recordingGroup": {
                    "allSupported": true,
                    "includeGlobalResourceTypes": false,
                    "exclusionByResourceTypes": { "resourceTypes": [] },
                    "recordingStrategy": { "useOnly": RECORDING_STRATEGY_ALL_SUPPORTED },
                },
                "recordingMode": {
                    "recordingFrequency": RECORDING_FREQUENCY_CONTINUOUS,
                    "recordingModeOverrides": [],
                },
            }),
            "case {case:?}: expected the recorder the linked service configured, got {recorder}"
        );

        // The same recorder is summarised by ListConfigurationRecorders.
        let listed = emulator
            .ok("ListConfigurationRecorders", &json!({}))
            .await;
        assert_eq!(
            listed["ConfigurationRecorderSummaries"],
            json!([{
                "arn": arn,
                "name": name,
                "servicePrincipal": service_principal,
                "recordingScope": RECORDING_SCOPE_INTERNAL,
            }]),
            "case {case:?}: expected the created recorder summarised from state, got {listed}"
        );

        // Its tags were taken at creation.
        let tags = emulator
            .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            tags["Tags"],
            json!([{ "Key": "owner", "Value": "platform" }]),
            "case {case:?}: expected the tags given at creation, got {tags}"
        );
    }
}

#[tokio::test]
async fn a_service_linked_recorder_records_from_creation_and_settles_on_its_own() {
    // Rows differ along the service the recorder is linked to, so the status is checked
    // on more than the one principal a recording used.
    for (case, service_principal, _) in [NAMING[0], NAMING[5]] {
        let emulator = Emulator::new();
        emulator.ok(PUT, &put_request(service_principal, &[])).await;

        // The linked service turned the recorder on, so it is recording straight away
        // and the recording event it is processing is still Pending.
        let status = status_of(case, &emulator, service_principal).await;
        assert!(
            flag(&status, "recording"),
            "case {case:?}: a service-linked recorder records from creation, got {status}"
        );
        assert_eq!(
            status["lastStatus"], "Pending",
            "case {case:?}: the recording event is Pending straight after creation, \
             got {status}"
        );
        assert!(
            status.get("lastStartTime").is_some(),
            "case {case:?}: the start is stamped, got {status}"
        );

        // The move to Success is owed by the clock, not by the describe.
        tokio::time::sleep(Duration::from_millis(700)).await;
        let settled = status_of(case, &emulator, service_principal).await;
        assert_eq!(
            settled["lastStatus"], "Success",
            "case {case:?}: the recording event has been processed, got {settled}"
        );
        assert!(
            flag(&settled, "recording"),
            "case {case:?}: the recorder is still recording, got {settled}"
        );

        // Reporting the recorder twice reports the same recorder: a read changes
        // nothing a later caller sees.
        let first = described(case, &emulator, service_principal).await;
        let again = described(case, &emulator, service_principal).await;
        assert_eq!(
            again, first,
            "case {case:?}: a second describe must report the same recorder, got {again}"
        );
    }
}

#[tokio::test]
async fn a_second_recorder_for_the_same_service_is_a_conflict() {
    // Rows differ along the second request: the same principal again, the same request
    // with different tags (tags are taken at creation, so this is no update either),
    // and the same service named with a sub-labelled principal of its own.
    type ConflictCase = (&'static str, &'static str, &'static [(&'static str, &'static str)]);
    let cases: [ConflictCase; 3] = [
        ("the same request again", NAMING[0].1, &[]),
        ("the same service with other tags", NAMING[0].1, &[("tier", "gold")]),
        ("a sub-labelled principal of the same service", NAMING[5].1, &[]),
    ];

    for (case, service_principal, tags) in cases {
        let emulator = Emulator::new();
        let created = emulator.ok(PUT, &put_request(service_principal, &[])).await;
        let before = described(case, &emulator, service_principal).await;

        let (status, code) = emulator
            .refused(PUT, &put_request(service_principal, tags))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ConflictException"),
            "case {case:?}: expected 400 ConflictException for a service that already has \
             a service-linked recorder, got {status} {code}"
        );

        // The refusal left the recorder it already had alone, ARN and all.
        let after = described(case, &emulator, service_principal).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused create must leave the recorder alone, got {after}"
        );
        assert_eq!(
            after["arn"], created["Arn"],
            "case {case:?}: the recorder keeps the ARN it was created with, got {after}"
        );
    }
}

#[tokio::test]
async fn a_value_that_names_no_service_principal_is_refused() {
    // Rows differ along the form of the value: the modelled pattern `^[\w+=,.@-]+$`
    // (smithy_query(query="operation",
    // operation="PutServiceLinkedConfigurationRecorder")) admits all of them, and none
    // of them names an Amazon Web Services service.
    let cases: [(&str, &str); 8] = [
        ("no DNS suffix at all", "ssm"),
        ("another suffix", "ssm.amazon.com"),
        ("the suffix on its own", "amazonaws.com"),
        ("an empty service label", ".amazonaws.com"),
        ("an empty inner label", "ssm..amazonaws.com"),
        ("an IAM-style principal", "config@amazonaws.com"),
        ("characters the pattern admits but a label does not", "ssm+logs.amazonaws.com"),
        ("a hyphen-bounded label", "-ssm.amazonaws.com"),
    ];

    for (case, service_principal) in cases {
        let emulator = Emulator::new();
        let (status, code) = emulator.refused(PUT, &put_request(service_principal, &[])).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?} ({service_principal:?}): expected 400 ValidationException, \
             got {status} {code}"
        );

        // A refused create persists nothing.
        let body = emulator
            .ok("DescribeConfigurationRecorders", &json!({}))
            .await;
        assert_eq!(
            body["ConfigurationRecorders"],
            json!([]),
            "case {case:?}: the refused create must persist nothing, got {body}"
        );
    }
}

#[tokio::test]
async fn the_account_holds_no_more_service_linked_recorders_than_its_quota() {
    let emulator = Emulator::new();

    // Up to the quota, each distinct service gets its own recorder. The principals are
    // generated rather than listed: what the rule generalises over is HOW MANY the
    // account holds, not which services they are.
    for index in 0..SERVICE_LINKED_RECORDERS_QUOTA {
        let service_principal = format!("svc{index}.amazonaws.com");
        let created = emulator.ok(PUT, &put_request(&service_principal, &[])).await;
        assert_eq!(
            created["Name"],
            json!(format!("{SERVICE_LINKED_RECORDER_NAME_PREFIX}Svc{index}")),
            "case \"recorder {index}\": expected the name assigned to \
             {service_principal:?}, got {created}"
        );
    }

    let at_quota = emulator
        .ok("ListConfigurationRecorders", &json!({}))
        .await;
    assert_eq!(
        at_quota["ConfigurationRecorderSummaries"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        SERVICE_LINKED_RECORDERS_QUOTA,
        "case \"at the quota\": expected {SERVICE_LINKED_RECORDERS_QUOTA} recorders, \
         got {at_quota}"
    );

    // One past it, for a service that has no recorder yet, is refused.
    let past = format!("svc{SERVICE_LINKED_RECORDERS_QUOTA}.amazonaws.com");
    let (status, code) = emulator.refused(PUT, &put_request(&past, &[])).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "case \"one past the quota\": expected 400 LimitExceededException, got {status} {code}"
    );

    // The refusal persisted nothing: the account still holds exactly its quota, and the
    // recorders it already had are unchanged.
    let after = emulator
        .ok("ListConfigurationRecorders", &json!({}))
        .await;
    assert_eq!(
        after["ConfigurationRecorderSummaries"], at_quota["ConfigurationRecorderSummaries"],
        "case \"one past the quota\": the refused create must leave the store alone, \
         got {after}"
    );

    // The customer managed recorder is a separate quota, so it is still creatable.
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
}

#[tokio::test]
async fn service_linked_recorders_are_no_customer_managed_recorder() {
    let emulator = Emulator::new();
    // Two services, each with its own recorder, created before the account has any
    // recorder of its own.
    for (case, service_principal, want_name) in [NAMING[1], NAMING[2]] {
        let created = emulator.ok(PUT, &put_request(service_principal, &[])).await;
        assert_eq!(
            created["Name"], want_name,
            "case {case:?}: expected {want_name:?}, got {created}"
        );
    }

    // The customer managed quota counts the account's OWN recorders, so the one it is
    // allowed is still creatable, in the PAID scope and with the role the request named.
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    let described = emulator
        .ok("DescribeConfigurationRecorders", &json!({}))
        .await;
    let recorders = described["ConfigurationRecorders"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a recorder list, got {described}"))
        .clone();
    assert_eq!(
        recorders.len(),
        3,
        "two service-linked recorders and the account's own must coexist, got {described}"
    );
    let own: Vec<&Value> = recorders
        .iter()
        .filter(|recorder| recorder.get("servicePrincipal").is_none())
        .collect();
    assert_eq!(
        own.len(),
        1,
        "exactly one recorder is customer managed, got {described}"
    );
    assert_eq!(
        own[0]["recordingScope"], RECORDING_SCOPE_PAID,
        "the customer managed recorder records in the PAID scope, got {described}"
    );
    assert_eq!(
        own[0]["roleARN"], ROLE_ARN,
        "the customer managed recorder keeps the role the request named, got {described}"
    );

    // A second customer managed recorder is still refused: the quota is on the
    // account's own recorders, not on the store.
    let (status, code) = emulator
        .refused(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "name": "second", "roleARN": ROLE_ARN })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "MaxNumberOfConfigurationRecordersExceededException"),
        "a second customer managed recorder must be refused, got {status} {code}"
    );

    // The caller's recorder operations do not affect a service-linked recorder.
    for (case, service_principal, name) in [NAMING[1], NAMING[2]] {
        for operation in [
            "StartConfigurationRecorder",
            "StopConfigurationRecorder",
            "DeleteConfigurationRecorder",
        ] {
            let (status, code) = emulator
                .refused(operation, &json!({ "ConfigurationRecorderName": name }))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "UnmodifiableEntityException"),
                "case {case:?}: {operation} must not affect a service-linked recorder, \
                 got {status} {code}"
            );
        }
        // Every refusal left the recorder recording.
        let status = status_of(case, &emulator, service_principal).await;
        assert!(
            flag(&status, "recording"),
            "case {case:?}: the service-linked recorder is still recording, got {status}"
        );
    }
}

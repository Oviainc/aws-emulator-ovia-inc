// SPDX-License-Identifier: CC0-1.0

//! What a configuration check RUN decides, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` to place a record: every rule result under test is one a
//! `StartConfigurationChecks` call actually produced.
//!
//! RULE under test: a configuration check run evaluates the rules its check's catalogue
//! entry names against the registered application, and when the clock brings it to its
//! terminal status it reports `SUCCESS` with nothing to explain, the catalogue's own
//! `Name` and `Description` for that check — the same text
//! `ListConfigurationCheckDefinitions` reports for it — and `RuleStatusCounts` that are
//! the tally, per `RuleResultStatus`, of the rule results it produced; each of those
//! results is decided by the registered configuration alone, so the same rule passes for
//! one registration and fails, warns, informs or stays unknown for another.
//!
//! The rows span what the rule generalises over: both modelled `ApplicationType` values,
//! a registration that declares its System ID and instance number and one that declares
//! neither, databases named by credential and by `DatabaseArn`, a system database present
//! and absent, a database with credentials and one without, no component, a component on
//! the registered instance, and a component placed on another instance and under another
//! System ID. The rules and the statuses each row expects are named through
//! `state::ConfigurationRule`, so a rule added to the catalogue is covered here rather
//! than silently untested.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::CONFIGURATION_CHECK_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    configuration_check_entry, ConfigurationRule, RULE_RESULT_STATUS_FAILED,
    RULE_RESULT_STATUS_INFO, RULE_RESULT_STATUS_PASSED, RULE_RESULT_STATUS_UNKNOWN,
    RULE_RESULT_STATUS_WARNING,
};

use crate::application_lifecycle::{Emulator, Reply};
use crate::configuration_checks::{field, read_record, settle, started, string_field};

/// The EC2 instance the rows register their application against, and another the rows
/// that misplace a component point at. Both are the model's `InstanceId` form
/// (`^i-[\w\d]{8}$|^i-[\w\d]{17}$`).
const REGISTERED_INSTANCE: &str = "i-0123456789abcdef0";
const ANOTHER_INSTANCE: &str = "i-0fedcba987654321f";

/// The largest `Credentials` list `RegisterApplication` admits
/// (`smithy_query(query="operation", operation="RegisterApplication")`: length 0..20) and
/// the longest `DatabaseName` an `ApplicationCredential` admits
/// (`smithy_query(query="shape", shape="ApplicationCredential")`: length 1..100) — the
/// published bounds the widest row is built on, so the rules are exercised on a
/// registration naming more databases than one `RuleResultMetadata` value can hold.
const MAX_CREDENTIALS: usize = 20;
const MAX_DATABASE_NAME_LENGTH: usize = 100;

/// The longest `RuleResultMetadata` key and value the model admits
/// (`smithy_query(query="shape", shape="RuleResultMetadataValue")`: length 1..1000, and
/// `RuleResultMetadataKey`: length 1..100).
const MAX_METADATA_KEY_LENGTH: usize = 100;
const MAX_METADATA_VALUE_LENGTH: usize = 1000;

/// One row: a label, the `RegisterApplication` body it registers, and the status each
/// rule of the catalogue must report for it.
struct RuleCase {
    label: &'static str,
    registration: Value,
    expected: Vec<(ConfigurationRule, &'static str)>,
}

/// `MAX_CREDENTIALS` credentials, each naming its own tenant database by a name of the
/// longest length the model admits, so the widest row's databases together exceed what one
/// metadata value holds.
fn widest_credential_list() -> Value {
    Value::Array(
        (0..MAX_CREDENTIALS)
            .map(|n| {
                let name = format!("TENANT{n:0>width$}", width = MAX_DATABASE_NAME_LENGTH - 6);
                json!({
                    "DatabaseName": name,
                    "CredentialType": "ADMIN",
                    "SecretId": format!("sap/hdb/admin-{n}"),
                })
            })
            .collect(),
    )
}

/// The rows. Each one's `expected` names every rule of the catalogue, so a rule the
/// catalogue gains makes [`every_catalogued_rule_is_covered_by_a_row`] fail rather than
/// going untested.
fn rule_cases() -> Vec<RuleCase> {
    vec![
        RuleCase {
            label: "HANA, one tenant-database credential, no ComponentsInfo",
            registration: json!({
                "ApplicationId": "HDB-TENANT-ONLY",
                "ApplicationType": "HANA",
                "Instances": [REGISTERED_INSTANCE],
                "Sid": "HDB",
                "SapInstanceNumber": "00",
                "Credentials": [{
                    "DatabaseName": "HDB",
                    "CredentialType": "ADMIN",
                    "SecretId": "sap/hdb/admin",
                }],
            }),
            expected: vec![
                (ConfigurationRule::SystemIdDeclared, RULE_RESULT_STATUS_PASSED),
                (
                    ConfigurationRule::SapInstanceNumberDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::RegisteredOnEc2Instance,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::HostOperatingSystemSupported,
                    RULE_RESULT_STATUS_UNKNOWN,
                ),
                (
                    ConfigurationRule::HanaDatabaseRegistered,
                    RULE_RESULT_STATUS_PASSED,
                ),
                // Only a tenant database is known, so the system database is not.
                (
                    ConfigurationRule::SystemDatabaseRegistered,
                    RULE_RESULT_STATUS_WARNING,
                ),
                (
                    ConfigurationRule::DatabaseCredentialsPresent,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::CredentialSecretsNamed,
                    RULE_RESULT_STATUS_PASSED,
                ),
                // The registration declares no `ComponentsInfo`, but naming a System ID
                // and an instance describes the system's SAP HANA component, so the
                // service does hold one and it sits on the registered instance.
                (
                    ConfigurationRule::ComponentsDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsOnRegisteredInstances,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsCarrySystemId,
                    RULE_RESULT_STATUS_PASSED,
                ),
            ],
        },
        RuleCase {
            label: "HANA, system and tenant databases, a component on its own instance",
            registration: json!({
                "ApplicationId": "HDB-COMPLETE",
                "ApplicationType": "HANA",
                "Instances": [REGISTERED_INSTANCE],
                "Sid": "HDB",
                "SapInstanceNumber": "00",
                "Credentials": [
                    {
                        "DatabaseName": "SYSTEMDB",
                        "CredentialType": "ADMIN",
                        "SecretId": "sap/hdb/system",
                    },
                    {
                        "DatabaseName": "HDB",
                        "CredentialType": "ADMIN",
                        "SecretId": "sap/hdb/admin",
                    },
                ],
                "ComponentsInfo": [{
                    "ComponentType": "HANA",
                    "Sid": "HDB",
                    "Ec2InstanceId": REGISTERED_INSTANCE,
                }],
            }),
            expected: vec![
                (ConfigurationRule::SystemIdDeclared, RULE_RESULT_STATUS_PASSED),
                (
                    ConfigurationRule::SapInstanceNumberDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::RegisteredOnEc2Instance,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::HostOperatingSystemSupported,
                    RULE_RESULT_STATUS_UNKNOWN,
                ),
                (
                    ConfigurationRule::HanaDatabaseRegistered,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::SystemDatabaseRegistered,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::DatabaseCredentialsPresent,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::CredentialSecretsNamed,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsOnRegisteredInstances,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsCarrySystemId,
                    RULE_RESULT_STATUS_PASSED,
                ),
            ],
        },
        RuleCase {
            label: "HANA, a component on another instance and under another System ID",
            registration: json!({
                "ApplicationId": "HDB-MISPLACED",
                "ApplicationType": "HANA",
                "Instances": [REGISTERED_INSTANCE],
                "Sid": "HDB",
                "SapInstanceNumber": "00",
                "Credentials": [{
                    "DatabaseName": "SYSTEMDB",
                    "CredentialType": "ADMIN",
                    "SecretId": "sap/hdb/system",
                }],
                "ComponentsInfo": [{
                    "ComponentType": "HANA",
                    "Sid": "ABC",
                    "Ec2InstanceId": ANOTHER_INSTANCE,
                }],
            }),
            expected: vec![
                (ConfigurationRule::SystemIdDeclared, RULE_RESULT_STATUS_PASSED),
                (
                    ConfigurationRule::SapInstanceNumberDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::RegisteredOnEc2Instance,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::HostOperatingSystemSupported,
                    RULE_RESULT_STATUS_UNKNOWN,
                ),
                (
                    ConfigurationRule::HanaDatabaseRegistered,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::SystemDatabaseRegistered,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::DatabaseCredentialsPresent,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::CredentialSecretsNamed,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsOnRegisteredInstances,
                    RULE_RESULT_STATUS_FAILED,
                ),
                (
                    ConfigurationRule::ComponentsCarrySystemId,
                    RULE_RESULT_STATUS_FAILED,
                ),
            ],
        },
        RuleCase {
            label: "HANA, a database named by DatabaseArn that no credential reaches",
            registration: json!({
                "ApplicationId": "HDB-BY-ARN",
                "ApplicationType": "HANA",
                "Instances": [REGISTERED_INSTANCE],
                "Sid": "HDB",
                "SapInstanceNumber": "00",
                "Credentials": [{
                    "DatabaseName": "HDB",
                    "CredentialType": "ADMIN",
                    "SecretId": "sap/hdb/admin",
                }],
                // The last segment of a database ARN is the database's own name, so this
                // names the system database of the HANA system.
                "DatabaseArn":
                    "arn:aws:ssm-sap:us-east-1:123456789012:database/HDB-BY-ARN/SYSTEMDB",
            }),
            expected: vec![
                (ConfigurationRule::SystemIdDeclared, RULE_RESULT_STATUS_PASSED),
                (
                    ConfigurationRule::SapInstanceNumberDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::RegisteredOnEc2Instance,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::HostOperatingSystemSupported,
                    RULE_RESULT_STATUS_UNKNOWN,
                ),
                (
                    ConfigurationRule::HanaDatabaseRegistered,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::SystemDatabaseRegistered,
                    RULE_RESULT_STATUS_PASSED,
                ),
                // The database the ARN named has no credential of its own.
                (
                    ConfigurationRule::DatabaseCredentialsPresent,
                    RULE_RESULT_STATUS_WARNING,
                ),
                (
                    ConfigurationRule::CredentialSecretsNamed,
                    RULE_RESULT_STATUS_PASSED,
                ),
                // Naming a System ID and an instance describes the system's SAP HANA
                // component, so this registration has one even without `ComponentsInfo`.
                (
                    ConfigurationRule::ComponentsDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsOnRegisteredInstances,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsCarrySystemId,
                    RULE_RESULT_STATUS_PASSED,
                ),
            ],
        },
        RuleCase {
            label: "SAP_ABAP, the required members only",
            registration: json!({
                "ApplicationId": "ABAP-MINIMAL",
                "ApplicationType": "SAP_ABAP",
                "Instances": [REGISTERED_INSTANCE],
            }),
            expected: vec![
                (ConfigurationRule::SystemIdDeclared, RULE_RESULT_STATUS_FAILED),
                (
                    ConfigurationRule::SapInstanceNumberDeclared,
                    RULE_RESULT_STATUS_FAILED,
                ),
                (
                    ConfigurationRule::RegisteredOnEc2Instance,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::HostOperatingSystemSupported,
                    RULE_RESULT_STATUS_UNKNOWN,
                ),
                // An application of a type that is not a HANA database and names none
                // has no database for these rules to judge.
                (
                    ConfigurationRule::HanaDatabaseRegistered,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::SystemDatabaseRegistered,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::DatabaseCredentialsPresent,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::CredentialSecretsNamed,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::ComponentsDeclared,
                    RULE_RESULT_STATUS_WARNING,
                ),
                (
                    ConfigurationRule::ComponentsOnRegisteredInstances,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::ComponentsCarrySystemId,
                    RULE_RESULT_STATUS_INFO,
                ),
            ],
        },
        RuleCase {
            label: "SAP_ABAP, a component but no System ID to compare it with",
            registration: json!({
                "ApplicationId": "ABAP-NO-SID",
                "ApplicationType": "SAP_ABAP",
                "Instances": [REGISTERED_INSTANCE],
                "ComponentsInfo": [{
                    "ComponentType": "ABAP",
                    "Sid": "ABC",
                    "Ec2InstanceId": REGISTERED_INSTANCE,
                }],
            }),
            expected: vec![
                (ConfigurationRule::SystemIdDeclared, RULE_RESULT_STATUS_FAILED),
                (
                    ConfigurationRule::SapInstanceNumberDeclared,
                    RULE_RESULT_STATUS_FAILED,
                ),
                (
                    ConfigurationRule::RegisteredOnEc2Instance,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::HostOperatingSystemSupported,
                    RULE_RESULT_STATUS_UNKNOWN,
                ),
                (
                    ConfigurationRule::HanaDatabaseRegistered,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::SystemDatabaseRegistered,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::DatabaseCredentialsPresent,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::CredentialSecretsNamed,
                    RULE_RESULT_STATUS_INFO,
                ),
                (
                    ConfigurationRule::ComponentsDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsOnRegisteredInstances,
                    RULE_RESULT_STATUS_PASSED,
                ),
                // Nothing to compare the component's System ID with.
                (
                    ConfigurationRule::ComponentsCarrySystemId,
                    RULE_RESULT_STATUS_UNKNOWN,
                ),
            ],
        },
        RuleCase {
            label: "HANA, the largest credential list the model admits",
            registration: json!({
                "ApplicationId": "HDB-WIDEST",
                "ApplicationType": "HANA",
                "Instances": [REGISTERED_INSTANCE],
                "Sid": "HDB",
                "SapInstanceNumber": "00",
                "Credentials": widest_credential_list(),
                "ComponentsInfo": [{
                    "ComponentType": "HANA",
                    "Sid": "HDB",
                    "Ec2InstanceId": REGISTERED_INSTANCE,
                }],
            }),
            expected: vec![
                (ConfigurationRule::SystemIdDeclared, RULE_RESULT_STATUS_PASSED),
                (
                    ConfigurationRule::SapInstanceNumberDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::RegisteredOnEc2Instance,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::HostOperatingSystemSupported,
                    RULE_RESULT_STATUS_UNKNOWN,
                ),
                (
                    ConfigurationRule::HanaDatabaseRegistered,
                    RULE_RESULT_STATUS_PASSED,
                ),
                // Every one of them is a tenant database.
                (
                    ConfigurationRule::SystemDatabaseRegistered,
                    RULE_RESULT_STATUS_WARNING,
                ),
                (
                    ConfigurationRule::DatabaseCredentialsPresent,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::CredentialSecretsNamed,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsDeclared,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsOnRegisteredInstances,
                    RULE_RESULT_STATUS_PASSED,
                ),
                (
                    ConfigurationRule::ComponentsCarrySystemId,
                    RULE_RESULT_STATUS_PASSED,
                ),
            ],
        },
    ]
}

impl Emulator {
    async fn list_sub_checks(&self, operation_id: &str) -> Reply {
        self.send(
            "/list-sub-check-results",
            json!({"OperationId": operation_id}),
        )
        .await
    }

    async fn list_rules(&self, sub_check_result_id: &str) -> Reply {
        self.send(
            "/list-sub-check-rule-results",
            json!({"SubCheckResultId": sub_check_result_id}),
        )
        .await
    }

    /// The rule results of the run `operation_id` names, keyed by their `Description` —
    /// which is what tells one rule's result from another's — walked through the two
    /// result listings over the wire.
    async fn rule_results_by_description(&self, operation_id: &str) -> BTreeMap<String, Value> {
        let sub_checks = self.list_sub_checks(operation_id).await;
        assert_eq!(
            sub_checks.status, 200,
            "expected 200 from ListSubCheckResults, got {sub_checks:?}"
        );
        let mut results: BTreeMap<String, Value> = BTreeMap::new();
        for sub_check in sub_checks
            .member("SubCheckResults")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let id = string_field(&sub_check, "Id");
            let rules = self.list_rules(&id).await;
            assert_eq!(
                rules.status, 200,
                "expected 200 from ListSubCheckRuleResults for {id:?}, got {rules:?}"
            );
            for rule in rules
                .member("RuleResults")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let description = string_field(&rule, "Description");
                assert_within_modelled_bounds(&description, &rule);
                assert!(
                    results.insert(description.clone(), rule).is_none(),
                    "a run must report one result per rule, got two for {description:?}"
                );
            }
        }
        results
    }
}

/// What every rule result must satisfy whatever the row: it reports the status and the
/// message of a rule that was evaluated, and its metadata stays inside the lengths the
/// model admits, which the widest row's databases together exceed.
fn assert_within_modelled_bounds(case: &str, rule: &Value) {
    assert!(
        !string_field(rule, "Status").is_empty() && !string_field(rule, "Message").is_empty(),
        "case {case:?}: a rule result reports the status it reached and why, got {rule:?}"
    );
    let metadata = field(rule, "Metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (key, value) in &metadata {
        let value = value.as_str().unwrap_or_default();
        assert!(
            (1..=MAX_METADATA_KEY_LENGTH).contains(&key.chars().count()),
            "case {case:?}: metadata key {key:?} must be within the modelled length, got {} \
             characters",
            key.chars().count()
        );
        assert!(
            (1..=MAX_METADATA_VALUE_LENGTH).contains(&value.chars().count()),
            "case {case:?}: metadata value of {key:?} must be within the modelled length, got \
             {} characters",
            value.chars().count()
        );
    }
}

/// The `RuleStatusCounts` of a record, bucket by bucket, as the response reports them.
fn counts(record: &Value) -> BTreeMap<String, i64> {
    let reported = field(record, "RuleStatusCounts")
        .unwrap_or_else(|| panic!("expected RuleStatusCounts on a finished run, got {record:?}"));
    ["Passed", "Failed", "Warning", "Info", "Unknown"]
        .iter()
        .map(|bucket| {
            (
                (*bucket).to_string(),
                reported
                    .get(bucket)
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| {
                        panic!("expected a {bucket} count, got {reported:?}")
                    }),
            )
        })
        .collect()
}

/// The bucket of `RuleStatusCounts` a `RuleResultStatus` is counted in.
fn bucket_of(status: &str) -> &'static str {
    match status {
        RULE_RESULT_STATUS_PASSED => "Passed",
        RULE_RESULT_STATUS_FAILED => "Failed",
        RULE_RESULT_STATUS_WARNING => "Warning",
        RULE_RESULT_STATUS_INFO => "Info",
        _ => "Unknown",
    }
}

/// RULE: every rule of a check's catalogue entry is evaluated against the registered
/// application, and each result's status is the one that application's own configuration
/// warrants — over a table of registrations that differ in what they declare.
#[tokio::test]
async fn a_run_decides_each_rule_from_the_registered_configuration() {
    for case in rule_cases() {
        let RuleCase {
            label,
            registration,
            expected,
        } = case;
        // One emulator per row: the rows are independent registrations, not a sequence.
        let emulator = Emulator::new();
        let application_id = string_field(&registration, "ApplicationId");
        let registered = emulator.send("/register-application", registration).await;
        assert_eq!(
            registered.status, 200,
            "case {label:?}: expected 200 from RegisterApplication, got {}: {:?}",
            registered.status, registered.body
        );
        settle().await;

        // The whole catalogue in one call, so every rule of every check runs for the row.
        let start = emulator
            .start_checks(json!({"ApplicationId": application_id}))
            .await;
        assert_eq!(
            start.status, 200,
            "case {label:?}: expected 200 from StartConfigurationChecks, got {}: {:?}",
            start.status, start.body
        );
        let operations = started(&start);
        settle().await;

        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        for operation in &operations {
            let operation_id = string_field(operation, "Id");
            let check = string_field(operation, "ConfigurationCheckId");
            let case = format!("{label} / {check}");
            let record = read_record(&emulator.get_check(&operation_id).await);
            assert_eq!(
                string_field(&record, "Status"),
                "SUCCESS",
                "case {case:?}: a run whose rules were evaluated ends in SUCCESS, got {record:?}"
            );
            let results = emulator.rule_results_by_description(&operation_id).await;
            for (description, result) in &results {
                seen.insert(description.clone(), string_field(result, "Status"));
            }

            // The counts the record reports are the tally of exactly those results.
            let reported = counts(&record);
            let mut tallied: BTreeMap<String, i64> = reported
                .keys()
                .map(|bucket| (bucket.clone(), 0))
                .collect();
            for result in results.values() {
                let bucket = bucket_of(&string_field(result, "Status"));
                *tallied
                    .get_mut(bucket)
                    .expect("every bucket is initialised above") += 1;
            }
            assert_eq!(
                reported, tallied,
                "case {case:?}: RuleStatusCounts must tally the rule results the run \
                 reported, got {reported:?} for {results:?}"
            );
        }

        // And each rule decided what the registration warrants.
        for (rule, status) in &expected {
            let description = rule.description().to_string();
            let actual = seen.get(&description).unwrap_or_else(|| {
                panic!("case {label:?}: no run reported a result for {rule:?}, got {seen:?}")
            });
            assert_eq!(
                actual, status,
                "case {label:?}: expected {rule:?} to be {status}, got {actual:?}"
            );
        }
    }
}

/// What the service's catalogue states for each modelled check, written out HERE rather
/// than read from `state::CONFIGURATION_CHECK_CATALOGUE`: an independent expectation, so
/// a row fails if the prose a response carries changes, instead of re-asserting the
/// literal the handler projects from. The catalogue-versus-listing equality is the
/// separate consistency check below.
///
/// The text is the SERVICE's own published text for each `ConfigurationCheckType`, which
/// is the same in every account and Region.
const EXPECTED_PROSE: &[(&str, &str, &str)] = &[
    (
        "SAP_CHECK_01",
        "SAP EC2 Instance Type Selection",
        "Checks any EC2 Instances Associated with this SAP Application, and evaluates whether \
         the Instance Type complies with SAP Certification requirements and that necessary \
         hardware settings are in place.",
    ),
    (
        "SAP_CHECK_02",
        "SAP EBS Storage Configuration",
        "Checks that the EBS Storage Configuration on Amazon EC2 Instances is appropriate for \
         the SAP workload.",
    ),
    (
        "SAP_CHECK_03",
        "SAP on AWS Pacemaker Configuration",
        "Checks whether the Pacemaker Configuration for the SAP Application follows best \
         practices.",
    ),
];

/// RULE: the `Name` and `Description` a run reports for its check are the catalogue's, so
/// they are the SAME text `ListConfigurationCheckDefinitions` reports for that
/// `ConfigurationCheckType` — in both phases of the run, since the check a run performs
/// is named before it finishes.
#[tokio::test]
async fn a_run_names_its_check_with_the_same_prose_the_catalogue_listing_reports() {
    let emulator = Emulator::new();
    let application_id = "HDB-CHECK-PROSE";
    emulator.with_application(application_id).await;

    let definitions = emulator
        .send("/list-configuration-check-definitions", json!({}))
        .await;
    assert_eq!(
        definitions.status, 200,
        "expected 200 from ListConfigurationCheckDefinitions, got {definitions:?}"
    );
    let catalogue: BTreeMap<String, (String, String)> = definitions
        .member("ConfigurationChecks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|definition| {
            (
                string_field(definition, "Id"),
                (
                    string_field(definition, "Name"),
                    string_field(definition, "Description"),
                ),
            )
        })
        .collect();
    assert_eq!(
        catalogue.keys().cloned().collect::<BTreeSet<String>>(),
        CONFIGURATION_CHECK_TYPE_VALUES
            .iter()
            .map(|check| (*check).to_string())
            .collect::<BTreeSet<String>>(),
        "expected a definition per modelled check, got {catalogue:?}"
    );
    // The prose itself, against the expectation this suite states independently.
    for (check, name, description) in EXPECTED_PROSE {
        let reported = catalogue
            .get(*check)
            .unwrap_or_else(|| panic!("case {check:?}: the listing reported this check"));
        assert_eq!(
            (reported.0.as_str(), reported.1.as_str()),
            (*name, *description),
            "case {check:?}: expected the catalogue's own name and description, got {reported:?}"
        );
    }

    let start = emulator
        .start_checks(json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(start.status, 200, "expected 200, got {start:?}");
    let operations = started(&start);

    // The start's own projection, while every run is still INPROGRESS.
    for operation in &operations {
        let check = string_field(operation, "ConfigurationCheckId");
        let expected = catalogue
            .get(&check)
            .unwrap_or_else(|| panic!("case {check:?}: the listing reported this check"));
        assert_eq!(
            (
                string_field(operation, "ConfigurationCheckName"),
                string_field(operation, "ConfigurationCheckDescription")
            ),
            expected.clone(),
            "case {check:?} (running): a started run names its check with the catalogue's \
             prose, got {operation:?}"
        );
    }

    // And the reader's, once the clock has ended every run.
    settle().await;
    for operation in &operations {
        let check = string_field(operation, "ConfigurationCheckId");
        let record = read_record(&emulator.get_check(&string_field(operation, "Id")).await);
        let expected = catalogue
            .get(&check)
            .unwrap_or_else(|| panic!("case {check:?}: the listing reported this check"));
        assert_eq!(
            (
                string_field(&record, "ConfigurationCheckName"),
                string_field(&record, "ConfigurationCheckDescription")
            ),
            expected.clone(),
            "case {check:?} (ended): a finished run names its check with the catalogue's \
             prose, got {record:?}"
        );
        assert!(
            !expected.0.is_empty() && !expected.1.is_empty(),
            "case {check:?}: the catalogue must state a name and a description, got {expected:?}"
        );
    }
}

/// The rows together must cover every rule of every catalogued check: a rule no row names
/// a status for would otherwise be evaluated by the emulator and asserted by nothing.
#[test]
fn every_catalogued_rule_is_covered_by_a_row() {
    let catalogued: BTreeSet<ConfigurationRule> = CONFIGURATION_CHECK_TYPE_VALUES
        .iter()
        .flat_map(|check| {
            configuration_check_entry(check)
                .unwrap_or_else(|| panic!("the service catalogues {check:?}"))
                .sub_checks
                .iter()
                .flat_map(|sub_check| sub_check.rules.iter().copied())
        })
        .collect();
    for case in rule_cases() {
        let covered: BTreeSet<ConfigurationRule> =
            case.expected.iter().map(|(rule, _)| *rule).collect();
        assert_eq!(
            covered,
            catalogued,
            "case {:?}: every row must name a status for every catalogued rule",
            case.label
        );
    }
}

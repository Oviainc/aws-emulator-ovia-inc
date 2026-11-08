// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeStateMachineForExecution`, driven over this
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rule under test: the operation reports, for the execution its `executionArn`
//! names, the state machine CONFIGURATION that execution RAN — the immutable
//! version snapshot when the start resolved to a published version (a version ARN,
//! or an alias routing to one), the state machine's own record otherwise — under the
//! unqualified ARN of the state machine the execution belongs to, with `updateDate`
//! the instant that configuration was last written and `definition` (and everything
//! read out of it) as `includedData` asks for it. An `executionArn` no record of this
//! endpoint's region answers for is the operation's own modelled refusal, and the
//! call is a REPORT: it changes nothing a later caller sees.
//!
//! Each table below spans that rule rather than one recorded request: the rows differ
//! along how the start resolved its state machine (unqualified, version, alias),
//! which configuration structures the create supplied, the form the definition's
//! variable references take, and which part of the `executionArn` makes it
//! unanswerable.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`,
//! `error_code`) is the one `state_machine_lifecycle` builds; `tests/all.rs` is this
//! binary's crate root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    INCLUDED_DATA_ALL_DATA, INCLUDED_DATA_METADATA_ONLY, METADATA_ONLY_DEFINITION,
};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, discriminated, error_code, start_execution, string,
    Emulator, ACCOUNT, HELLO_DEFINITION, PAST_THE_DELETE_DELAY, PAST_THE_EXECUTION_DELAY, REGION,
    ROLE_ARN,
};

/// A second role ARN, so an update that replaces the `roleArn` makes the current
/// record differ from the version snapshots published before it.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OtherStepFunctionsRole";

/// The members `DescribeStateMachine` and `DescribeStateMachineForExecution` both
/// report for one state machine configuration. Both readers answer from the SAME
/// stored record, so for an execution that ran the configuration `DescribeStateMachine`
/// is asked about, every one of these must agree — which is what makes
/// `DescribeStateMachine` the reference this suite compares against instead of a
/// hand-written expectation.
const SHARED_MEMBERS: &[&str] = &[
    "name",
    "definition",
    "roleArn",
    "revisionId",
    "loggingConfiguration",
    "tracingConfiguration",
    "encryptionConfiguration",
    "variableReferences",
];

/// A definition whose single `Pass` state computes `version`, so two snapshots of
/// one state machine differ in their `definition` text.
fn definition(version: i64) -> String {
    json!({
        "StartAt": "Only",
        "States": { "Only": { "Type": "Pass", "Result": { "version": version }, "End": true } },
    })
    .to_string()
}

/// A JSONata definition whose two states read variables: `Compose` reads the two it
/// composes, `Emit` reads what `Compose` assigned and one it did not.
const JSONATA_DEFINITION: &str = r#"{
  "QueryLanguage": "JSONata",
  "StartAt": "Compose",
  "States": {
    "Compose": {
      "Type": "Pass",
      "Assign": { "message": "{% $greeting & ', ' & $target %}" },
      "Next": "Emit"
    },
    "Emit": {
      "Type": "Pass",
      "Output": "{% { 'text': $message, 'to': $target } %}",
      "End": true
    }
  }
}"#;

/// The one log destination a logging configuration may carry: `destinations` is
/// "limited to size 1" and "required, if your log level is not set to OFF".
fn log_destinations() -> Value {
    json!([{
        "cloudWatchLogsLogGroup": {
            "logGroupArn": "arn:aws:logs:us-west-2:000000000000:log-group:/sfn:*"
        }
    }])
}

/// A KMS key ARN inside the modelled `kmsKeyId` length (1..2048).
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// `DescribeStateMachineForExecution` on `execution_arn`, which must succeed, with
/// the `includedData` the row names (none when `included` is `None`).
async fn describe_for_execution(
    emulator: &Emulator,
    execution_arn: &str,
    included: Option<&str>,
) -> Value {
    let mut request = json!({ "executionArn": execution_arn });
    if let Some(included) = included {
        request["includedData"] = json!(included);
    }
    emulator
        .ok("DescribeStateMachineForExecution", &request)
        .await
}

/// `DescribeStateMachine` on `arn`, which must succeed.
async fn describe_machine(emulator: &Emulator, arn: &str) -> Value {
    emulator
        .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
        .await
}

/// Publish the state machine's current configuration as its next version, and return
/// that version's ARN.
async fn publish(emulator: &Emulator, machine_arn: &str) -> String {
    let published = emulator
        .ok(
            "PublishStateMachineVersion",
            &json!({ "stateMachineArn": machine_arn }),
        )
        .await;
    string(&published, "stateMachineVersionArn")
}

/// Replace the state machine's definition (and, when given, its role) without
/// publishing a version, and return the `updateDate` the update answered.
async fn update(
    emulator: &Emulator,
    machine_arn: &str,
    definition: &str,
    role_arn: Option<&str>,
) -> f64 {
    let mut request = json!({ "stateMachineArn": machine_arn, "definition": definition });
    if let Some(role_arn) = role_arn {
        request["roleArn"] = json!(role_arn);
    }
    let updated = emulator.ok("UpdateStateMachine", &request).await;
    number(&updated, "updateDate")
}

/// Create an alias routing every start to `version_arn`, and return its ARN.
async fn create_alias(emulator: &Emulator, name: &str, version_arn: &str) -> String {
    let created = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({
                "name": name,
                "routingConfiguration": [
                    { "stateMachineVersionArn": version_arn, "weight": 100 }
                ],
            }),
        )
        .await;
    string(&created, "stateMachineAliasArn")
}

/// The numeric member `name` of `body`, which must be present — a timestamp as this
/// protocol puts one on the wire.
fn number(body: &Value, name: &str) -> f64 {
    body.get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric '{name}' in {body:?}"))
}

/// Assert that `described` carries no `mapRunArn` and no `label`.
///
/// Both are "returned only if the executionArn is a child workflow execution that
/// was started by a Distributed Map state", and no execution this emulator stores is
/// one — including an execution that itself ENTERED a Distributed Map state, which
/// owns the Map Run rather than being its child.
fn assert_no_map_run_members(case: &str, described: &Value) {
    for member in ["mapRunArn", "label"] {
        assert!(
            described.get(member).is_none_or(Value::is_null),
            "case {case:?}: expected no {member} for an execution no Distributed Map \
             dispatched, got {described:?}"
        );
    }
}

/// Assert that the configuration `described` reports is the one
/// `DescribeStateMachine` reports for `config_arn`.
async fn assert_matches_machine(
    emulator: &Emulator,
    case: &str,
    described: &Value,
    config_arn: &str,
) {
    let reference = describe_machine(emulator, config_arn).await;
    for member in SHARED_MEMBERS {
        assert_eq!(
            described.get(*member),
            reference.get(*member),
            "case {case:?}: expected {member} {:?} (what DescribeStateMachine reports for \
             {config_arn}), got {:?}",
            reference.get(*member),
            described.get(*member)
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: the configuration the execution RAN
// ---------------------------------------------------------------------------

/// One row of the resolution table: how the execution was started, the ARN whose
/// `DescribeStateMachine` reports the configuration that start ran, and the
/// `updateDate` that configuration was last written at.
struct RanCase {
    case: &'static str,
    start_arn: String,
    config_arn: String,
    update_date: f64,
}

/// Whatever ARN the start named, the operation reports the configuration the
/// execution ran, under the unqualified ARN of its state machine.
#[tokio::test]
async fn reports_the_configuration_the_execution_ran() {
    let emulator = Emulator::new();

    // Version 1 holds `definition(1)` under `ROLE_ARN`, version 2 `definition(2)`
    // under `OTHER_ROLE_ARN`, and the CURRENT record `definition(3)` — published by
    // nothing, so the unqualified ARN and each version ARN name three different
    // configurations of one state machine.
    let machine = create_machine(&emulator, "ran-config", &definition(1)).await;
    let first_version = publish(&emulator, &machine).await;
    update(&emulator, &machine, &definition(2), Some(OTHER_ROLE_ARN)).await;
    let second_version = publish(&emulator, &machine).await;
    let machine_update_date = update(&emulator, &machine, &definition(3), Some(ROLE_ARN)).await;
    let alias = create_alias(&emulator, "prod", &first_version).await;

    // A version is a snapshot nothing updates after the publish that took it, so the
    // instant it was created is the instant the configuration it holds was written —
    // which is what `updateDate` reports for an execution that ran it.
    let first_created = number(&describe_machine(&emulator, &first_version).await, "creationDate");
    let second_created =
        number(&describe_machine(&emulator, &second_version).await, "creationDate");

    // Rows differ along the FORM the start's ARN took, which is what decides the
    // configuration the execution ran.
    let cases = [
        RanCase {
            case: "an unqualified start runs the state machine's current record",
            start_arn: machine.clone(),
            config_arn: machine.clone(),
            update_date: machine_update_date,
        },
        RanCase {
            case: "a start through version 1 runs that version's snapshot",
            start_arn: first_version.clone(),
            config_arn: first_version.clone(),
            update_date: first_created,
        },
        RanCase {
            case: "a start through version 2 runs that version's snapshot",
            start_arn: second_version.clone(),
            config_arn: second_version.clone(),
            update_date: second_created,
        },
        RanCase {
            case: "a start through an alias runs the version it routes to",
            start_arn: alias.clone(),
            config_arn: first_version.clone(),
            update_date: first_created,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let execution =
            start_execution(&emulator, &row.start_arn, &format!("ran-{index}"), "{}").await;
        let described = describe_for_execution(&emulator, &execution, None).await;

        assert_eq!(
            described.get("stateMachineArn").and_then(Value::as_str),
            Some(machine.as_str()),
            "case {:?}: expected the unqualified stateMachineArn {machine}, got {described:?}",
            row.case
        );
        assert_matches_machine(&emulator, row.case, &described, &row.config_arn).await;
        assert_eq!(
            number(&described, "updateDate"),
            row.update_date,
            "case {:?}: expected updateDate {}, got {described:?}",
            row.case,
            row.update_date
        );
        assert_no_map_run_members(row.case, &described);
    }
}

/// What happens to the state machine AFTER an execution of it has started.
#[derive(Debug, Clone, Copy)]
enum Later {
    /// An update that replaces the definition and the role, publishing nothing.
    Update,
    /// Two updates in a row, so the record is two revisions past the start.
    TwoUpdates,
    /// An update that publishes a new version in the same action.
    UpdateAndPublish,
    /// Deleting the version the start ran, which "won't terminate its in-progress
    /// executions".
    DeleteTheVersion,
}

/// An execution is PINNED to the configuration it started with: whatever happens to
/// the state machine afterwards, the report for that execution does not move.
#[tokio::test]
async fn pins_the_configuration_to_the_one_in_effect_at_the_start() {
    let emulator = Emulator::new();

    // Rows differ along how the start resolved its configuration and what moved the
    // state machine afterwards.
    let cases: &[(&str, bool, Later)] = &[
        (
            "an update after an unqualified start",
            false,
            Later::Update,
        ),
        (
            "two updates after an unqualified start",
            false,
            Later::TwoUpdates,
        ),
        (
            "an update that publishes a version after an unqualified start",
            false,
            Later::UpdateAndPublish,
        ),
        (
            "an update after a start through a version",
            true,
            Later::Update,
        ),
        (
            "deleting the version the start ran",
            true,
            Later::DeleteTheVersion,
        ),
    ];

    for (index, (case, through_version, later)) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("pinned-{index}"), &definition(1)).await;
        let version = publish(&emulator, &machine).await;
        let start_arn = if *through_version {
            version.clone()
        } else {
            machine.clone()
        };
        let execution =
            start_execution(&emulator, &start_arn, &format!("pinned-{index}"), "{}").await;

        let at_start = describe_for_execution(&emulator, &execution, None).await;
        assert_eq!(
            at_start.get("definition").and_then(Value::as_str),
            Some(definition(1).as_str()),
            "case {case:?}: expected the definition in effect at the start, got {at_start:?}"
        );

        match later {
            Later::Update => {
                update(&emulator, &machine, &definition(9), Some(OTHER_ROLE_ARN)).await;
            }
            Later::TwoUpdates => {
                update(&emulator, &machine, &definition(9), Some(OTHER_ROLE_ARN)).await;
                update(&emulator, &machine, &definition(10), Some(ROLE_ARN)).await;
            }
            Later::UpdateAndPublish => {
                emulator
                    .ok(
                        "UpdateStateMachine",
                        &json!({
                            "stateMachineArn": machine,
                            "definition": definition(9),
                            "roleArn": OTHER_ROLE_ARN,
                            "publish": true,
                        }),
                    )
                    .await;
            }
            Later::DeleteTheVersion => {
                emulator
                    .ok(
                        "DeleteStateMachineVersion",
                        &json!({ "stateMachineVersionArn": version }),
                    )
                    .await;
            }
        }

        let after = describe_for_execution(&emulator, &execution, None).await;
        assert_eq!(
            after, at_start,
            "case {case:?}: expected the report for an execution already started to be \
             unmoved by {later:?}, got {after:?} after {at_start:?}"
        );
        assert_eq!(
            after.get("definition").and_then(Value::as_str),
            Some(definition(1).as_str()),
            "case {case:?}: expected the definition the execution ran, got {after:?}"
        );
    }
}

/// A redrive re-runs the configuration the execution started with — "redriven
/// executions use the same state machine definition [...] as the original execution
/// attempt" — so an update between two attempts changes neither what the re-run does
/// nor what the report says.
#[tokio::test]
async fn a_redrive_re_runs_the_configuration_the_execution_started_with() {
    let emulator = Emulator::new();

    // A definition that fails at a `Fail` state, so the execution is redrivable.
    let failing = json!({
        "StartAt": "Bang",
        "States": { "Bang": { "Type": "Fail", "Error": "Boom", "Cause": "on purpose" } },
    })
    .to_string();

    let machine = create_machine(&emulator, "redrive-pinned", &failing).await;
    let execution = start_execution(&emulator, &machine, "redrive-pinned", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    // The state machine moves to a definition that would SUCCEED instead.
    update(&emulator, &machine, HELLO_DEFINITION, None).await;
    let pinned = describe_for_execution(&emulator, &execution, None).await;
    assert_eq!(
        pinned.get("definition").and_then(Value::as_str),
        Some(failing.as_str()),
        "case {:?}: expected the definition the first attempt ran, got {pinned:?}",
        "after the update"
    );

    emulator
        .ok("RedriveExecution", &json!({ "executionArn": execution }))
        .await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let settled = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            settled.get("status").and_then(Value::as_str),
            settled.get("error").and_then(Value::as_str),
        ),
        (Some("FAILED"), Some("Boom")),
        "case {:?}: expected the re-run to run the pinned definition, got {settled:?}",
        "the redriven attempt"
    );
    let after = describe_for_execution(&emulator, &execution, None).await;
    assert_eq!(
        after, pinned,
        "case {:?}: expected the redrive to leave the reported configuration alone, got \
         {after:?} after {pinned:?}",
        "after the redrive"
    );
}

/// An execution that entered a Distributed Map state owns a Map Run, and is still
/// not a child workflow execution: the two Map Run members stay absent while
/// `ListMapRuns` reports the run.
#[tokio::test]
async fn reports_no_map_run_members_for_an_execution_that_owns_a_map_run() {
    let emulator = Emulator::new();

    let distributed = json!({
        "StartAt": "M",
        "States": {
            "M": {
                "Type": "Map",
                "Items": [1, 2],
                "ItemProcessor": {
                    "ProcessorConfig": { "Mode": "DISTRIBUTED" },
                    "StartAt": "P",
                    "States": { "P": { "Type": "Pass", "End": true } },
                },
                "End": true,
            },
        },
    })
    .to_string();

    let machine = create_machine(&emulator, "map-owner", &distributed).await;
    let execution = start_execution(&emulator, &machine, "owns-a-map-run", "{}").await;

    let listed = emulator
        .ok("ListMapRuns", &json!({ "executionArn": execution }))
        .await;
    let runs = listed
        .get("mapRuns")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a mapRuns list in {listed:?}"));
    assert_eq!(
        runs.len(),
        1,
        "case {:?}: expected the Distributed Map state to start one Map Run, got {listed:?}",
        "the owning execution"
    );

    let described = describe_for_execution(&emulator, &execution, None).await;
    assert_no_map_run_members("an execution that owns a Map Run", &described);
}

// ---------------------------------------------------------------------------
// Rule: the EFFECTIVE configuration structures
// ---------------------------------------------------------------------------

/// Whatever subset of the three configuration structures the create supplied, the
/// read reports all three: the ones supplied as supplied, the rest at their own
/// modelled defaults — through both arms of the resolution above.
#[tokio::test]
async fn reports_every_configuration_structure_at_its_effective_value() {
    let emulator = Emulator::new();

    let default_logging = json!({ "level": "OFF", "includeExecutionData": false });
    let default_tracing = json!({ "enabled": false });
    let default_encryption = json!({ "type": "AWS_OWNED_KEY" });

    // Rows differ along WHICH configurations the create named, what each held, and
    // whether the execution reached them through the state machine or through a
    // published version of it.
    let cases: Vec<(&str, Value, Value, Value, Value, bool)> = vec![
        (
            "no configuration at all, read through the state machine",
            json!({}),
            default_logging.clone(),
            default_tracing.clone(),
            default_encryption.clone(),
            false,
        ),
        (
            "no configuration at all, read through a version",
            json!({}),
            default_logging.clone(),
            default_tracing.clone(),
            default_encryption.clone(),
            true,
        ),
        (
            "tracing only",
            json!({ "tracingConfiguration": { "enabled": true } }),
            default_logging.clone(),
            json!({ "enabled": true }),
            default_encryption.clone(),
            false,
        ),
        (
            "a logging level with no includeExecutionData, read through a version",
            json!({ "loggingConfiguration": { "level": "ERROR", "destinations": log_destinations() } }),
            json!({
                "level": "ERROR",
                "includeExecutionData": false,
                "destinations": log_destinations(),
            }),
            default_tracing.clone(),
            default_encryption.clone(),
            true,
        ),
        (
            "logging at ALL with execution data, and a customer-managed key",
            json!({
                "loggingConfiguration": {
                    "level": "ALL",
                    "includeExecutionData": true,
                    "destinations": log_destinations(),
                },
                "encryptionConfiguration": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyId": KMS_KEY_ARN,
                },
            }),
            json!({
                "level": "ALL",
                "includeExecutionData": true,
                "destinations": log_destinations(),
            }),
            default_tracing.clone(),
            json!({ "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": KMS_KEY_ARN }),
            false,
        ),
    ];

    for (index, (case, configuration, logging, tracing, encryption, through_version)) in
        cases.iter().enumerate()
    {
        let mut request = json!({
            "name": format!("effective-{index}"),
            "definition": HELLO_DEFINITION,
            "roleArn": ROLE_ARN,
        });
        for (member, value) in configuration.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let created = emulator.ok("CreateStateMachine", &request).await;
        let machine = string(&created, "stateMachineArn");

        let start_arn = if *through_version {
            publish(&emulator, &machine).await
        } else {
            machine.clone()
        };
        let execution =
            start_execution(&emulator, &start_arn, &format!("effective-{index}"), "{}").await;
        let described = describe_for_execution(&emulator, &execution, None).await;

        // Each structure is compared WITH the type discriminator this service puts on
        // it, so the wire shape is asserted whole.
        for (member, expected) in [
            ("loggingConfiguration", logging),
            ("tracingConfiguration", tracing),
            ("encryptionConfiguration", encryption),
        ] {
            let shape = member[..1].to_uppercase() + &member[1..];
            let expected = discriminated(&shape, expected.clone());
            assert_eq!(
                described.get(member),
                Some(&expected),
                "case {case:?}: expected {member} {expected}, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: includedData decides the definition and what is read out of it
// ---------------------------------------------------------------------------

/// `includedData = METADATA_ONLY` reports the `definition` as `{}` and withholds
/// `variableReferences`, which is read out of the definition; every other value — and
/// an absent member — reports both in full.
#[tokio::test]
async fn included_data_decides_the_definition_and_its_variable_references() {
    let emulator = Emulator::new();

    // Rows differ along the definition (variable references in JSONata expressions,
    // a JSONPath reference field, none at all), how the start resolved its
    // configuration, and which `includedData` the read names.
    let cases: Vec<(&str, String, Value, bool)> = vec![
        (
            "a JSONata definition that assigns and reads variables",
            JSONATA_DEFINITION.to_string(),
            json!({ "Compose": ["greeting", "target"], "Emit": ["message", "target"] }),
            false,
        ),
        (
            "a JSONata definition read through a published version",
            JSONATA_DEFINITION.to_string(),
            json!({ "Compose": ["greeting", "target"], "Emit": ["message", "target"] }),
            true,
        ),
        (
            "a definition whose states reference no variable",
            HELLO_DEFINITION.to_string(),
            json!({}),
            false,
        ),
    ];

    for (index, (case, definition, references, through_version)) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("included-{index}"), definition).await;
        let start_arn = if *through_version {
            publish(&emulator, &machine).await
        } else {
            machine.clone()
        };
        let execution =
            start_execution(&emulator, &start_arn, &format!("included-{index}"), "{}").await;

        // An absent `includedData` and an explicit ALL_DATA report everything; only
        // METADATA_ONLY withholds the definition and what derives from it.
        for included in [None, Some(INCLUDED_DATA_ALL_DATA)] {
            let described = describe_for_execution(&emulator, &execution, included).await;
            assert_eq!(
                described.get("definition").and_then(Value::as_str),
                Some(definition.as_str()),
                "case {case:?} with includedData {included:?}: expected the whole definition, \
                 got {described:?}"
            );
            let reported = described
                .get("variableReferences")
                .cloned()
                .unwrap_or_else(|| json!({}));
            assert_eq!(
                &reported, references,
                "case {case:?} with includedData {included:?}: expected variableReferences \
                 {references}, got {reported}"
            );
        }

        let metadata =
            describe_for_execution(&emulator, &execution, Some(INCLUDED_DATA_METADATA_ONLY)).await;
        assert_eq!(
            metadata.get("definition").and_then(Value::as_str),
            Some(METADATA_ONLY_DEFINITION),
            "case {case:?}: expected the metadata-only definition, got {metadata:?}"
        );
        assert!(
            metadata
                .get("variableReferences")
                .is_none_or(Value::is_null),
            "case {case:?}: expected no variableReferences for a metadata-only read, got \
             {metadata:?}"
        );
        // The metadata the read does carry is the configuration's, unchanged.
        assert_eq!(
            metadata.get("roleArn").and_then(Value::as_str),
            Some(ROLE_ARN),
            "case {case:?}: expected the roleArn beside the withheld definition, got {metadata:?}"
        );
    }
}

/// A value outside the modelled `IncludedData` enum is a bad request, not a report.
#[tokio::test]
async fn refuses_an_included_data_the_model_does_not_declare() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "bad-included-data", HELLO_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "bad-included-data", "{}").await;

    let (status, code) = emulator
        .err(
            "DescribeStateMachineForExecution",
            &json!({ "executionArn": execution, "includedData": "EVERYTHING" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {:?}: expected a 400 ValidationException, got {status} {code}",
        "an includedData outside the enum"
    );
}

// ---------------------------------------------------------------------------
// Rule: an executionArn no record answers for
// ---------------------------------------------------------------------------

/// The ARN is judged BEFORE the lookup, and only an execution of this endpoint's
/// region that this API reports answers at all.
#[tokio::test]
async fn refuses_an_execution_arn_no_record_answers_for() {
    let emulator = Emulator::new();

    let machine = create_machine(&emulator, "refusals", HELLO_DEFINITION).await;
    let live = start_execution(&emulator, &machine, "live", "{}").await;

    // An EXPRESS workflow's execution: "this API action is not supported by EXPRESS
    // state machines", and an EXPRESS execution is named by the `express` ARN its run
    // carries its own id in — a resource type no execution-ARN member of this service
    // accepts — so the ARN is refused before any record is looked up.
    let express = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "refusals-express",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        )
        .await;
    let express_execution = start_execution(
        &emulator,
        &string(&express, "stateMachineArn"),
        "express",
        "{}",
    )
    .await;

    // An execution whose state machine was deleted: the deletion takes its
    // executions with it, so nothing is left to report once the clock made it due.
    let doomed = create_machine(&emulator, "refusals-deleted", HELLO_DEFINITION).await;
    let gone = start_execution(&emulator, &doomed, "gone", "{}").await;
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": doomed }))
        .await;
    std::thread::sleep(PAST_THE_DELETE_DELAY);

    let other_region = live.replacen(REGION, "eu-west-1", 1);
    let unknown_name = format!("{live}-not-started");
    let unknown_machine = format!(
        "arn:aws:states:{REGION}:{ACCOUNT}:execution:refusals-absent:{}",
        "live"
    );

    // Rows differ along WHICH part of the ARN makes it unanswerable: its syntax, its
    // service, its resource type, its region, the record it names, and the workflow
    // type of the state machine behind it.
    let cases: &[(&str, &str, &str)] = &[
        ("text that is not an ARN", "not-an-arn", "InvalidArn"),
        (
            "an ARN of another service",
            "arn:aws:sqs:us-west-2:000000000000:execution:refusals:live",
            "InvalidArn",
        ),
        (
            "a state machine ARN, not an execution ARN",
            machine.as_str(),
            "InvalidArn",
        ),
        (
            "an execution ARN of another region",
            other_region.as_str(),
            "InvalidArn",
        ),
        (
            "an execution name nothing started",
            unknown_name.as_str(),
            "ExecutionDoesNotExist",
        ),
        (
            "an execution ARN of a state machine that does not exist",
            unknown_machine.as_str(),
            "ExecutionDoesNotExist",
        ),
        (
            "an EXPRESS workflow's execution",
            express_execution.as_str(),
            "InvalidArn",
        ),
        (
            "an execution whose state machine was deleted",
            gone.as_str(),
            "ExecutionDoesNotExist",
        ),
    ];

    for (case, execution_arn, expected) in cases {
        let (status, code) = emulator
            .err(
                "DescribeStateMachineForExecution",
                &json!({ "executionArn": execution_arn }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, *expected),
            "case {case:?}: expected a 400 {expected}, got {status} {code}"
        );
    }

    // The refusal names the execution the request asked about.
    let (_, body) = emulator
        .call(
            "DescribeStateMachineForExecution",
            &json!({ "executionArn": unknown_name }),
        )
        .await;
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains(&unknown_name) && error_code(&body) == "ExecutionDoesNotExist",
        "case {:?}: expected the refusal to name {unknown_name}, got {body:?}",
        "the message of an absent execution"
    );
}

// ---------------------------------------------------------------------------
// Rule: the call REPORTS
// ---------------------------------------------------------------------------

/// Describing the state machine for an execution changes nothing a later caller
/// sees: the second read answers exactly what the first did, and every other reader
/// of the same records is unmoved.
#[tokio::test]
async fn reporting_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    let machine = create_machine(&emulator, "reports-only", HELLO_DEFINITION).await;
    let version = publish(&emulator, &machine).await;
    let plain = start_execution(&emulator, &machine, "plain", "{}").await;
    let versioned = start_execution(&emulator, &version, "versioned", "{}").await;

    // Past the delay the clock owed, so the executions have reached their terminal
    // status and nothing but this call could move what the readers below report.
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for execution in [&plain, &versioned] {
        let before_machine = describe_machine(&emulator, &machine).await;
        let before_execution = emulator
            .ok("DescribeExecution", &json!({ "executionArn": execution }))
            .await;

        let first = describe_for_execution(&emulator, execution, None).await;
        let second = describe_for_execution(&emulator, execution, None).await;
        assert_eq!(
            first, second,
            "case {execution:?}: expected a second read to answer the first's body, got \
             {first:?} then {second:?}"
        );

        let after_machine = describe_machine(&emulator, &machine).await;
        let after_execution = emulator
            .ok("DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        assert_eq!(
            before_machine, after_machine,
            "case {execution:?}: expected DescribeStateMachine to be unmoved by the report"
        );
        assert_eq!(
            before_execution, after_execution,
            "case {execution:?}: expected DescribeExecution to be unmoved by the report"
        );
    }
}

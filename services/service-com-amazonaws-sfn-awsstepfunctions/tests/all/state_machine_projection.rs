// SPDX-License-Identifier: CC0-1.0

//! What a state machine READ reports, and what an UPDATE writes, over this
//! service's own wire protocol (`awsJson1_0`) through the production router.
//!
//! The rules under test, each asserted over a TABLE that spans its class rather
//! than one recorded request:
//!
//! 1. a describe reports a state machine's EFFECTIVE configuration: a
//!    `loggingConfiguration`, `tracingConfiguration` or `encryptionConfiguration`
//!    no create or update set is reported at the default its own model declares
//!    (`level: OFF`, `includeExecutionData: false`, `enabled: false`,
//!    `type: AWS_OWNED_KEY`), never omitted;
//! 2. `variableReferences` reports, per state, the variables that state's own
//!    expressions reference, and leaves out a state that references none;
//! 3. `includedData = METADATA_ONLY` reports the `definition` as `{}` — the
//!    member's own documentation — and withholds what is derived from it;
//! 4. an update persists exactly the members it names, retains the rest, and mints
//!    a fresh `revisionId` that every later read reports — and an update that names
//!    neither a `definition` nor a `roleArn` is the modelled
//!    `MissingRequiredParameter`.
//!
//! The harness lives in `tests/all/state_machine_lifecycle.rs`: `tests/all.rs` is
//! this binary's crate root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Map, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    ENCRYPTION_TYPE_AWS_OWNED_KEY, INCLUDED_DATA_ALL_DATA, INCLUDED_DATA_METADATA_ONLY,
    LOG_LEVEL_OFF, METADATA_ONLY_DEFINITION,
};

use crate::state_machine_lifecycle::{
    create_machine, string, Emulator, HELLO_DEFINITION, PAST_THE_DELETE_DELAY, ROLE_ARN,
};

/// Every value `LogLevel` declares, from the model
/// (`smithy_query(query="shape",shape="LogLevel")` -> `["ALL","ERROR","FATAL","OFF"]`,
/// which the crate also carries as `ops::types::LOG_LEVEL_VALUES`).
const LOG_LEVELS: &[&str] = &["ALL", "ERROR", "FATAL", "OFF"];

/// Every value `EncryptionType` declares, from the model
/// (`smithy_query(query="shape",shape="EncryptionType")`).
const ENCRYPTION_TYPES: &[&str] = &["AWS_OWNED_KEY", "CUSTOMER_MANAGED_KMS_KEY"];

/// The published bounds of `EncryptionConfiguration.kmsDataKeyReusePeriodSeconds`
/// (`range 60..900` in the model).
const KMS_REUSE_PERIOD_BOUNDS: (i64, i64) = (60, 900);

/// A KMS key ARN inside the modelled `kmsKeyId` length (1..2048).
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The one log destination a logging configuration may carry: `destinations` is
/// "limited to size 1" and "required, if your log level is not set to OFF", which
/// `CreateStateMachine` and `UpdateStateMachine` both enforce, so every row below
/// that logs at a level other than OFF carries exactly this.
fn log_destinations() -> Value {
    json!([{
        "cloudWatchLogsLogGroup": {
            "logGroupArn": "arn:aws:logs:us-west-2:000000000000:log-group:/sfn:*"
        }
    }])
}

/// A JSONata definition whose two states read variables: `Compose` reads the two
/// it composes, `Emit` reads what `Compose` assigned and one it did not.
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

/// Describe one state machine, requiring a 200.
async fn describe(emulator: &Emulator, arn: &str) -> Value {
    emulator
        .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
        .await
}

/// The `variableReferences` of a describe body as a sorted, comparable JSON object
/// (`{}` when the member is absent).
fn variable_references(described: &Value) -> Value {
    described
        .get("variableReferences")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

// ---------------------------------------------------------------------------
// Rule 1: a read reports the EFFECTIVE configuration
// ---------------------------------------------------------------------------

/// Whatever subset of the three configuration structures a create supplied, the
/// describe reports all three: the ones supplied as supplied, the rest at their own
/// modelled defaults.
#[tokio::test]
async fn describe_reports_every_configuration_structure_at_its_effective_value() {
    let emulator = Emulator::new();

    let default_logging = json!({ "level": LOG_LEVEL_OFF, "includeExecutionData": false });
    let default_tracing = json!({ "enabled": false });
    let default_encryption = json!({ "type": ENCRYPTION_TYPE_AWS_OWNED_KEY });

    // Rows differ along WHICH configurations the create named and what each held:
    // none, one, two, all three, and a logging configuration that names a level but
    // not the boolean beside it.
    let mut cases: Vec<(String, Value, Value, Value, Value)> = vec![
        (
            "no configuration at all".to_string(),
            json!({}),
            default_logging.clone(),
            default_tracing.clone(),
            default_encryption.clone(),
        ),
        (
            "tracing only".to_string(),
            json!({ "tracingConfiguration": { "enabled": true } }),
            default_logging.clone(),
            json!({ "enabled": true }),
            default_encryption.clone(),
        ),
        (
            "a logging level with no includeExecutionData".to_string(),
            json!({ "loggingConfiguration": { "level": "ERROR", "destinations": log_destinations() } }),
            json!({
                "level": "ERROR",
                "includeExecutionData": false,
                "destinations": log_destinations(),
            }),
            default_tracing.clone(),
            default_encryption.clone(),
        ),
        (
            "logging with a destination, and tracing".to_string(),
            json!({
                "loggingConfiguration": {
                    "level": "ALL",
                    "includeExecutionData": true,
                    "destinations": log_destinations(),
                },
                "tracingConfiguration": { "enabled": false },
            }),
            json!({
                "level": "ALL",
                "includeExecutionData": true,
                "destinations": log_destinations(),
            }),
            json!({ "enabled": false }),
            default_encryption.clone(),
        ),
        (
            "a customer-managed key at the published lower reuse bound".to_string(),
            json!({
                "encryptionConfiguration": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyId": KMS_KEY_ARN,
                    "kmsDataKeyReusePeriodSeconds": KMS_REUSE_PERIOD_BOUNDS.0,
                },
            }),
            default_logging.clone(),
            default_tracing.clone(),
            json!({
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": KMS_KEY_ARN,
                "kmsDataKeyReusePeriodSeconds": KMS_REUSE_PERIOD_BOUNDS.0,
            }),
        ),
        (
            "a customer-managed key at the published upper reuse bound".to_string(),
            json!({
                "encryptionConfiguration": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyId": KMS_KEY_ARN,
                    "kmsDataKeyReusePeriodSeconds": KMS_REUSE_PERIOD_BOUNDS.1,
                },
            }),
            default_logging.clone(),
            default_tracing.clone(),
            json!({
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": KMS_KEY_ARN,
                "kmsDataKeyReusePeriodSeconds": KMS_REUSE_PERIOD_BOUNDS.1,
            }),
        ),
    ];

    // One row per value of the two enums the model declares, so the rule is pinned
    // on every published member and not only on the recorded one. A level other than
    // OFF needs its one destination, and only the customer-managed encryption type
    // carries a `kmsKeyId` — the two input rules `CreateStateMachine` enforces.
    for level in LOG_LEVELS {
        let mut logging = json!({ "level": level, "includeExecutionData": true });
        if *level != LOG_LEVEL_OFF {
            logging["destinations"] = log_destinations();
        }
        cases.push((
            format!("the modelled log level {level}"),
            json!({ "loggingConfiguration": logging.clone() }),
            logging,
            default_tracing.clone(),
            default_encryption.clone(),
        ));
    }
    for encryption_type in ENCRYPTION_TYPES {
        let encryption = if *encryption_type == ENCRYPTION_TYPE_AWS_OWNED_KEY {
            json!({ "type": encryption_type })
        } else {
            json!({ "type": encryption_type, "kmsKeyId": KMS_KEY_ARN })
        };
        cases.push((
            format!("the modelled encryption type {encryption_type}"),
            json!({ "encryptionConfiguration": encryption.clone() }),
            default_logging.clone(),
            default_tracing.clone(),
            encryption,
        ));
    }

    for (index, (case, configuration, logging, tracing, encryption)) in cases.iter().enumerate() {
        let mut request = json!({
            "name": format!("effective-{index}"),
            "definition": HELLO_DEFINITION,
            "roleArn": ROLE_ARN,
        });
        for (member, value) in configuration.as_object().expect("an object of members") {
            request[member] = value.clone();
        }

        let created = emulator.ok("CreateStateMachine", &request).await;
        let arn = string(&created, "stateMachineArn");
        let described = describe(&emulator, &arn).await;

        assert_eq!(
            described.get("loggingConfiguration"),
            Some(logging),
            "case {case:?}: expected loggingConfiguration {logging}, got {described:?}"
        );
        assert_eq!(
            described.get("tracingConfiguration"),
            Some(tracing),
            "case {case:?}: expected tracingConfiguration {tracing}, got {described:?}"
        );
        assert_eq!(
            described.get("encryptionConfiguration"),
            Some(encryption),
            "case {case:?}: expected encryptionConfiguration {encryption}, got {described:?}"
        );
        // A freshly created state machine has no revision: nothing has revised it.
        assert!(
            described.get("revisionId").is_none_or(Value::is_null),
            "case {case:?}: expected no revisionId before any update, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: variableReferences is read out of the definition
// ---------------------------------------------------------------------------

/// Each state is reported with the variables ITS OWN expressions reference, sorted
/// and without repeats, and a state that references none is absent.
#[tokio::test]
async fn describe_reports_the_variables_each_state_references() {
    let emulator = Emulator::new();

    // Rows differ along the FORM the reference takes: JSONata assignments and
    // outputs, a reserved context object, a function call, a JSONPath reference
    // field, a literal that merely contains a `$`, a repeated reference, and states
    // nested inside a Parallel branch.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a JSONata definition that assigns and reads variables",
            JSONATA_DEFINITION.to_string(),
            json!({ "Compose": ["greeting", "target"], "Emit": ["message", "target"] }),
        ),
        (
            "a definition with no expressions at all",
            HELLO_DEFINITION.to_string(),
            json!({}),
        ),
        (
            "only the reserved context object",
            r#"{"StartAt":"Only","States":{"Only":{"Type":"Pass","Output":"{% $states.input %}","End":true}}}"#
                .to_string(),
            json!({}),
        ),
        (
            "a function call around a variable",
            r#"{"StartAt":"Call","States":{"Call":{"Type":"Pass","Output":"{% $string($count) & $suffix %}","End":true}}}"#
                .to_string(),
            json!({ "Call": ["count", "suffix"] }),
        ),
        (
            "a JSONPath reference field beside a path and the context",
            r#"{"StartAt":"Path","States":{"Path":{"Type":"Pass","Parameters":{"greeting.$":"$hello","from.$":"$.input.x","run.$":"$$.Execution.Name"},"End":true}}}"#
                .to_string(),
            json!({ "Path": ["hello"] }),
        ),
        (
            "a literal that merely contains a dollar sign",
            r#"{"StartAt":"Literal","States":{"Literal":{"Type":"Pass","Result":{"price":"$total"},"End":true}}}"#
                .to_string(),
            json!({}),
        ),
        (
            "the same variable read twice in one state",
            r#"{"StartAt":"Twice","States":{"Twice":{"Type":"Pass","Output":"{% $n + $n %}","End":true}}}"#
                .to_string(),
            json!({ "Twice": ["n"] }),
        ),
        (
            "a variable read inside a Parallel branch",
            r#"{"StartAt":"Fan","States":{"Fan":{"Type":"Parallel","Branches":[{"StartAt":"Left","States":{"Left":{"Type":"Pass","Output":"{% $leftVar %}","End":true}}}],"End":true}}}"#
                .to_string(),
            json!({ "Left": ["leftVar"] }),
        ),
        (
            "a nested state sharing a top-level state's name",
            r#"{"StartAt":"Work","States":{"Work":{"Type":"Parallel","Output":"{% $outer %}","Branches":[{"StartAt":"Work","States":{"Work":{"Type":"Pass","Output":"{% $inner %}","End":true}}}],"End":true}}}"#
                .to_string(),
            json!({ "Work": ["inner", "outer"] }),
        ),
        (
            "a literal payload member that happens to be called States",
            r#"{"StartAt":"Carry","States":{"Carry":{"Type":"Pass","Result":{"States":{"Ghost":{"Type":"Pass"}}},"Output":"{% $kept %}","End":true}}}"#
                .to_string(),
            json!({ "Carry": ["kept"] }),
        ),
    ];

    for (index, (case, definition, expected)) in cases.iter().enumerate() {
        let arn = create_machine(&emulator, &format!("refs-{index}"), definition).await;
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            &variable_references(&described),
            expected,
            "case {case:?}: expected variableReferences {expected}, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: includedData decides what a read reports of the definition
// ---------------------------------------------------------------------------

/// `METADATA_ONLY` reports the definition as `{}` and withholds what is derived
/// from it; absent and `ALL_DATA` report it in full. A value outside the modelled
/// enum is refused, not guessed at.
#[tokio::test]
async fn metadata_only_blanks_the_definition_and_what_is_derived_from_it() {
    let emulator = Emulator::new();
    let with_variables = create_machine(&emulator, "included-jsonata", JSONATA_DEFINITION).await;
    let without_variables = create_machine(&emulator, "included-plain", HELLO_DEFINITION).await;

    let all_references = json!({ "Compose": ["greeting", "target"], "Emit": ["message", "target"] });

    // Rows cross the two definitions with every form `includedData` can take.
    let cases: Vec<(&str, &String, Option<&str>, &str, Value)> = vec![
        (
            "a JSONata definition, no includedData",
            &with_variables,
            None,
            JSONATA_DEFINITION,
            all_references.clone(),
        ),
        (
            "a JSONata definition, ALL_DATA",
            &with_variables,
            Some(INCLUDED_DATA_ALL_DATA),
            JSONATA_DEFINITION,
            all_references.clone(),
        ),
        (
            "a JSONata definition, METADATA_ONLY",
            &with_variables,
            Some(INCLUDED_DATA_METADATA_ONLY),
            METADATA_ONLY_DEFINITION,
            json!({}),
        ),
        (
            "a definition with no variables, no includedData",
            &without_variables,
            None,
            HELLO_DEFINITION,
            json!({}),
        ),
        (
            "a definition with no variables, METADATA_ONLY",
            &without_variables,
            Some(INCLUDED_DATA_METADATA_ONLY),
            METADATA_ONLY_DEFINITION,
            json!({}),
        ),
    ];

    for (case, arn, included_data, definition, references) in cases {
        let mut request = json!({ "stateMachineArn": arn });
        if let Some(included_data) = included_data {
            request["includedData"] = json!(included_data);
        }
        let described = emulator.ok("DescribeStateMachine", &request).await;
        assert_eq!(
            described.get("definition").and_then(Value::as_str),
            Some(definition),
            "case {case:?}: expected definition {definition:?}, got {described:?}"
        );
        assert_eq!(
            variable_references(&described),
            references,
            "case {case:?}: expected variableReferences {references}, got {described:?}"
        );
        // Everything that is not read out of the definition is unchanged by a
        // metadata-only read.
        assert_eq!(
            described.get("roleArn").and_then(Value::as_str),
            Some(ROLE_ARN),
            "case {case:?}: expected the stored roleArn, got {described:?}"
        );
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected the stored status, got {described:?}"
        );
    }

    for case in ["", "METADATA", "all_data"] {
        let (status, code) = emulator
            .err(
                "DescribeStateMachine",
                &json!({ "stateMachineArn": &with_variables, "includedData": case }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for an unmodelled includedData, \
             got {status} {code}"
        );
    }
}

/// A published VERSION is a state machine too, read from its own record: the three
/// read rules above hold for it exactly as they hold for the state machine.
#[tokio::test]
async fn describe_reports_a_published_version_by_the_same_rules() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "versioned",
                "definition": JSONATA_DEFINITION,
                "roleArn": ROLE_ARN,
                "publish": true,
            }),
        )
        .await;
    let version_arn = string(&created, "stateMachineVersionArn");

    let described = describe(&emulator, &version_arn).await;
    assert_eq!(
        described.get("definition").and_then(Value::as_str),
        Some(JSONATA_DEFINITION),
        "expected the version's own definition, got {described:?}"
    );
    for (member, expected) in [
        (
            "loggingConfiguration",
            json!({ "level": LOG_LEVEL_OFF, "includeExecutionData": false }),
        ),
        ("tracingConfiguration", json!({ "enabled": false })),
        (
            "encryptionConfiguration",
            json!({ "type": ENCRYPTION_TYPE_AWS_OWNED_KEY }),
        ),
    ] {
        assert_eq!(
            described.get(member),
            Some(&expected),
            "case {member:?}: a version reports it at its effective value {expected}, got \
             {described:?}"
        );
    }
    assert_eq!(
        variable_references(&described),
        json!({ "Compose": ["greeting", "target"], "Emit": ["message", "target"] }),
        "expected the version's variableReferences, got {described:?}"
    );

    let metadata_only = emulator
        .ok(
            "DescribeStateMachine",
            &json!({
                "stateMachineArn": &version_arn,
                "includedData": INCLUDED_DATA_METADATA_ONLY,
            }),
        )
        .await;
    assert_eq!(
        metadata_only.get("definition").and_then(Value::as_str),
        Some(METADATA_ONLY_DEFINITION),
        "expected a metadata-only read of a version to blank the definition, got \
         {metadata_only:?}"
    );
    assert_eq!(
        variable_references(&metadata_only),
        json!({}),
        "expected a metadata-only read of a version to withhold what is derived from the \
         definition, got {metadata_only:?}"
    );
}

/// An update persists exactly the members it named, retains every member it did
/// not, and mints a fresh `revisionId` that a later describe reports.
#[tokio::test]
async fn update_state_machine_persists_what_it_names_and_revises_the_record() {
    let emulator = Emulator::new();
    let arn = create_machine(&emulator, "updated", HELLO_DEFINITION).await;

    let wait_definition =
        r#"{"StartAt":"WaitState","States":{"WaitState":{"Type":"Wait","Seconds":60,"End":true}}}"#;
    let other_role = "arn:aws:iam::000000000000:role/OtherStepFunctionsRole";

    // Rows differ along WHICH member the update names: the definition, the role,
    // configuration structures beside one of the two the operation requires, and a
    // definition that introduces variable references, which the read must then
    // report. Every row names a `definition` or a `roleArn`, because an update that
    // names neither is the refusal the table in
    // `update_state_machine_refuses_what_it_cannot_serve` pins.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the definition alone",
            json!({ "definition": wait_definition }),
            json!({ "definition": wait_definition, "roleArn": ROLE_ARN }),
        ),
        (
            "the role alone",
            json!({ "roleArn": other_role }),
            json!({ "definition": wait_definition, "roleArn": other_role }),
        ),
        (
            "a logging configuration beside the role it already carries",
            json!({
                "roleArn": other_role,
                "loggingConfiguration": {
                    "level": "FATAL",
                    "includeExecutionData": true,
                    "destinations": log_destinations(),
                },
            }),
            json!({
                "definition": wait_definition,
                "roleArn": other_role,
                "loggingConfiguration": {
                    "level": "FATAL",
                    "includeExecutionData": true,
                    "destinations": log_destinations(),
                },
            }),
        ),
        (
            "tracing and encryption beside the definition it already carries",
            json!({
                "definition": wait_definition,
                "tracingConfiguration": { "enabled": true },
                "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": KMS_KEY_ARN },
            }),
            json!({
                "definition": wait_definition,
                "roleArn": other_role,
                "loggingConfiguration": {
                    "level": "FATAL",
                    "includeExecutionData": true,
                    "destinations": log_destinations(),
                },
                "tracingConfiguration": { "enabled": true },
                "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": KMS_KEY_ARN },
            }),
        ),
        (
            "a definition that introduces variable references",
            json!({ "definition": JSONATA_DEFINITION }),
            json!({
                "definition": JSONATA_DEFINITION,
                "roleArn": other_role,
                "loggingConfiguration": {
                    "level": "FATAL",
                    "includeExecutionData": true,
                    "destinations": log_destinations(),
                },
                "tracingConfiguration": { "enabled": true },
                "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": KMS_KEY_ARN },
                "variableReferences": { "Compose": ["greeting", "target"], "Emit": ["message", "target"] },
            }),
        ),
    ];

    let before = describe(&emulator, &arn).await;
    assert!(
        before.get("revisionId").is_none_or(Value::is_null),
        "expected no revisionId before any update, got {before:?}"
    );

    let mut revisions: Vec<String> = Vec::new();
    for (case, update, expected) in cases {
        let mut request = json!({ "stateMachineArn": &arn });
        for (member, value) in update.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let updated = emulator.ok("UpdateStateMachine", &request).await;
        let revision = string(&updated, "revisionId");
        assert!(
            updated.get("updateDate").is_some_and(|date| !date.is_null()),
            "case {case:?}: an update reports the date it happened, got {updated:?}"
        );
        assert!(
            !revisions.contains(&revision),
            "case {case:?}: every update mints a revisionId of its own, and {revision:?} is \
             already in {revisions:?}"
        );
        revisions.push(revision.clone());

        let described = describe(&emulator, &arn).await;
        assert_eq!(
            described.get("revisionId").and_then(Value::as_str),
            Some(revision.as_str()),
            "case {case:?}: expected the read to report revisionId {revision:?}, got {described:?}"
        );
        for (member, value) in expected.as_object().expect("an object of members") {
            assert_eq!(
                described.get(member),
                Some(value),
                "case {case:?}: expected {member} {value}, got {described:?}"
            );
        }
        // A member no update has ever named keeps the value the create stored.
        assert_eq!(
            described.get("name").and_then(Value::as_str),
            Some("updated"),
            "case {case:?}: an update never renames the state machine, got {described:?}"
        );
        assert_eq!(
            described.get("creationDate"),
            before.get("creationDate"),
            "case {case:?}: an update never moves the creationDate, got {described:?}"
        );

        // The revision a read reports is stable until the next update.
        let again = describe(&emulator, &arn).await;
        assert_eq!(
            again.get("revisionId").and_then(Value::as_str),
            Some(revision.as_str()),
            "case {case:?}: two reads of one revision must agree, got {again:?}"
        );
    }
}

/// An update this operation cannot serve is one of its own modelled errors, never a
/// 501 and never a silent success.
#[tokio::test]
async fn update_state_machine_refuses_what_it_cannot_serve() {
    let emulator = Emulator::new();
    let arn = create_machine(&emulator, "refusals", HELLO_DEFINITION).await;
    let deleting = create_machine(&emulator, "refusals-deleting", HELLO_DEFINITION).await;
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": &deleting }))
        .await;

    // Rows here are the ones NOT already covered by
    // `state_machine_create_options.rs`'s update-refusal table (the ARN forms, the
    // definition's structure, `versionDescription` without `publish` and an update
    // naming neither required member at all): the documented boundary between a
    // configuration-only update and one that names `definition` or `roleArn`, and a
    // state machine whose deletion has begun.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "only a logging configuration, naming neither definition nor roleArn",
            json!({
                "stateMachineArn": &arn,
                "loggingConfiguration": { "level": "OFF", "includeExecutionData": false },
            }),
            400,
            "MissingRequiredParameter",
        ),
        (
            "only tracing and encryption, naming neither definition nor roleArn",
            json!({
                "stateMachineArn": &arn,
                "tracingConfiguration": { "enabled": true },
                "encryptionConfiguration": { "type": "AWS_OWNED_KEY" },
            }),
            400,
            "MissingRequiredParameter",
        ),
        (
            "a state machine being deleted",
            json!({ "stateMachineArn": &deleting, "roleArn": ROLE_ARN }),
            400,
            "StateMachineDeleting",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, code) = emulator.err("UpdateStateMachine", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code}"
        );
    }

    // None of the refusals wrote anything: the record still answers for the
    // definition and role the create stored, with no revision.
    let described = describe(&emulator, &arn).await;
    assert_eq!(
        described.get("definition").and_then(Value::as_str),
        Some(HELLO_DEFINITION),
        "a refused update must leave the stored definition alone, got {described:?}"
    );
    assert!(
        described.get("revisionId").is_none_or(Value::is_null),
        "a refused update mints no revision, got {described:?}"
    );

    // Once the deletion the clock owed has happened, the update is the modelled
    // "does not exist" rather than "is being deleted".
    std::thread::sleep(PAST_THE_DELETE_DELAY);
    let (status, code) = emulator
        .err(
            "UpdateStateMachine",
            &json!({ "stateMachineArn": &deleting, "roleArn": ROLE_ARN }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "StateMachineDoesNotExist"),
        "expected 400 StateMachineDoesNotExist once the record is gone, got {status} {code}"
    );
}

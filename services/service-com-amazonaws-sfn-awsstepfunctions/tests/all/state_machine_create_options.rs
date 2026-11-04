// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the accept/reject decision `CreateStateMachine` takes on
//! its three option structures — `encryptionConfiguration`, `publish` /
//! `versionDescription`, and the configuration the idempotency check reads — driven
//! over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. an `encryptionConfiguration` is accepted only when its members agree with its
//!    `type`: under `CUSTOMER_MANAGED_KMS_KEY` a `kmsKeyId` naming a KMS key (an
//!    alias, alias ARN, key ID or key ARN) is required, under `AWS_OWNED_KEY`
//!    neither KMS member applies, and `kmsDataKeyReusePeriodSeconds` sits inside
//!    60..900 — anything else is the modelled `InvalidEncryptionConfiguration`
//!    (400) and NOTHING is stored;
//! 2. `publish` publishes version 1 as a numbered snapshot of the state machine's
//!    configuration, reachable by its own qualified ARN, and an unpublished state
//!    machine has no version at all;
//! 3. the idempotency check compares the EFFECTIVE configuration — each optional
//!    structure defaulted the way the service stores it — so a repeat that spells
//!    out a default the first request omitted is the same request, and a repeat that
//!    changes what the configuration denotes is `StateMachineAlreadyExists`.
//!
//! The harness (`Emulator`, `string`) is the one `state_machine_lifecycle` already
//! builds; `tests/all.rs` is this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS, MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
};

use crate::state_machine_lifecycle::{
    discriminated, string, Emulator, ACCOUNT, HELLO_DEFINITION, PAST_THE_DELETE_DELAY, REGION,
    ROLE_ARN,
};

/// A key ARN in the emulator's own account, one of the four forms `kmsKeyId`
/// documents.
const KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The `__type` discriminator of an awsJson error body, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A `CreateStateMachine` request for `name` carrying `options` on top of the
/// required members.
fn create_request(name: &str, options: Value) -> Value {
    let mut request = json!({
        "name": name,
        "definition": HELLO_DEFINITION,
        "roleArn": ROLE_ARN,
    });
    if let Some(members) = options.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    request
}

// ---------------------------------------------------------------------------
// encryptionConfiguration
// ---------------------------------------------------------------------------

/// An `encryptionConfiguration` is accepted only when its members agree with its
/// `type`, and a rejected one stores nothing.
#[tokio::test]
async fn create_state_machine_checks_the_encryption_configuration() {
    let emulator = Emulator::new();

    // Rows vary along the `type`, whether each KMS member is set, and the FORM of
    // the key reference. The accepted forms are the four the `kmsKeyId` member's own
    // documentation names — "an alias, alias ARN, key ID, or key ARN of a symmetric
    // encryption KMS key" — plus the `mrk-` id a multi-Region key carries; none of
    // them is the configuration the issue recorded.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "no encryptionConfiguration at all",
            json!({}),
        ),
        (
            "the AWS owned key, with no KMS member",
            json!({ "encryptionConfiguration": { "type": "AWS_OWNED_KEY" } }),
        ),
        (
            "a customer-managed key named by key ARN",
            json!({ "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": KEY_ARN } }),
        ),
        (
            "a customer-managed key named by key id",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            } }),
        ),
        (
            "a customer-managed multi-Region key id",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": "mrk-1234abcd12ab34cd56ef1234567890ab",
            } }),
        ),
        (
            "a customer-managed key named by alias",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": "alias/sfn-probe",
            } }),
        ),
        (
            "a customer-managed key named by alias ARN",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:alias/sfn-probe",
            } }),
        ),
        (
            "a customer-managed key with a reuse period inside the modelled range",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": KEY_ARN,
                "kmsDataKeyReusePeriodSeconds": 120,
            } }),
        ),
    ];

    for (index, (case, options)) in accepted.into_iter().enumerate() {
        let name = format!("encryption-ok-{index}");
        let request = create_request(&name, options.clone());
        let (status, body) = emulator.call("CreateStateMachine", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a valid encryption configuration must be accepted, got {status} with \
             body {body:?}"
        );

        // The configuration is PERSISTED, not echoed: the describe reads the record,
        // and a create that named no encryption configuration reads back the type
        // its data IS encrypted under (`tests/all/state_machine_projection.rs` pins
        // that effective-configuration rule over the whole class).
        let described = emulator
            .ok(
                "DescribeStateMachine",
                &json!({ "stateMachineArn": string(&body, "stateMachineArn") }),
            )
            .await;
        let expected = discriminated(
            "EncryptionConfiguration",
            options
                .get("encryptionConfiguration")
                .cloned()
                .unwrap_or(json!({ "type": "AWS_OWNED_KEY" })),
        );
        assert_eq!(
            described.get("encryptionConfiguration"),
            Some(&expected),
            "case {case:?}: expected the stored encryption configuration {expected}, got \
             {described:?}"
        );
    }

    // Rows vary along WHICH condition makes the configuration one the service cannot
    // apply.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a customer-managed key with no kmsKeyId",
            json!({ "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY" } }),
        ),
        (
            "a customer-managed key whose kmsKeyId names no KMS key",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": "not-a-kms-key",
            } }),
        ),
        (
            "a customer-managed key whose key ARN carries no key id",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/",
            } }),
        ),
        (
            "a customer-managed key named by an ARN of another service",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": "arn:aws:lambda:us-west-2:000000000000:function:encrypt",
            } }),
        ),
        (
            "a customer-managed key named by a blank kmsKeyId",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": " ",
            } }),
        ),
        (
            "the AWS owned key with a kmsKeyId it cannot use",
            json!({ "encryptionConfiguration": { "type": "AWS_OWNED_KEY", "kmsKeyId": KEY_ARN } }),
        ),
        (
            "the AWS owned key with a data key reuse period it cannot use",
            json!({ "encryptionConfiguration": {
                "type": "AWS_OWNED_KEY",
                "kmsDataKeyReusePeriodSeconds": 120,
            } }),
        ),
    ];

    for (index, (case, options)) in refused.into_iter().enumerate() {
        let name = format!("encryption-bad-{index}");
        let (status, body) = emulator
            .call("CreateStateMachine", &create_request(&name, options))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidEncryptionConfiguration"),
            "case {case:?}: expected 400 InvalidEncryptionConfiguration, got {status} with body \
             {body:?}"
        );
        assert!(
            body.get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.starts_with("Invalid Encryption Configuration:")),
            "case {case:?}: the refusal must say what is invalid, got {body:?}"
        );

        // NOTHING was stored: the refused create left no record behind.
        let arn = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:{name}");
        let (status, body) = emulator
            .call("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "StateMachineDoesNotExist"),
            "case {case:?}: a refused create must store nothing, got {status} with body {body:?}"
        );
    }

    // `EncryptionType` is a MODELLED enum, so a type outside it is refused by the
    // centralized request walk before the handler runs — a 400 either way, with the
    // standard validation template naming the member and the value set.
    let (status, code) = emulator
        .err(
            "CreateStateMachine",
            &create_request(
                "encryption-unknown-type",
                json!({ "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_HSM_KEY" } }),
            ),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "an encryption type the model does not declare is refused, got {status} {code}"
    );
}

/// `kmsDataKeyReusePeriodSeconds` is tested at each published bound of the modelled
/// range (60..900) and one past it: on the bounds the create is accepted, outside
/// them it is refused.
#[tokio::test]
async fn the_data_key_reuse_period_is_bounded_at_its_edges() {
    let emulator = Emulator::new();

    let inside = [
        ("the lowest reuse period the range allows", MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS),
        ("the highest reuse period the range allows", MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS),
    ];
    for (index, (case, period)) in inside.into_iter().enumerate() {
        let options = json!({ "encryptionConfiguration": {
            "type": "CUSTOMER_MANAGED_KMS_KEY",
            "kmsKeyId": KEY_ARN,
            "kmsDataKeyReusePeriodSeconds": period,
        } });
        let (status, body) = emulator
            .call(
                "CreateStateMachine",
                &create_request(&format!("reuse-ok-{index}"), options),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a reuse period of {period} must be accepted, got {status} with body \
             {body:?}"
        );
    }

    // One past each bound. The range is MODELLED, so the centralized request walk
    // may refuse the value before the handler runs — the requirement is the 400 and
    // that nothing is created, not which of the two 400s answers.
    let outside = [
        (
            "one second below the lowest reuse period",
            MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS - 1,
        ),
        (
            "one second above the highest reuse period",
            MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS + 1,
        ),
    ];
    for (index, (case, period)) in outside.into_iter().enumerate() {
        let name = format!("reuse-bad-{index}");
        let options = json!({ "encryptionConfiguration": {
            "type": "CUSTOMER_MANAGED_KMS_KEY",
            "kmsKeyId": KEY_ARN,
            "kmsDataKeyReusePeriodSeconds": period,
        } });
        let (status, body) = emulator
            .call("CreateStateMachine", &create_request(&name, options))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reuse period of {period} must be refused, got {status} with body \
             {body:?}"
        );
        let code = error_code(&body);
        assert!(
            ["InvalidEncryptionConfiguration", "ValidationException"].contains(&code.as_str()),
            "case {case:?}: expected the encryption or validation refusal, got {code} with body \
             {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// loggingConfiguration
// ---------------------------------------------------------------------------

/// One log destination naming a CloudWatch Logs log group, the form `logGroupArn`
/// documents ("the ARN must end with `:*`").
fn log_destination(log_group: &str) -> Value {
    json!({ "cloudWatchLogsLogGroup": { "logGroupArn":
        format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{log_group}:*") } })
}

/// A `loggingConfiguration` is accepted only when its destinations agree with its
/// level: "Limited to size 1. Required, if your log level is not set to OFF."
#[tokio::test]
async fn create_state_machine_checks_the_logging_configuration() {
    let emulator = Emulator::new();

    // Rows walk EVERY level the modelled `LogLevel` enum declares (ALL, ERROR,
    // FATAL, OFF — `smithy_query(query="shape", shape="com.amazonaws.sfn#LogLevel")`),
    // each with the destinations that level needs, plus the configuration a request
    // that logs nothing may leave empty.
    let accepted: Vec<(&str, Value)> = vec![
        ("no loggingConfiguration at all", json!({})),
        (
            "the OFF level with no destination",
            json!({ "loggingConfiguration": { "level": "OFF" } }),
        ),
        (
            "the ALL level with one destination",
            json!({ "loggingConfiguration": {
                "level": "ALL",
                "includeExecutionData": true,
                "destinations": [log_destination("/aws/vendedlogs/all")],
            } }),
        ),
        (
            "the ERROR level with one destination",
            json!({ "loggingConfiguration": {
                "level": "ERROR",
                "destinations": [log_destination("/aws/vendedlogs/error")],
            } }),
        ),
        (
            "the FATAL level with one destination",
            json!({ "loggingConfiguration": {
                "level": "FATAL",
                "destinations": [log_destination("/aws/vendedlogs/fatal")],
            } }),
        ),
    ];
    for (index, (case, options)) in accepted.into_iter().enumerate() {
        let name = format!("logging-ok-{index}");
        let (status, body) = emulator
            .call("CreateStateMachine", &create_request(&name, options.clone()))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a valid logging configuration must be accepted, got {status} with \
             body {body:?}"
        );
        let described = emulator
            .ok(
                "DescribeStateMachine",
                &json!({ "stateMachineArn": string(&body, "stateMachineArn") }),
            )
            .await;
        // The record, not the request: `includeExecutionData` is a modelled default
        // member, so the response always carries it, and the members the request set
        // are compared one by one. A create that named no logging configuration reads
        // back the level it EFFECTIVELY logs at, which is `OFF`
        // (`tests/all/state_machine_projection.rs` pins that rule over the class).
        let supplied = options.get("loggingConfiguration");
        let stored = described.get("loggingConfiguration");
        let expected_level = supplied
            .and_then(|supplied| supplied.get("level"))
            .cloned()
            .unwrap_or(json!("OFF"));
        assert_eq!(
            stored.and_then(|stored| stored.get("level")),
            Some(&expected_level),
            "case {case:?}: expected the stored log level {expected_level}, got {described:?}"
        );
        assert_eq!(
            stored.and_then(|stored| stored.get("destinations")),
            supplied.and_then(|supplied| supplied.get("destinations")),
            "case {case:?}: expected the stored destinations, got {described:?}"
        );
    }

    // Rows vary along WHICH condition makes the configuration one the service cannot
    // apply: the destination count against the level, and the form of the log group
    // ARN.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a level that logs with no destinations member",
            json!({ "loggingConfiguration": { "level": "ALL" } }),
        ),
        (
            "a level that logs with an empty destinations list",
            json!({ "loggingConfiguration": { "level": "ERROR", "destinations": [] } }),
        ),
        (
            "two destinations, past the size the member allows",
            json!({ "loggingConfiguration": {
                "level": "FATAL",
                "destinations": [log_destination("/one"), log_destination("/two")],
            } }),
        ),
        (
            "two destinations while the level is OFF",
            json!({ "loggingConfiguration": {
                "level": "OFF",
                "destinations": [log_destination("/one"), log_destination("/two")],
            } }),
        ),
        (
            "a destination naming no log group",
            json!({ "loggingConfiguration": { "level": "ALL", "destinations": [{}] } }),
        ),
        (
            "a log group ARN that does not end with ':*'",
            json!({ "loggingConfiguration": {
                "level": "ALL",
                "destinations": [{ "cloudWatchLogsLogGroup": { "logGroupArn":
                    format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:/sfn") } }],
            } }),
        ),
        (
            "a log group ARN of another service",
            json!({ "loggingConfiguration": {
                "level": "ALL",
                "destinations": [{ "cloudWatchLogsLogGroup": { "logGroupArn":
                    format!("arn:aws:s3:{REGION}:{ACCOUNT}:log-group:/sfn:*") } }],
            } }),
        ),
    ];
    for (index, (case, options)) in refused.into_iter().enumerate() {
        let name = format!("logging-bad-{index}");
        let (status, body) = emulator
            .call("CreateStateMachine", &create_request(&name, options))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLoggingConfiguration"),
            "case {case:?}: expected 400 InvalidLoggingConfiguration, got {status} with body \
             {body:?}"
        );
        assert!(
            body.get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.starts_with("Invalid Logging Configuration:")),
            "case {case:?}: the refusal must say what is invalid, got {body:?}"
        );

        // NOTHING was stored: the refused create left no record behind.
        let arn = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:{name}");
        let (status, body) = emulator
            .call("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "StateMachineDoesNotExist"),
            "case {case:?}: a refused create must store nothing, got {status} with body {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// publish
// ---------------------------------------------------------------------------

/// `publish` publishes version 1 as a snapshot of the state machine, reachable by
/// its own qualified ARN; without it no version exists.
#[tokio::test]
async fn publish_mints_version_one_and_a_describe_reads_it_back() {
    let emulator = Emulator::new();

    // Rows vary along the workflow type, the definition, and whether the create also
    // set a `versionDescription` — the member that only a published create may
    // carry.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a STANDARD machine published with no description",
            json!({ "name": "published-standard", "publish": true }),
        ),
        (
            "an EXPRESS machine published with a description",
            json!({
                "name": "published-express",
                "type": "EXPRESS",
                "publish": true,
                "versionDescription": "the first cut",
            }),
        ),
        (
            "a published machine whose definition uses a state this emulator cannot run",
            json!({
                "name": "published-task",
                "definition": r#"{"StartAt":"Work","States":{"Work":{"Type":"Task","Resource":"arn:aws:states:::lambda:invoke","End":true}}}"#,
                "publish": true,
            }),
        ),
    ];

    for (case, options) in cases {
        let name = string(&options, "name");
        let request = create_request(&name, options.clone());
        let created = emulator.ok("CreateStateMachine", &request).await;
        let arn = string(&created, "stateMachineArn");
        assert_eq!(
            created.get("stateMachineVersionArn").and_then(Value::as_str),
            Some(format!("{arn}:1").as_str()),
            "case {case:?}: publish must report version 1 of the created machine, got {created:?}"
        );

        // The version is a STORED snapshot: a describe of the qualified ARN answers
        // from it, with the configuration the state machine was published with.
        let version_arn = string(&created, "stateMachineVersionArn");
        let described = emulator
            .ok(
                "DescribeStateMachine",
                &json!({ "stateMachineArn": version_arn }),
            )
            .await;
        assert_eq!(
            described.get("stateMachineArn").and_then(Value::as_str),
            Some(version_arn.as_str()),
            "case {case:?}: a version describes itself by its own ARN, got {described:?}"
        );
        assert_eq!(
            described.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: a version carries the state machine's name, got {described:?}"
        );
        assert_eq!(
            described.get("definition"),
            request.get("definition"),
            "case {case:?}: a version carries the published definition, got {described:?}"
        );
        assert_eq!(
            described.get("type").and_then(Value::as_str),
            Some(request.get("type").and_then(Value::as_str).unwrap_or("STANDARD")),
            "case {case:?}: a version carries the published type, got {described:?}"
        );
        assert_eq!(
            described.get("description").and_then(Value::as_str),
            request.get("versionDescription").and_then(Value::as_str),
            "case {case:?}: a version carries its versionDescription, got {described:?}"
        );

        // Only ONE version was published, and the next number is not taken yet.
        let (status, body) = emulator
            .call(
                "DescribeStateMachine",
                &json!({ "stateMachineArn": format!("{arn}:2") }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "StateMachineDoesNotExist"),
            "case {case:?}: no second version was published, got {status} with body {body:?}"
        );

        // An idempotent repeat answers the FIRST response — the same version ARN —
        // rather than publishing another version.
        let repeat = emulator.ok("CreateStateMachine", &request).await;
        assert_eq!(
            repeat, created,
            "case {case:?}: a repeat of a published create must answer the first response"
        );
    }

    // Without `publish` there is no version at all, and the qualified ARN names
    // nothing.
    let unpublished = emulator
        .ok(
            "CreateStateMachine",
            &create_request("unpublished", json!({})),
        )
        .await;
    assert!(
        unpublished
            .get("stateMachineVersionArn")
            .is_none_or(Value::is_null),
        "a create without publish reports no version ARN, got {unpublished:?}"
    );
    let arn = string(&unpublished, "stateMachineArn");
    let (status, body) = emulator
        .call(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": format!("{arn}:1") }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "StateMachineDoesNotExist"),
        "an unpublished machine has no version 1, got {status} with body {body:?}"
    );
}

/// A version goes with the state machine it belongs to: once the deletion the clock
/// owed has happened, the version ARN names nothing.
#[tokio::test]
async fn deleting_a_state_machine_deletes_its_versions() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateStateMachine",
            &create_request("published-then-deleted", json!({ "publish": true })),
        )
        .await;
    let arn = string(&created, "stateMachineArn");
    let version_arn = string(&created, "stateMachineVersionArn");

    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": arn }))
        .await;
    std::thread::sleep(PAST_THE_DELETE_DELAY);

    let (status, body) = emulator
        .call(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": version_arn }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "StateMachineDoesNotExist"),
        "a deleted state machine keeps no version, got {status} with body {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The idempotency check reads the EFFECTIVE configuration
// ---------------------------------------------------------------------------
/// A repeat that spells out a default the first request omitted is the SAME request;
/// a repeat that changes what the configuration denotes is a conflict.
#[tokio::test]
async fn idempotency_compares_the_effective_configuration() {
    let emulator = Emulator::new();
    let first = emulator
        .ok(
            "CreateStateMachine",
            &create_request("effective", json!({})),
        )
        .await;

    // Rows vary along WHICH default is spelled out: the logging level ("By default,
    // the level is set to OFF"), `includeExecutionData` and `enabled` (modelled
    // default false), and the encryption type (the AWS owned key, which is what a
    // state machine with no encryptionConfiguration is encrypted under). The last row
    // is the whole set at once — what reading a DescribeStateMachine response back
    // into a create produces.
    let same: Vec<(&str, Value)> = vec![
        (
            "an explicit OFF logging level",
            json!({ "loggingConfiguration": { "level": "OFF", "includeExecutionData": false } }),
        ),
        (
            "an explicit logging configuration with no level",
            json!({ "loggingConfiguration": { "includeExecutionData": false } }),
        ),
        (
            "an explicit disabled tracing configuration",
            json!({ "tracingConfiguration": { "enabled": false } }),
        ),
        (
            "an explicit AWS owned key",
            json!({ "encryptionConfiguration": { "type": "AWS_OWNED_KEY" } }),
        ),
        (
            "every default spelled out at once",
            json!({
                "loggingConfiguration": { "level": "OFF", "includeExecutionData": false, "destinations": [] },
                "tracingConfiguration": { "enabled": false },
                "encryptionConfiguration": { "type": "AWS_OWNED_KEY" },
            }),
        ),
    ];
    for (case, options) in same {
        let repeat = emulator
            .ok("CreateStateMachine", &create_request("effective", options))
            .await;
        assert_eq!(
            repeat, first,
            "case {case:?}: a repeat that spells out a default must answer the first response"
        );
    }

    // Rows vary along which member changes what the configuration DENOTES.
    let conflicting: Vec<(&str, Value)> = vec![
        (
            "a logging level that is not the stored default",
            json!({ "loggingConfiguration": {
                "level": "ALL",
                "includeExecutionData": false,
                "destinations": [log_destination("/conflict")],
            } }),
        ),
        (
            "execution data the stored configuration does not log",
            json!({ "loggingConfiguration": { "level": "OFF", "includeExecutionData": true } }),
        ),
        (
            "tracing the stored configuration does not enable",
            json!({ "tracingConfiguration": { "enabled": true } }),
        ),
        (
            "a customer-managed key where the stored machine uses the AWS owned key",
            json!({ "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": KEY_ARN,
            } }),
        ),
        (
            "a published version the stored machine never had",
            json!({ "publish": true }),
        ),
    ];
    for (case, options) in conflicting {
        let (status, code) = emulator
            .err("CreateStateMachine", &create_request("effective", options))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "StateMachineAlreadyExists"),
            "case {case:?}: expected 400 StateMachineAlreadyExists, got {status} {code}"
        );
    }
}

/// The idempotency check compares a repeat against the state machine's CURRENT
/// configuration, so a repeat of the request that created it stops being idempotent
/// once an update has moved that configuration, and a repeat carrying the UPDATED
/// configuration is the same request.
#[tokio::test]
async fn idempotency_follows_the_updated_configuration() {
    let emulator = Emulator::new();
    let created_request = create_request("updated", json!({}));
    let created = emulator.ok("CreateStateMachine", &created_request).await;
    let arn = string(&created, "stateMachineArn");

    // Rows vary along what the update changes: the definition alone, then the
    // definition together with a configuration structure.
    let updates: Vec<(&str, Value, Value)> = vec![
        (
            "a new definition",
            json!({ "stateMachineArn": arn, "definition":
                r#"{"StartAt":"WaitState","States":{"WaitState":{"Type":"Wait","Seconds":60,"End":true}}}"# }),
            json!({ "definition":
                r#"{"StartAt":"WaitState","States":{"WaitState":{"Type":"Wait","Seconds":60,"End":true}}}"# }),
        ),
        (
            "a new definition and an enabled tracing configuration",
            json!({
                "stateMachineArn": arn,
                "definition": r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#,
                "tracingConfiguration": { "enabled": true },
            }),
            json!({
                "definition": r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#,
                "tracingConfiguration": { "enabled": true },
            }),
        ),
    ];

    let mut revisions: Vec<String> = Vec::new();
    for (case, update, repeat_options) in updates {
        let updated = emulator.ok("UpdateStateMachine", &update).await;
        let revision = string(&updated, "revisionId");
        assert!(
            !revisions.contains(&revision),
            "case {case:?}: every update must move the revisionId, got {revision:?} again"
        );
        revisions.push(revision.clone());
        assert!(
            updated.get("stateMachineVersionArn").is_none_or(Value::is_null),
            "case {case:?}: an update without publish reports no version ARN, got {updated:?}"
        );

        // The update is PERSISTED: the describe reads the record it wrote.
        let described = emulator
            .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
            .await;
        assert_eq!(
            described.get("definition"),
            update.get("definition"),
            "case {case:?}: expected the updated definition, got {described:?}"
        );
        assert_eq!(
            described.get("revisionId").and_then(Value::as_str),
            Some(revision.as_str()),
            "case {case:?}: expected the update's revisionId, got {described:?}"
        );

        // The request that CREATED the state machine is no longer the same request.
        let (status, code) = emulator
            .err("CreateStateMachine", &created_request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "StateMachineAlreadyExists"),
            "case {case:?}: the create-time request no longer matches, got {status} {code}"
        );

        // A create carrying the configuration the update wrote is idempotent, and it
        // answers the state machine's own ARN and creationDate.
        let again = emulator
            .ok(
                "CreateStateMachine",
                &create_request("updated", repeat_options),
            )
            .await;
        assert_eq!(
            again, created,
            "case {case:?}: a repeat of the updated configuration must answer the create"
        );
    }

    // An update may publish in the same action: "After you update your state
    // machine, you can set the publish parameter to true in the same action to
    // publish a new version." The numbers increase, starting at 1.
    let published = emulator
        .ok(
            "UpdateStateMachine",
            &json!({
                "stateMachineArn": arn,
                "definition": HELLO_DEFINITION,
                "publish": true,
                "versionDescription": "back to hello",
            }),
        )
        .await;
    assert_eq!(
        string(&published, "stateMachineVersionArn"),
        format!("{arn}:1"),
        "the first published version of this state machine is version 1, got {published:?}"
    );
    let republished = emulator
        .ok(
            "UpdateStateMachine",
            &json!({ "stateMachineArn": arn, "definition": HELLO_DEFINITION, "publish": true }),
        )
        .await;
    assert_eq!(
        string(&republished, "stateMachineVersionArn"),
        format!("{arn}:2"),
        "the next published version takes the next number, got {republished:?}"
    );

    // Each version is its own stored snapshot, read back through the describe.
    let version_one = emulator
        .ok(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": format!("{arn}:1") }),
        )
        .await;
    assert_eq!(
        version_one.get("description").and_then(Value::as_str),
        Some("back to hello"),
        "version 1 carries the description its publish gave it, got {version_one:?}"
    );

    // Rows the update refuses, each for its own documented reason.
    let refused: Vec<(&str, Value, &str)> = vec![
        (
            "neither definition nor roleArn",
            json!({ "stateMachineArn": arn }),
            "MissingRequiredParameter",
        ),
        (
            "a definition that breaks the Amazon States Language's own rules",
            json!({ "stateMachineArn": arn, "definition": r#"{"States":{}}"# }),
            "InvalidDefinition",
        ),
        (
            "a roleArn that is not an IAM role ARN",
            json!({ "stateMachineArn": arn, "roleArn": "not-an-arn" }),
            "InvalidArn",
        ),
        (
            "an encryption configuration the service cannot apply",
            json!({
                "stateMachineArn": arn,
                "definition": HELLO_DEFINITION,
                "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY" },
            }),
            "InvalidEncryptionConfiguration",
        ),
        (
            "a logging configuration with no destination to log to",
            json!({
                "stateMachineArn": arn,
                "definition": HELLO_DEFINITION,
                "loggingConfiguration": { "level": "ALL" },
            }),
            "InvalidLoggingConfiguration",
        ),
        (
            "a versionDescription without publish",
            json!({
                "stateMachineArn": arn,
                "definition": HELLO_DEFINITION,
                "versionDescription": "unpublished",
            }),
            "ValidationException",
        ),
        (
            // A version ARN names the immutable snapshot rather than the state
            // machine record this call updates, so it names no state machine to
            // update at all — the same classification an ARN nothing answers for
            // gets (`tests/all/control_plane_refusal_classes.rs` is the rule).
            "a version ARN, which is an immutable snapshot",
            json!({ "stateMachineArn": format!("{arn}:1"), "definition": HELLO_DEFINITION }),
            "StateMachineDoesNotExist",
        ),
        (
            "a state machine that does not exist",
            json!({
                "stateMachineArn": format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:absent"),
                "definition": HELLO_DEFINITION,
            }),
            "StateMachineDoesNotExist",
        ),
        (
            "an ARN that is not a state machine ARN",
            json!({ "stateMachineArn": "not-an-arn", "definition": HELLO_DEFINITION }),
            "InvalidArn",
        ),
    ];
    for (case, request, expected) in refused {
        let (status, code) = emulator.err("UpdateStateMachine", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} {code}"
        );
    }

    // A refused update changed nothing: the state machine still reports the last
    // definition an accepted update wrote.
    let described = emulator
        .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
        .await;
    assert_eq!(
        described.get("definition").and_then(Value::as_str),
        Some(HELLO_DEFINITION),
        "a refused update must leave the record as it was, got {described:?}"
    );
}

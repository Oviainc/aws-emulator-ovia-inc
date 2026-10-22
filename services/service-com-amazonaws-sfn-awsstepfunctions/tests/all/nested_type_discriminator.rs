// SPDX-License-Identifier: CC0-1.0

//! The awsJson type discriminator this service puts on a NESTED structure of a
//! response body, driven over its own wire protocol (`awsJson1_0`, `POST /`
//! with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rule under test: a response body serializes a structure AWS shares with
//! its SWF base model — `LoggingConfiguration`, `TracingConfiguration`,
//! `EncryptionConfiguration` and `CloudWatchEventsExecutionDataDetails`, the
//! four shapes of `service::wire::DISCRIMINATED_SHAPES` — with a `__type` member
//! carrying that shape's id in `com.amazonaws.swf.base.model`, beside the
//! structure's own members, wherever in whatever operation's response it occurs;
//! the response's own top-level body and every other structure it nests (a list
//! item, a log destination) carry none.
//!
//! Each rule is asserted over a TABLE spanning its class — every modelled log
//! level and encryption type, a state machine read both unqualified and by
//! version ARN, an execution read while running and once terminal — and the
//! catalog the serializer resolves a member through is re-derived from this
//! crate's own Smithy model, so a model that grows a member targeting one of
//! those four shapes fails here rather than silently losing its discriminator.
//!
//! The harness (`Emulator`, `create_machine`, `error_code`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's
//! crate root, so it is reached through `crate::state_machine_lifecycle`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::service::wire;
use service_com_amazonaws_sfn_awsstepfunctions::state::EXECUTION_SETTLE_DELAY;

use crate::state_machine_lifecycle::{
    create_machine, error_code, string, Emulator, HELLO_DEFINITION, ROLE_ARN,
};

/// This crate's Smithy model — the source the catalog in `service::wire` is a
/// transcription of, read back here so the two cannot drift.
const MODEL_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-sfn-awsstepfunctions.json"
);

/// Long enough that an execution's terminal status is due.
const PAST_THE_EXECUTION_DELAY: Duration =
    EXECUTION_SETTLE_DELAY.saturating_add(Duration::from_millis(300));

/// A definition that waits far longer than any test, so its execution stays
/// non-terminal.
const SLOW_DEFINITION: &str =
    r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":600,"Next":"Done"},"Done":{"Type":"Succeed"}}}"#;

/// A definition that fails immediately, so the execution is terminal with no output.
const FAIL_DEFINITION: &str =
    r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail","Error":"Boom","Cause":"on purpose"}}}"#;

/// A definition that carries its input through to its output.
const ECHO_DEFINITION: &str = r#"{"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}"#;

/// Every value `LogLevel` declares, from the model
/// (`smithy_query(query="shape",shape="LogLevel")` -> `["ALL","ERROR","FATAL","OFF"]`).
const LOG_LEVELS: &[&str] = &["ALL", "ERROR", "FATAL", "OFF"];

/// Every value `EncryptionType` declares, from the model
/// (`smithy_query(query="shape",shape="EncryptionType")`).
const ENCRYPTION_TYPES: &[&str] = &["AWS_OWNED_KEY", "CUSTOMER_MANAGED_KMS_KEY"];

/// A KMS key ARN inside the modelled `kmsKeyId` length (1..2048).
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The one log destination a logging configuration may carry (`destinations` is
/// "limited to size 1", and required at any level other than OFF).
fn log_destinations() -> Value {
    json!([{
        "cloudWatchLogsLogGroup": {
            "logGroupArn": "arn:aws:logs:us-west-2:000000000000:log-group:/sfn:*"
        }
    }])
}

/// Assert that `body`'s `member` is a structure carrying the discriminator for
/// `shape` beside at least one member of its own.
fn assert_discriminated(case: &str, body: &Value, member: &str, shape: &str) {
    let structure = body
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} structure, got {body:?}"));
    let expected = wire::discriminator(shape);
    assert_eq!(
        structure.get("__type").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {case:?}: expected {member}.__type {expected:?}, got {structure:?}"
    );
    let own_members = structure
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} to be an object, got {structure:?}"))
        .keys()
        .filter(|key| key.as_str() != "__type")
        .count();
    assert!(
        own_members > 0,
        "case {case:?}: the discriminator stands BESIDE the structure's own members, \
         and {member} carries none: {structure:?}"
    );
}

/// Every `__type` value anywhere inside `body`, paired with the JSON path it sits at.
fn discriminators(body: &Value) -> Vec<(String, String)> {
    fn walk(path: &str, value: &Value, found: &mut Vec<(String, String)>) {
        match value {
            Value::Object(members) => {
                for (name, member) in members {
                    if name == "__type" {
                        found.push((
                            path.to_string(),
                            member.as_str().unwrap_or("<not a string>").to_string(),
                        ));
                    } else {
                        walk(&format!("{path}.{name}"), member, found);
                    }
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(&format!("{path}[{index}]"), item, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk("$", body, &mut found);
    found
}

// ---------------------------------------------------------------------------
// The catalog against its source
// ---------------------------------------------------------------------------

/// The serializer is handed a structure MEMBER's schema, not its target's, so it
/// resolves the member through `wire::DISCRIMINATED_MEMBERS`. That catalog is a
/// transcription of this crate's Smithy model, and this re-derives it from the
/// model: every member in the model targeting one of the discriminated shapes
/// must be listed with that shape, every listed row must exist in the model, and
/// every discriminated shape must be a structure the model declares.
#[test]
fn the_catalog_of_discriminated_members_is_the_model_s_own() {
    let source = std::fs::read_to_string(MODEL_PATH)
        .unwrap_or_else(|error| panic!("the Smithy model at {MODEL_PATH} must be readable: {error}"));
    let model: Value = serde_json::from_str(&source).expect("the Smithy model parses as JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the Smithy model carries a shapes map");

    // Every (member name, target shape) pair the model declares into one of the
    // discriminated shapes.
    let mut from_model: Vec<(String, String)> = Vec::new();
    for (shape_id, shape) in shapes {
        if shape.get("type").and_then(Value::as_str) != Some("structure") {
            continue;
        }
        let Some(members) = shape.get("members").and_then(Value::as_object) else {
            continue;
        };
        for (member_name, member) in members {
            let Some(target) = member.get("target").and_then(Value::as_str) else {
                continue;
            };
            let local = target.rsplit('#').next().unwrap_or(target);
            if !wire::DISCRIMINATED_SHAPES.contains(&local) {
                continue;
            }
            let case = format!("{shape_id}${member_name}");
            assert!(
                wire::DISCRIMINATED_MEMBERS
                    .iter()
                    .any(|(name, shape)| *name == member_name.as_str() && *shape == local),
                "case {case:?}: the model targets {local} from member {member_name:?}, which \
                 wire::DISCRIMINATED_MEMBERS does not carry as ({member_name:?}, {local:?}); \
                 got {:?}",
                wire::DISCRIMINATED_MEMBERS
            );
            from_model.push((member_name.clone(), local.to_string()));
        }
    }

    for (member_name, shape) in wire::DISCRIMINATED_MEMBERS {
        assert!(
            from_model
                .iter()
                .any(|(name, target)| name == member_name && target == shape),
            "case {member_name:?}: the catalog claims it targets {shape}, which no structure \
             member of the model does"
        );
    }

    for shape in wire::DISCRIMINATED_SHAPES {
        let suffix = format!("#{shape}");
        assert!(
            shapes.iter().any(|(id, declared)| id.ends_with(&suffix)
                && declared.get("type").and_then(Value::as_str) == Some("structure")),
            "case {shape:?}: a discriminated shape must be a structure the model declares"
        );
    }
}

// ---------------------------------------------------------------------------
// DescribeStateMachine: the three configuration structures
// ---------------------------------------------------------------------------

/// Whatever a create configured, and whether the read names the state machine or
/// one of its versions, each of the three configuration structures a describe
/// reports carries its own discriminator.
#[tokio::test]
async fn describe_state_machine_discriminates_every_configuration_structure() {
    let emulator = Emulator::new();

    // Rows differ along WHICH configurations the create named and what each held:
    // none at all, each one alone, and all three at once.
    let mut cases: Vec<(String, Value)> = vec![
        ("no configuration at all".to_string(), json!({})),
        (
            "tracing enabled".to_string(),
            json!({ "tracingConfiguration": { "enabled": true } }),
        ),
        (
            "all three at once".to_string(),
            json!({
                "loggingConfiguration": {
                    "level": "ALL",
                    "includeExecutionData": true,
                    "destinations": log_destinations(),
                },
                "tracingConfiguration": { "enabled": true },
                "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": KMS_KEY_ARN },
            }),
        ),
    ];
    // One row per published value of each of the two enums, so the rule is pinned
    // on every modelled value and not only on the recorded one.
    for level in LOG_LEVELS {
        let mut logging = json!({ "level": level, "includeExecutionData": false });
        if *level != "OFF" {
            logging["destinations"] = log_destinations();
        }
        cases.push((
            format!("the modelled log level {level}"),
            json!({ "loggingConfiguration": logging }),
        ));
    }
    for encryption_type in ENCRYPTION_TYPES {
        let encryption = if *encryption_type == "AWS_OWNED_KEY" {
            json!({ "type": encryption_type })
        } else {
            json!({ "type": encryption_type, "kmsKeyId": KMS_KEY_ARN })
        };
        cases.push((
            format!("the modelled encryption type {encryption_type}"),
            json!({ "encryptionConfiguration": encryption }),
        ));
    }

    for (index, (case, configuration)) in cases.iter().enumerate() {
        let mut request = json!({
            "name": format!("discriminated-{index}"),
            "definition": HELLO_DEFINITION,
            "roleArn": ROLE_ARN,
            // Publishing gives the SAME record a second, version-qualified ARN,
            // which `DescribeStateMachine` answers from its own stored version.
            "publish": true,
        });
        for (member, value) in configuration.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let created = emulator.ok("CreateStateMachine", &request).await;
        let arn = string(&created, "stateMachineArn");
        let version_arn = string(&created, "stateMachineVersionArn");

        for (read, read_arn) in [("unqualified", &arn), ("version-qualified", &version_arn)] {
            let described = emulator
                .ok("DescribeStateMachine", &json!({ "stateMachineArn": read_arn }))
                .await;
            let case = format!("{case} read {read}");
            for (member, shape) in [
                ("loggingConfiguration", "LoggingConfiguration"),
                ("tracingConfiguration", "TracingConfiguration"),
                ("encryptionConfiguration", "EncryptionConfiguration"),
            ] {
                assert_discriminated(&case, &described, member, shape);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DescribeExecution: the execution data details
// ---------------------------------------------------------------------------

/// `inputDetails` and `outputDetails` are the same shape, so both carry the same
/// discriminator — whichever of them the execution's state puts in the body.
#[tokio::test]
async fn describe_execution_discriminates_both_execution_data_details() {
    let emulator = Emulator::new();

    // Rows differ along the execution's STATE, which is what decides whether the
    // body carries an output beside its input: still running, succeeded with a
    // static result, succeeded carrying its input through, and failed.
    let cases: [(&str, &str, &str, bool); 4] = [
        ("a run still in flight", SLOW_DEFINITION, "{}", false),
        ("a succeeded run", HELLO_DEFINITION, "{}", true),
        (
            "a succeeded run that echoes a larger input",
            ECHO_DEFINITION,
            r#"{"probe":true,"items":[1,2,3]}"#,
            true,
        ),
        ("a failed run", FAIL_DEFINITION, "{}", false),
    ];

    for (index, (case, definition, input, expect_output)) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("details-{index}"), definition).await;
        let started = emulator
            .ok(
                "StartExecution",
                &json!({ "stateMachineArn": &machine, "name": "run", "input": input }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);

        let described = emulator
            .ok("DescribeExecution", &json!({ "executionArn": &execution }))
            .await;
        assert_discriminated(
            case,
            &described,
            "inputDetails",
            "CloudWatchEventsExecutionDataDetails",
        );
        // "Indicates whether input or output was included in the response. Always
        // true for API calls": the discriminator never displaces the member.
        assert_eq!(
            described["inputDetails"].get("included").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected inputDetails.included true beside the discriminator, \
             got {described:?}"
        );
        let has_output_details = described
            .get("outputDetails")
            .is_some_and(|details| !details.is_null());
        assert_eq!(
            has_output_details, *expect_output,
            "case {case:?}: outputDetails accompanies an output and only an output, got \
             {described:?}"
        );
        if *expect_output {
            assert_discriminated(
                case,
                &described,
                "outputDetails",
                "CloudWatchEventsExecutionDataDetails",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The boundary: what carries NO discriminator
// ---------------------------------------------------------------------------

/// The discriminator covers the four shapes the recordings show it on and
/// nothing else: not the response's own top-level body (the operation names that
/// shape), not a list item, and not any other structure a body nests.
#[tokio::test]
async fn nothing_outside_the_recorded_shapes_carries_a_discriminator() {
    let emulator = Emulator::new();

    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "boundary",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "loggingConfiguration": {
                    "level": "ALL",
                    "includeExecutionData": true,
                    "destinations": log_destinations(),
                },
                "tags": [{ "key": "team", "value": "platform" }],
            }),
        )
        .await;
    let arn = string(&created, "stateMachineArn");
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": &arn, "name": "run", "input": "{}" }),
        )
        .await;
    let execution = string(&started, "executionArn");
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    // Rows span the implemented responses: the two reads the discriminator applies
    // to, the two writes, and the three list reads whose bodies are collections of
    // this service's OWN item shapes.
    let bodies: Vec<(&str, Value)> = vec![
        ("CreateStateMachine", created.clone()),
        ("StartExecution", started.clone()),
        (
            "DescribeStateMachine",
            emulator
                .ok("DescribeStateMachine", &json!({ "stateMachineArn": &arn }))
                .await,
        ),
        (
            "DescribeExecution",
            emulator
                .ok("DescribeExecution", &json!({ "executionArn": &execution }))
                .await,
        ),
        (
            "ListStateMachines",
            emulator.ok("ListStateMachines", &json!({})).await,
        ),
        (
            "ListExecutions",
            emulator
                .ok("ListExecutions", &json!({ "stateMachineArn": &arn }))
                .await,
        ),
        (
            "ListTagsForResource",
            emulator
                .ok("ListTagsForResource", &json!({ "resourceArn": &arn }))
                .await,
        ),
    ];

    for (case, body) in &bodies {
        // The top-level body never carries one: the operation names its shape.
        assert!(
            body.get("__type").is_none(),
            "case {case:?}: a success body carries no top-level __type, got {body:?}"
        );
        // And every discriminator anywhere inside it names one of the four shapes,
        // in the SWF base-model namespace.
        for (path, value) in discriminators(body) {
            let shape = value
                .strip_prefix(&format!("{}#", wire::SWF_BASE_MODEL_NAMESPACE))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {path} carries __type {value:?}, which is not in the SWF \
                         base-model namespace"
                    )
                });
            assert!(
                wire::DISCRIMINATED_SHAPES.contains(&shape),
                "case {case:?}: {path} carries __type {value:?}, and {shape} is not one of the \
                 shapes the recordings show a discriminator on: {:?}",
                wire::DISCRIMINATED_SHAPES
            );
        }
    }

    // Each list body's items are this service's own item shapes, which carry none
    // even though their enclosing response is serialized through the same path.
    for (case, body, member) in [
        ("ListStateMachines", &bodies[4].1, "stateMachines"),
        ("ListExecutions", &bodies[5].1, "executions"),
        ("ListTagsForResource", &bodies[6].1, "tags"),
    ] {
        let items = body
            .get(member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a {member} list, got {body:?}"));
        assert!(
            !items.is_empty(),
            "case {case:?}: the boundary is only pinned by a NON-empty {member}, got {body:?}"
        );
        for (index, item) in items.iter().enumerate() {
            assert!(
                item.get("__type").is_none(),
                "case {case:?}: {member}[{index}] carries no discriminator, got {item:?}"
            );
        }
    }

    // The one other structure a describe nests — a log destination and the log
    // group inside it — carries none either.
    let described = &bodies[2].1;
    let destination = described
        .pointer("/loggingConfiguration/destinations/0")
        .unwrap_or_else(|| panic!("expected a log destination, got {described:?}"));
    assert!(
        destination.get("__type").is_none(),
        "a log destination carries no discriminator, got {destination:?}"
    );
    let log_group = destination
        .get("cloudWatchLogsLogGroup")
        .unwrap_or_else(|| panic!("expected a log group, got {destination:?}"));
    assert!(
        log_group.get("__type").is_none(),
        "a log group carries no discriminator, got {log_group:?}"
    );
}

/// The envelope the dispatch sequence renders a MODELED failure in is untouched:
/// an error body still carries the top-level `__type` naming its own shape, which
/// is the one `__type` this protocol does not write itself.
#[tokio::test]
async fn an_error_body_still_names_its_own_shape() {
    let emulator = Emulator::new();

    // Rows differ along the operation and the defect: an unknown resource and a
    // malformed ARN, on both of the reads this suite covers.
    let cases: [(&str, Value, &str); 4] = [
        (
            "DescribeStateMachine",
            json!({ "stateMachineArn": "arn:aws:states:us-west-2:000000000000:stateMachine:absent" }),
            "StateMachineDoesNotExist",
        ),
        (
            "DescribeStateMachine",
            json!({ "stateMachineArn": "not-an-arn" }),
            "InvalidArn",
        ),
        (
            "DescribeExecution",
            json!({ "executionArn": "arn:aws:states:us-west-2:000000000000:execution:sm:absent" }),
            "ExecutionDoesNotExist",
        ),
        (
            "DescribeExecution",
            json!({ "executionArn": "not-an-arn" }),
            "InvalidArn",
        ),
    ];

    for (operation, request, expected) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected),
            "case {operation} {request}: expected 400 {expected}, got {status} with {body:?}"
        );
        assert!(
            body.get("__type").is_some(),
            "case {operation} {request}: an error body names its own shape in __type, got {body:?}"
        );
    }
}

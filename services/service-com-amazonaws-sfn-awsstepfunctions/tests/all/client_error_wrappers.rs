// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WRAPPER a client-error message puts around the
//! DIAGNOSTIC or REASON it reports, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`, so what is asserted is what a
//! signed AWS SDK request observes in `$.message`.
//!
//! `error_messages.rs` owns the rule for an error that reports one VALUE of the
//! request (`<Condition>: '<value>'`) and the validation envelope; this suite
//! owns the three constructions that rule does not reach, and reads that
//! suite's `failure` helper rather than carrying a second copy:
//!
//! 1. a document the service could not read is reported as WHAT was invalid with
//!    the diagnostic for it as the QUOTED value
//!    (`Invalid State Machine Definition: '<diagnostic>'`,
//!    `Invalid State Machine Execution Input: '<diagnostic>'`), so an internal
//!    parser string is never answered bare;
//! 2. `ExecutionNotRedrivable` names the EXECUTION the request identified and
//!    then why it is not eligible (`Execution <executionArn> invalid for redrive
//!    as: <reason>`), so a caller redriving several executions can attribute the
//!    refusal to one — while `DescribeExecution`'s `redriveStatusReason`, which
//!    names no execution, carries that reason WITHOUT the wrapper;
//! 3. a value outside a modelled enum is refused with the standard validation
//!    envelope, which names the member, echoes the rejected value and lists
//!    exactly the value set the MODEL declares. The ORDER that set is listed in
//!    is the model's declaration order, which no published rule fixes, so it is
//!    asserted as a set rather than as a sequence.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's
//! crate root, so it is reached through `crate::state_machine_lifecycle`.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    not_redrivable_message, EXECUTION_SETTLE_DELAY, INVALID_STATE_MACHINE_DEFINITION,
    INVALID_STATE_MACHINE_EXECUTION_INPUT, NOT_REDRIVABLE_DELETING, NOT_REDRIVABLE_RUNNING,
    NOT_REDRIVABLE_SUCCEEDED,
};

use crate::error_messages::{failure, MODEL};
use crate::state_machine_lifecycle::{
    create_machine, string, Emulator, HELLO_DEFINITION, ROLE_ARN,
};

/// Long enough that an outcome the clock owed has become due.
///
/// `std::thread::sleep`, for the reason `state_machine_lifecycle` records: the
/// workspace's `tokio` carries no `time` feature, and each test runs on its own
/// single-threaded runtime with no concurrent task.
const PAST_THE_EXECUTION_DELAY: Duration =
    EXECUTION_SETTLE_DELAY.saturating_add(Duration::from_millis(300));

/// A definition that fails immediately, so the execution reaches a terminal
/// status AWS reports as redrivable.
const FAIL_DEFINITION: &str =
    r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail","Error":"Boom","Cause":"on purpose"}}}"#;

/// A definition whose wait is long enough that the execution stays non-terminal
/// for the whole test.
const SLOW_DEFINITION: &str =
    r#"{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":600,"Next":"Done"},"Done":{"Type":"Succeed"}}}"#;

/// The value a message quotes inside `<condition>: '<value>'`, or `None` when
/// the message is not of that form.
fn quoted_value<'a>(message: &'a str, condition: &str) -> Option<&'a str> {
    message
        .strip_prefix(&format!("{condition}: '"))?
        .strip_suffix('\'')
}

// ---------------------------------------------------------------------------
// Rule 1: what was invalid, then the diagnostic in quotes
// ---------------------------------------------------------------------------

/// A document the service could not read is reported as WHAT was invalid, with
/// the diagnostic for it as the quoted value — never as a bare internal parser
/// string.
///
/// The table spans both documents the service parses (an execution `input` and a
/// state machine `definition`), the operations that parse each, and several forms
/// of malformation; the last rows are documents that DO parse, which the service
/// accepts, so the table separates "refused and reported" from "refused at all".
#[tokio::test]
async fn an_unreadable_document_is_reported_as_what_was_invalid() {
    // RECORDED wrappers: `states-creation-input-validation-errors#task#11`
    // records `Invalid State Machine Execution Input: '<detail>'` and the same
    // fixture's definition rows record `Invalid State Machine Definition: '...'`.
    const RECORDED_INPUT: &str = "Invalid State Machine Execution Input";
    const RECORDED_DEFINITION: &str = "Invalid State Machine Definition";
    for (constant, recorded) in [
        (INVALID_STATE_MACHINE_EXECUTION_INPUT, RECORDED_INPUT),
        (INVALID_STATE_MACHINE_DEFINITION, RECORDED_DEFINITION),
    ] {
        assert_eq!(
            constant, recorded,
            "expected the wrapper constant to be {recorded:?}, got {constant:?}"
        );
    }

    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "wrap-input", HELLO_DEFINITION).await;

    // Rows differ along the KIND of malformation — an unquoted key, a bare word,
    // a truncated object, a truncated array, a trailing comma, the empty
    // document — and along which document and operation carries it.
    let malformed = [
        ("an unquoted object key", r#"{oops: 1}"#),
        ("a bare word", "not-json"),
        ("a truncated object", r#"{"a":"#),
        ("a truncated array", "[1, 2"),
        ("a trailing comma", r#"{"a":1,}"#),
        ("the empty document", ""),
    ];

    for (index, (kind, document)) in malformed.iter().enumerate() {
        // The execution `input`: `StartExecution`'s own modelled
        // `InvalidExecutionInput`.
        let case = format!("StartExecution with {kind} as its input");
        let (status, code, message) = failure(
            &emulator,
            &case,
            "StartExecution",
            &json!({
                "stateMachineArn": machine,
                "name": format!("wrap-bad-input-{index}"),
                "input": document,
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExecutionInput"),
            "case {case:?}: expected 400 InvalidExecutionInput, got {status} {code} with message \
             {message:?}"
        );
        let detail = quoted_value(&message, RECORDED_INPUT).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a message of the form \"{RECORDED_INPUT}: '<detail>'\", \
                 got {message:?}"
            )
        });
        assert!(
            !detail.is_empty(),
            "case {case:?}: expected the wrapper to quote a diagnostic, got {message:?}"
        );

        // The state machine `definition`, on BOTH operations that parse one:
        // the same construction with the other wrapper.
        for (operation, body) in [
            (
                "CreateStateMachine",
                json!({
                    "name": format!("wrap-bad-definition-{index}"),
                    "definition": document,
                    "roleArn": ROLE_ARN,
                }),
            ),
            (
                "UpdateStateMachine",
                json!({ "stateMachineArn": machine, "definition": document }),
            ),
        ] {
            // The empty document is outside `definition`'s modelled length
            // (1..1048576), so the boundary refuses it before the parse: that is
            // the validation envelope's row, not this rule's.
            if document.is_empty() {
                continue;
            }
            let case = format!("{operation} with {kind} as its definition");
            let (status, code, message) = failure(&emulator, &case, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidDefinition"),
                "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with message \
                 {message:?}"
            );
            let detail = quoted_value(&message, RECORDED_DEFINITION).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a message of the form \
                     \"{RECORDED_DEFINITION}: '<detail>'\", got {message:?}"
                )
            });
            assert!(
                !detail.is_empty(),
                "case {case:?}: expected the wrapper to quote a diagnostic, got {message:?}"
            );
        }
    }

    // Documents that DO parse are accepted: an object, an array, a string, a
    // number and a null are all JSON input data.
    for (index, document) in [r#"{"a":1}"#, "[1,2]", r#""text""#, "7", "null"]
        .iter()
        .enumerate()
    {
        let case = format!("StartExecution with the valid JSON input {document}");
        let started = emulator
            .ok(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": format!("wrap-good-input-{index}"),
                    "input": document,
                }),
            )
            .await;
        let arn = string(&started, "executionArn");
        assert!(
            arn.contains(":execution:"),
            "case {case:?}: expected an execution ARN, got {arn:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a redrive refusal names the execution, then the reason
// ---------------------------------------------------------------------------

/// `RedriveExecution`'s refusal names the execution the request identified and
/// then the reason, over a table spanning the ineligibility reasons this service
/// reports and more than one execution — while `DescribeExecution`'s
/// `redriveStatusReason` carries the reason alone.
#[tokio::test]
async fn a_redrive_refusal_names_the_execution_and_then_the_reason() {
    let emulator = Emulator::new();

    // Each row: how the execution got where it is, whether its state machine is
    // then deleted, and the reason the refusal must carry.
    let cases = [
        ("a run still in flight", SLOW_DEFINITION, false, NOT_REDRIVABLE_RUNNING),
        ("a succeeded run", HELLO_DEFINITION, false, NOT_REDRIVABLE_SUCCEEDED),
        (
            "a failed run whose state machine is being deleted",
            FAIL_DEFINITION,
            true,
            NOT_REDRIVABLE_DELETING,
        ),
    ];

    let mut started = Vec::new();
    for (index, (_, definition, delete, _)) in cases.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("wrap-redrive-{index}"), definition).await;
        let execution = string(
            &emulator
                .ok(
                    "StartExecution",
                    &json!({
                        "stateMachineArn": machine,
                        "name": format!("wrap-redrive-run-{index}"),
                        "input": "{}",
                    }),
                )
                .await,
            "executionArn",
        );
        started.push((machine, execution, *delete));
    }
    // The outcomes the clock owes are due once, for the whole table; the
    // deletions are started afterwards so each row's execution is terminal
    // before its state machine goes.
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    for (machine, _, delete) in &started {
        if *delete {
            emulator
                .ok("DeleteStateMachine", &json!({ "stateMachineArn": machine }))
                .await;
        }
    }

    for ((case, _, _, reason), (_, execution, _)) in cases.iter().zip(&started) {
        let (status, code, message) = failure(
            &emulator,
            case,
            "RedriveExecution",
            &json!({ "executionArn": execution }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ExecutionNotRedrivable"),
            "case {case:?}: expected 400 ExecutionNotRedrivable, got {status} {code} with message \
             {message:?}"
        );
        // Spelled literally, so a reworded constructor fails here: the recorded
        // service answers `Execution <executionArn> invalid for redrive as:
        // <reason>` (`states-execution-failure-and-redrive#task#15`).
        let expected = format!("Execution {execution} invalid for redrive as: {reason}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert_eq!(
            message,
            not_redrivable_message(execution, reason),
            "case {case:?}: expected the refusal to read the service's one constructor, got \
             {message:?}"
        );

        // The projection the caller reads to decide whether to redrive carries
        // the same reason, and names no execution: the wrapper belongs to the
        // refusal, not to `redriveStatusReason`.
        let described = emulator
            .ok("DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        let projected = described
            .get("redriveStatusReason")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a redriveStatusReason in {described:?}"));
        assert_eq!(
            projected, *reason,
            "case {case:?}: expected redriveStatusReason {reason:?}, got {projected:?}"
        );
        assert!(
            !projected.contains("invalid for redrive as"),
            "case {case:?}: expected redriveStatusReason to carry the reason alone, got \
             {projected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: an out-of-set enum value lists the set the MODEL declares
// ---------------------------------------------------------------------------

/// The values an enum shape of this model declares, read off the model itself
/// (`error_messages::MODEL`, the one copy this test binary embeds).
///
/// A Smithy 2.0 `enum` shape carries one member per value, each with an
/// `smithy.api#enumValue` trait holding the wire value; a member without one
/// takes its own name. Returned as a SET, because the ORDER a refusal lists them
/// in is not fixed by anything the service publishes.
fn declared_enum_values(shape_id: &str) -> BTreeSet<String> {
    let model: Value = serde_json::from_str(MODEL).expect("the Smithy model parses as JSON");
    let shape = model
        .get("shapes")
        .and_then(|shapes| shapes.get(shape_id))
        .unwrap_or_else(|| panic!("the model declares {shape_id}"));
    assert_eq!(
        shape.get("type").and_then(Value::as_str),
        Some("enum"),
        "expected {shape_id} to be an enum shape, got {shape:?}"
    );
    let members = shape
        .get("members")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("expected {shape_id} to declare members, got {shape:?}"));
    members
        .iter()
        .map(|(name, member)| {
            member
                .get("traits")
                .and_then(|traits| traits.get("smithy.api#enumValue"))
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string()
        })
        .collect()
}

/// A value outside a modelled enum is refused with the validation envelope,
/// which names the member, echoes the value and lists exactly the value set the
/// model declares.
///
/// Rows span two enum members of two operations and several rejected values. The
/// listed set is compared as a SET: the sequence a service lists it in is not
/// derivable from the model (which declares one order) and AWS publishes no rule
/// for it, so only membership is asserted.
#[tokio::test]
async fn an_out_of_set_enum_value_lists_the_set_the_model_declares() {
    const CLAUSE: &str = "Member must satisfy enum value set: [";

    let emulator = Emulator::new();

    // (operation, body, member path, rejected value, the enum shape it targets)
    // `TestState.inspectionLevel -> InspectionLevel` and
    // `CreateStateMachine.type -> StateMachineType`
    // (smithy_query(query="operation", operation="TestState,CreateStateMachine")).
    let cases: Vec<(&str, Value, &str, &str, &str)> = vec![
        (
            "TestState",
            json!({ "definition": HELLO_DEFINITION, "inspectionLevel": "NOT_A_REAL_LEVEL" }),
            "inspectionLevel",
            "NOT_A_REAL_LEVEL",
            "com.amazonaws.sfn#InspectionLevel",
        ),
        (
            "TestState",
            json!({ "definition": HELLO_DEFINITION, "inspectionLevel": "info" }),
            "inspectionLevel",
            "info",
            "com.amazonaws.sfn#InspectionLevel",
        ),
        (
            "CreateStateMachine",
            json!({
                "name": "wrap-enum",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "SUPER_EXPRESS",
            }),
            "type",
            "SUPER_EXPRESS",
            "com.amazonaws.sfn#StateMachineType",
        ),
    ];

    for (operation, body, member, rejected, shape_id) in cases {
        let case = format!("{operation} with {member} = {rejected:?}");
        let (status, code, message) = failure(&emulator, &case, operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with message \
             {message:?}"
        );
        let opening = format!(
            "1 validation error detected: Value '{rejected}' at '{member}' failed to satisfy \
             constraint: {CLAUSE}"
        );
        let listed = message
            .strip_prefix(&opening)
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a message opening {opening:?}, got {message:?}")
            });
        let listed: BTreeSet<String> = listed.split(", ").map(str::to_string).collect();
        let declared = declared_enum_values(shape_id);
        assert_eq!(
            listed, declared,
            "case {case:?}: expected the clause to list the model's value set {declared:?}, got \
             {listed:?} (message {message:?})"
        );
    }
}

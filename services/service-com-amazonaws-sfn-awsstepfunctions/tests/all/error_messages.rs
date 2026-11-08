// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEXT of this service's error messages, driven over
//! its own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes in `$.message`.
//!
//! Three rules are under test, each asserted over a table whose rows differ
//! along what the rule generalises over rather than on the one request that
//! exposed it:
//!
//! 1. a client error that reports one value of the request states the
//!    condition its error shape names, then `: ` and that value in single
//!    quotes — so the same shape reads the same sentence whichever operation
//!    raised it and whichever value it is about;
//! 2. `StateMachineTypeNotSupported` states a FIXED service string that carries
//!    no value of the request, so its message does not vary with the operation,
//!    the workflow type or the ARN;
//! 3. a refused input value is echoed in the standard AWS validation envelope as
//!    the member's MODELLED TYPE renders it: a `float`/`double` member with a
//!    fractional part, an `integer`/`long` member without one. Values inside
//!    every modelled bound are accepted by the boundary (and so reach the
//!    operation), which is what distinguishes "rejected and reported" from
//!    "rejected at all".
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's
//! crate root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    FRACTIONAL_MEMBERS, STATE_MACHINE_TYPE_NOT_SUPPORTED,
};

use crate::state_machine_lifecycle::{
    create_machine, error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION,
    PAST_THE_DELETE_DELAY, REGION, ROLE_ARN,
};

/// This service's Smithy model, read at compile time so the fractional-member
/// list below can be checked against the model it was derived from.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a
/// sibling suite reading the model reads THIS copy rather than embedding a
/// second one.
pub(crate) const MODEL: &str =
    include_str!("../../smithy-model/com-amazonaws-sfn-awsstepfunctions.json");

/// A second definition, different from [`HELLO_DEFINITION`], so a repeat create
/// under the same name is the documented conflict rather than the documented
/// idempotent hit.
const OTHER_DEFINITION: &str =
    r#"{"StartAt":"Bye","States":{"Bye":{"Type":"Pass","Result":{"msg":"bye"},"End":true}}}"#;

/// A `mapRunArn` inside the modelled `LongArn` length (1..2000), so a row that
/// probes another member of `UpdateMapRun` is refused for that member alone.
const MAP_RUN_ARN: &str =
    "arn:aws:states:us-west-2:000000000000:mapRun:probe-sm/01234567-89ab-cdef-0123-456789abcdef:0";

/// `(status, errorCode, message)` of a call that must fail.
///
/// The `__type` split is the harness's own `error_code`, not a copy of it.
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a
/// sibling suite asserting another message rule reads the same helper rather
/// than carrying a second copy of it.
pub(crate) async fn failure(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, parsed) = emulator.call(operation, body).await;
    assert_ne!(
        status, 200,
        "case {case:?}: expected {operation} to fail, got 200 with body {parsed:?}"
    );
    let code = error_code(&parsed);
    let message = parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {parsed:?}"))
        .to_string();
    (status, code, message)
}

/// The ARN of a state machine in the region every request here signs for.
fn state_machine_arn(name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:{name}")
}

/// The ARN of one execution of a state machine, in that same region.
fn execution_arn(state_machine: &str, execution: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:{state_machine}:{execution}")
}

// ---------------------------------------------------------------------------
// Rule 1: `<Condition>: '<value>'`
// ---------------------------------------------------------------------------

/// Every client error that reports one value of the request states its own
/// condition followed by that value, over a table spanning the error shapes in
/// that family, the operations that raise them, both resource kinds and both
/// kinds of reported value (an ARN and a name).
#[tokio::test]
async fn a_client_error_names_its_condition_and_the_request_value() {
    let emulator = Emulator::new();

    // A state machine with one running execution, and a state machine that has been
    // deleted: between them these reach every existence error this service raises.
    let standard = create_machine(&emulator, "msg-standard", HELLO_DEFINITION).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": standard, "name": "msg-exec", "input": "{\"a\":1}" }),
        )
        .await;
    let execution = string(&started, "executionArn");

    let deleted = create_machine(&emulator, "msg-deleted", HELLO_DEFINITION).await;
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": deleted }))
        .await;
    std::thread::sleep(PAST_THE_DELETE_DELAY);

    let absent_machine = state_machine_arn("msg-never-created");
    let absent_execution = execution_arn("msg-never-created", "msg-never-started");

    // Each row differs along the error shape, the operation raising it, the
    // resource kind and which resource the condition is about.
    let cases: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "CreateStateMachine on a name in use with a different definition",
            "CreateStateMachine",
            json!({ "name": "msg-standard", "definition": OTHER_DEFINITION, "roleArn": ROLE_ARN }),
            "StateMachineAlreadyExists",
            format!("State Machine Already Exists: '{standard}'"),
        ),
        (
            "StartExecution reusing a running execution's name with a different input",
            "StartExecution",
            json!({ "stateMachineArn": standard, "name": "msg-exec", "input": "{\"a\":2}" }),
            "ExecutionAlreadyExists",
            format!("Execution Already Exists: '{execution}'"),
        ),
        (
            "DescribeStateMachine on a deleted state machine",
            "DescribeStateMachine",
            json!({ "stateMachineArn": deleted }),
            "StateMachineDoesNotExist",
            format!("State Machine Does Not Exist: '{deleted}'"),
        ),
        (
            "DescribeStateMachine on a state machine that was never created",
            "DescribeStateMachine",
            json!({ "stateMachineArn": absent_machine }),
            "StateMachineDoesNotExist",
            format!("State Machine Does Not Exist: '{absent_machine}'"),
        ),
        (
            "StartExecution on a state machine that was never created",
            "StartExecution",
            json!({ "stateMachineArn": absent_machine, "name": "msg-orphan" }),
            "StateMachineDoesNotExist",
            format!("State Machine Does Not Exist: '{absent_machine}'"),
        ),
        (
            "ListExecutions on a state machine that was never created",
            "ListExecutions",
            json!({ "stateMachineArn": absent_machine }),
            "StateMachineDoesNotExist",
            format!("State Machine Does Not Exist: '{absent_machine}'"),
        ),
        (
            "DescribeExecution on an execution that was never started",
            "DescribeExecution",
            json!({ "executionArn": absent_execution }),
            "ExecutionDoesNotExist",
            format!("Execution Does Not Exist: '{absent_execution}'"),
        ),
        (
            "StopExecution on an execution that was never started",
            "StopExecution",
            json!({ "executionArn": absent_execution }),
            "ExecutionDoesNotExist",
            format!("Execution Does Not Exist: '{absent_execution}'"),
        ),
        // The reported value is not always an ARN: `InvalidName` reports the
        // name the caller sent, through the same template. (`InvalidArn`'s own
        // quoted value is a DIAGNOSIS of the ARN rather than a value echoed
        // from the request, which `tests/all/arn_validation.rs` owns.)
        (
            "CreateStateMachine with a wildcard in the name",
            "CreateStateMachine",
            json!({ "name": "bad*name?", "definition": HELLO_DEFINITION, "roleArn": ROLE_ARN }),
            "InvalidName",
            "Invalid Name: 'bad*name?'".to_string(),
        ),
        (
            "StartExecution with white space in the execution name",
            "StartExecution",
            json!({ "stateMachineArn": standard, "name": "bad name" }),
            "InvalidName",
            "Invalid Name: 'bad name'".to_string(),
        ),
        // `UpdateStateMachine` states the same template, whichever member of the
        // update the request carried.
        (
            "UpdateStateMachine's definition on a state machine that was never created",
            "UpdateStateMachine",
            json!({ "stateMachineArn": absent_machine, "definition": HELLO_DEFINITION }),
            "StateMachineDoesNotExist",
            format!("State Machine Does Not Exist: '{absent_machine}'"),
        ),
        (
            "UpdateStateMachine's roleArn on a deleted state machine",
            "UpdateStateMachine",
            json!({ "stateMachineArn": deleted, "roleArn": ROLE_ARN }),
            "StateMachineDoesNotExist",
            format!("State Machine Does Not Exist: '{deleted}'"),
        ),
    ];

    for (case, operation, body, expected_code, expected_message) in cases {
        let (status, code, message) = failure(&emulator, case, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with message {message:?}"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected message {expected_message:?}, got {message:?}"
        );
    }
}

/// A condition the service words as a SENTENCE rather than as the words of its
/// shape name still states that sentence and then the ARN: the template is the
/// same, only the condition phrase differs. Asserted on both operations that
/// refuse a state machine the clock has not finished deleting.
#[tokio::test]
async fn the_deleting_condition_is_stated_with_the_arn_too() {
    let emulator = Emulator::new();

    let arn = create_machine(&emulator, "msg-deleting", HELLO_DEFINITION).await;
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": arn }))
        .await;
    // No sleep: the deletion is owed to the clock, so the record is normally
    // still DELETING for the calls below — which is the state these errors
    // report. A loaded machine could let the deletion settle first, and then the
    // record is gone; BOTH outcomes are the template, so the assertion accepts
    // the condition for the state the caller actually found rather than racing
    // the clock.
    let owed: [(&str, String); 2] = [
        (
            "StateMachineDeleting",
            format!("State Machine is being deleted: '{arn}'"),
        ),
        (
            "StateMachineDoesNotExist",
            format!("State Machine Does Not Exist: '{arn}'"),
        ),
    ];

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "CreateStateMachine on a name whose state machine is being deleted",
            "CreateStateMachine",
            json!({ "name": "msg-deleting", "definition": OTHER_DEFINITION, "roleArn": ROLE_ARN }),
        ),
        (
            "StartExecution on a state machine being deleted",
            "StartExecution",
            json!({ "stateMachineArn": arn, "name": "msg-deleting-exec" }),
        ),
        // Both members of an update reach the same precondition, and each reads
        // the same sentence.
        (
            "UpdateStateMachine's definition on a state machine being deleted",
            "UpdateStateMachine",
            json!({ "stateMachineArn": arn, "definition": OTHER_DEFINITION }),
        ),
        (
            "UpdateStateMachine's roleArn on a state machine being deleted",
            "UpdateStateMachine",
            json!({ "stateMachineArn": arn, "roleArn": ROLE_ARN }),
        ),
    ];

    for (case, operation, body) in cases {
        let (status, code, message) = failure(&emulator, case, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with message {message:?}"
        );
        let matched = owed
            .iter()
            .find(|(expected_code, _)| *expected_code == code);
        let Some((_, expected_message)) = matched else {
            panic!(
                "case {case:?}: expected one of {:?}, got error code {code:?} with message \
                 {message:?}",
                owed.iter().map(|(code, _)| *code).collect::<Vec<_>>()
            );
        };
        assert_eq!(
            &message, expected_message,
            "case {case:?}: expected message {expected_message:?} for error code {code:?}, got \
             {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a fixed service string
// ---------------------------------------------------------------------------
/// `StateMachineTypeNotSupported` reads the same constant whichever state
/// machine the request named, so the message carries no value of the request.
///
/// The sentence is spelled LITERALLY here, not read from
/// `state::STATE_MACHINE_TYPE_NOT_SUPPORTED`, so a wrong constant fails as well
/// as a varying one; the second assertion then pins the constant itself to this
/// recorded text.
#[tokio::test]
async fn the_unsupported_type_message_is_a_constant() {
    // RECORDED text (`states-express-state-machine-type-restrictions#task#7`).
    const RECORDED: &str = "This operation is not supported by this type of state machine";
    assert_eq!(
        STATE_MACHINE_TYPE_NOT_SUPPORTED, RECORDED,
        "expected state::STATE_MACHINE_TYPE_NOT_SUPPORTED to be {RECORDED:?}, got {:?}",
        STATE_MACHINE_TYPE_NOT_SUPPORTED
    );

    let emulator = Emulator::new();

    // Two EXPRESS state machines differing in name and definition: the rule is
    // that neither difference reaches the message.
    let first = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "msg-express-one",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        )
        .await;
    let first = string(&first, "stateMachineArn");
    let second = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "msg-express-two-with-a-much-longer-name",
                "definition": OTHER_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        )
        .await;
    let second = string(&second, "stateMachineArn");

    for arn in [&first, &second] {
        let case = format!("ListExecutions on the EXPRESS state machine {arn}");
        let (status, code, message) = failure(
            &emulator,
            &case,
            "ListExecutions",
            &json!({ "stateMachineArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with message {message:?}"
        );
        assert_eq!(
            code, "StateMachineTypeNotSupported",
            "case {case:?}: expected error code StateMachineTypeNotSupported, got {code:?}"
        );
        assert_eq!(
            message, RECORDED,
            "case {case:?}: expected message {RECORDED:?}, got {message:?}"
        );
        assert!(
            !message.contains(arn.as_str()) && !message.contains("ListExecutions"),
            "case {case:?}: expected no request value in the message, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the rejected value in the member's modelled type
// ---------------------------------------------------------------------------

/// A refused value is reported in the standard validation envelope, rendered as
/// the member's modelled type renders it, over a table spanning the numeric
/// types the model declares, both ends of a declared range, both published
/// bounds (which are accepted), a `@length` bound and an absent required
/// member — and over more than one operation, because the rule is the
/// service's rather than any one operation's.
#[tokio::test]
async fn a_refused_value_renders_in_the_members_modelled_type() {
    let emulator = Emulator::new();

    // `None` means the boundary ACCEPTS the row, so it reaches the operation — and
    // the Map Run ARN below names no record, which `UpdateMapRun` refuses with its
    // own modelled `ResourceNotFound`.
    let cases: Vec<(&str, &str, Value, Option<String>)> = vec![
        // `toleratedFailurePercentage` targets `ToleratedFailurePercentage`,
        // modelled `float` with range 0..100
        // (smithy_query(query="operation", operation="UpdateMapRun")), so a
        // rejected value renders with a fractional part even when it has none.
        (
            "a float member past its maximum, written without a fraction",
            "UpdateMapRun",
            json!({ "mapRunArn": MAP_RUN_ARN, "toleratedFailurePercentage": 150 }),
            Some(
                "1 validation error detected: Value '150.0' at 'toleratedFailurePercentage' \
                 failed to satisfy constraint: Member must have value less than or equal to 100"
                    .to_string(),
            ),
        ),
        (
            "a float member past its maximum, written with a fraction",
            "UpdateMapRun",
            json!({ "mapRunArn": MAP_RUN_ARN, "toleratedFailurePercentage": 100.5 }),
            Some(
                "1 validation error detected: Value '100.5' at 'toleratedFailurePercentage' \
                 failed to satisfy constraint: Member must have value less than or equal to 100"
                    .to_string(),
            ),
        ),
        (
            "a float member below its minimum, written without a fraction",
            "UpdateMapRun",
            json!({ "mapRunArn": MAP_RUN_ARN, "toleratedFailurePercentage": -1 }),
            Some(
                "1 validation error detected: Value '-1.0' at 'toleratedFailurePercentage' \
                 failed to satisfy constraint: Member must have value greater than or equal to 0"
                    .to_string(),
            ),
        ),
        (
            "a float member below its minimum, written with a fraction",
            "UpdateMapRun",
            json!({ "mapRunArn": MAP_RUN_ARN, "toleratedFailurePercentage": -0.5 }),
            Some(
                "1 validation error detected: Value '-0.5' at 'toleratedFailurePercentage' \
                 failed to satisfy constraint: Member must have value greater than or equal to 0"
                    .to_string(),
            ),
        ),
        // Each published bound of that range is INSIDE it.
        (
            "a float member at its published minimum",
            "UpdateMapRun",
            json!({ "mapRunArn": MAP_RUN_ARN, "toleratedFailurePercentage": 0 }),
            None,
        ),
        (
            "a float member at its published maximum",
            "UpdateMapRun",
            json!({ "mapRunArn": MAP_RUN_ARN, "toleratedFailurePercentage": 100 }),
            None,
        ),
        // `maxConcurrency` is `integer` and `toleratedFailureCount` is `long`,
        // both range 0..: an integral member renders with no fractional part.
        (
            "an integer member below its minimum",
            "UpdateMapRun",
            json!({ "mapRunArn": MAP_RUN_ARN, "maxConcurrency": -1 }),
            Some(
                "1 validation error detected: Value '-1' at 'maxConcurrency' failed to satisfy \
                 constraint: Member must have value greater than or equal to 0"
                    .to_string(),
            ),
        ),
        (
            "a long member below its minimum",
            "UpdateMapRun",
            json!({ "mapRunArn": MAP_RUN_ARN, "toleratedFailureCount": -5 }),
            Some(
                "1 validation error detected: Value '-5' at 'toleratedFailureCount' failed to \
                 satisfy constraint: Member must have value greater than or equal to 0"
                    .to_string(),
            ),
        ),
        // A `@length` bound and an absent required member: the rest of the
        // envelope is unchanged by the value rendering above.
        (
            "a string member under its minimum length",
            "UpdateMapRun",
            json!({ "mapRunArn": "" }),
            Some(
                "1 validation error detected: Value '' at 'mapRunArn' failed to satisfy \
                 constraint: Member must have length greater than or equal to 1"
                    .to_string(),
            ),
        ),
        (
            "an absent required member",
            "UpdateMapRun",
            json!({}),
            Some(
                "1 validation error detected: Value null at 'mapRunArn' failed to satisfy \
                 constraint: Member must not be null"
                    .to_string(),
            ),
        ),
        // Another operation, to show the envelope is the service's rather than
        // `UpdateMapRun`'s: `maxResults` is `integer` with range 0..1000 and
        // `name` is a string of length 1..80.
        (
            "an integer member of another operation past its maximum",
            "ListExecutions",
            json!({ "stateMachineArn": state_machine_arn("msg-any"), "maxResults": 1001 }),
            Some(
                "1 validation error detected: Value '1001' at 'maxResults' failed to satisfy \
                 constraint: Member must have value less than or equal to 1000"
                    .to_string(),
            ),
        ),
        (
            "a string member of another operation under its minimum length",
            "CreateStateMachine",
            json!({ "name": "", "definition": HELLO_DEFINITION, "roleArn": ROLE_ARN }),
            Some(
                "1 validation error detected: Value '' at 'name' failed to satisfy constraint: \
                 Member must have length greater than or equal to 1"
                    .to_string(),
            ),
        ),
    ];

    for (case, operation, body, expected) in cases {
        match expected {
            Some(expected_message) => {
                let (status, code, message) = failure(&emulator, case, operation, &body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} with message {message:?}"
                );
                assert_eq!(
                    code, "ValidationException",
                    "case {case:?}: expected error code ValidationException, got {code:?}"
                );
                assert_eq!(
                    message, expected_message,
                    "case {case:?}: expected message {expected_message:?}, got {message:?}"
                );
            }
            None => {
                let (status, code, message) = failure(&emulator, case, operation, &body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the boundary to accept the value and the operation \
                     to refuse the absent Map Run, got {status} with message {message:?}"
                );
                assert_eq!(
                    code, "ResourceNotFound",
                    "case {case:?}: expected error code ResourceNotFound, got {code:?}"
                );
            }
        }
    }
}

/// Every FRACTIONAL member the model declares is listed in
/// `state::FRACTIONAL_MEMBERS`, read off the model itself rather than trusted.
///
/// The list stands in for the modelled numeric type, because the framework
/// hands a constraint violation no type; this test is what ties it to the model,
/// so a model that later grows a `float`/`double` member fails here instead of
/// silently reporting that member's rejected value in an integer's spelling.
#[test]
fn every_fractional_member_of_the_model_is_listed() {
    let model: Value = serde_json::from_str(MODEL).expect("the Smithy model parses as JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the model carries a 'shapes' object");

    // A member's modelled type is the `type` of the shape it targets.
    let type_of = |target: &str| -> Option<String> {
        shapes
            .get(target)?
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    let mut fractional: Vec<String> = Vec::new();
    for shape in shapes.values() {
        let Some(members) = shape.get("members").and_then(Value::as_object) else {
            continue;
        };
        for (member, definition) in members {
            let Some(target) = definition.get("target").and_then(Value::as_str) else {
                continue;
            };
            // The wire name is the member name in lowerCamel, which is how the
            // framework spells a violation's path.
            if type_of(target).is_some_and(|kind| kind == "float" || kind == "double") {
                let mut wire = member.clone();
                if let Some(first) = wire.get_mut(0..1) {
                    first.make_ascii_lowercase();
                }
                if !fractional.contains(&wire) {
                    fractional.push(wire);
                }
            }
        }
    }

    assert!(
        !fractional.is_empty(),
        "expected the model to declare at least one fractional member, found none — the walk \
         above no longer reads the model"
    );
    for member in &fractional {
        assert!(
            FRACTIONAL_MEMBERS.contains(&member.as_str()),
            "case {member:?}: the model declares it float/double, but state::FRACTIONAL_MEMBERS \
             is {FRACTIONAL_MEMBERS:?}"
        );
    }
    for listed in FRACTIONAL_MEMBERS {
        assert!(
            fractional.iter().any(|member| member == listed),
            "case {listed:?}: listed in state::FRACTIONAL_MEMBERS, but the model declares no \
             float/double member of that name (found {fractional:?})"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SUBJECTS an `InvalidDefinition` message states, driven
//! over this service's own wire protocol through the production `MultiServiceRouter`.
//!
//! The rule under test: a definition is refused with EVERY problem it carries, one
//! subject per problem, and each subject states where the problem was found — the
//! `/States/<name>/<field>` position for a problem a state or a field owns, and the
//! bare word `null` for a problem about the definition AS A WHOLE, which sits at no
//! position in the document. A caller fixing the reported problems is therefore told
//! all of them at once rather than one per attempt.
//!
//! The rows differ along what the rule generalises over: how many problems the
//! definition carries (one, two, three), which KINDS they are (a transition target no
//! state declares, a state nothing reaches, a state that says nowhere to go, and a
//! workflow with no terminal state at all) and whether the problem is owned by a field,
//! by a state or by the whole definition — which is what decides the place its subject
//! states.
//!
//! The harness is `state_machine_lifecycle`'s; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{Emulator, ROLE_ARN};

/// The envelope every definition refusal of this service carries.
const ENVELOPE_OPEN: &str = "Invalid State Machine Definition: '";

/// The place a subject states for a problem about the definition as a whole.
const NO_LOCATION: &str = "null";

/// The subjects the `InvalidDefinition` for `definition` quotes, in order.
async fn subjects(emulator: &Emulator, case: &str, index: usize, definition: &Value) -> String {
    let (status, body) = emulator
        .call(
            "CreateStateMachine",
            &json!({
                "name": format!("findings-{index}"),
                "definition": definition.to_string(),
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let code = crate::state_machine_lifecycle::error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidDefinition"),
        "case {case:?}: expected 400 InvalidDefinition, got {status} with {body:?}"
    );
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    message
        .strip_prefix(ENVELOPE_OPEN)
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the definition envelope, got {message:?}")
        })
        .to_string()
}

/// A refusal states every problem the definition carries, and a problem about the
/// whole definition states its place as `null`.
#[tokio::test]
async fn a_definition_refusal_states_every_problem_and_where_it_is() {
    let emulator = Emulator::new();

    // (case, the definition, the subjects the refusal must quote, in order)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            // The recorded row: a state transitioning to a state the definition does
            // not declare, in a workflow that can therefore never stop.
            "a dangling transition target in a workflow with no terminal state",
            json!({
                "StartAt": "InvokeProcessingLambda",
                "States": {
                    "InvokeProcessingLambda": {
                        "Type": "Pass",
                        "Next": "AStateThatIsNotDefined",
                    }
                },
            }),
            vec![
                "MISSING_TRANSITION_TARGET: Missing 'Next' target: AStateThatIsNotDefined at \
                 /States/InvokeProcessingLambda/Next"
                    .to_string(),
                format!("MISSING_END_STATE: Workflow has no terminal state at {NO_LOCATION}"),
            ],
        ),
        (
            // The same dangling target, in a workflow that CAN stop: one problem, and
            // the machine-level subject is not stated at all.
            "a dangling transition target beside a terminal state",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Choice", "Choices": [
                        { "Variable": "$.n", "NumericEquals": 1, "Next": "Nowhere" }
                    ], "Default": "Done" },
                    "Done": { "Type": "Succeed" },
                },
            }),
            vec![
                "MISSING_TRANSITION_TARGET: Missing 'Next' target: Nowhere at \
                 /States/A/Choices[0]/Next"
                    .to_string(),
            ],
        ),
        (
            // A workflow that cannot stop and nothing else: the one subject states its
            // place as `null`.
            "a workflow with no terminal state and nothing else wrong",
            json!({
                "StartAt": "Loop",
                "States": { "Loop": { "Type": "Pass", "Next": "Loop" } },
            }),
            vec![format!(
                "MISSING_END_STATE: Workflow has no terminal state at {NO_LOCATION}"
            )],
        ),
        (
            // Three problems at once: a state that says nowhere to go, a state nothing
            // reaches, and the workflow that can therefore never stop.
            "a state with no transition, an unreachable state and no terminal state",
            json!({
                "StartAt": "Start",
                "States": {
                    "Start": { "Type": "Pass" },
                    "Orphan": { "Type": "Pass", "Next": "Start" },
                },
            }),
            vec![
                "SCHEMA_VALIDATION_FAILED: state 'Start' is a 'Pass' state, so it must carry \
                 either 'Next' or \"End\": true at /States/Start"
                    .to_string(),
                "MISSING_TRANSITION_TARGET: State \"Orphan\" is not reachable. at /States/Orphan"
                    .to_string(),
                format!("MISSING_END_STATE: Workflow has no terminal state at {NO_LOCATION}"),
            ],
        ),
    ];

    for (index, (case, definition, expected)) in rows.into_iter().enumerate() {
        let stated = subjects(&emulator, case, index, &definition).await;
        let expected = expected.join(", ");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the subjects {expected:?}, got {stated:?}"
        );
    }
}

/// The same definition `ValidateStateMachineDefinition` reports a diagnostic list for
/// is the one a create refuses, problem for problem: the create's subjects are the
/// validation's ERROR diagnostics, so the two cannot disagree about what is wrong.
#[tokio::test]
async fn a_create_refuses_what_the_validation_reports() {
    let emulator = Emulator::new();
    let definition = json!({
        "StartAt": "InvokeProcessingLambda",
        "States": {
            "InvokeProcessingLambda": { "Type": "Pass", "Next": "AStateThatIsNotDefined" }
        },
    });

    let validated = emulator
        .ok(
            "ValidateStateMachineDefinition",
            &json!({ "definition": definition.to_string() }),
        )
        .await;
    let diagnostics = validated
        .get("diagnostics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let codes: Vec<&str> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.get("severity").and_then(Value::as_str) == Some("ERROR"))
        .filter_map(|diagnostic| diagnostic.get("code").and_then(Value::as_str))
        .collect();
    assert_eq!(
        codes,
        vec!["MISSING_TRANSITION_TARGET", "MISSING_END_STATE"],
        "expected both problems to be reported, got {validated:?}"
    );

    let stated = subjects(&emulator, "the same definition on a create", 99, &definition).await;
    for code in codes {
        assert!(
            stated.contains(code),
            "expected the create's subjects to carry {code:?}, got {stated:?}"
        );
    }
}

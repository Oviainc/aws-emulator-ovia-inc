// SPDX-License-Identifier: CC0-1.0

//! What `ValidateStateMachineDefinition` reports about a definition's GRAPH: the
//! transitions it names, whether it can stop, and the variables its fields read.
//!
//! Driven end to end over this service's own wire protocol through the production
//! router (`crate::state_machine_lifecycle::Emulator`), so what is asserted is the
//! `diagnostics[]` a signed SDK request observes — and, where the diagnostic is an
//! ERROR, what `CreateStateMachine` does with the same definition.
//!
//! The rules under test, each over a table whose rows differ along what the rule
//! generalises over rather than along the one definition that exposed it:
//!
//! 1. a transition target the enclosing `States` map does not declare is one problem
//!    whichever member names it — a state's own `Next`, a `Choice` rule's `Next`, the
//!    `Default` beside them, a `Catch` catcher's `Next`, the machine's own `StartAt` —
//!    reported as `MISSING_TRANSITION_TARGET` at that member, naming the target it
//!    named as the value of the transition field the language calls `Next`; and the
//!    same code reports the other half of that rule, a state the transitions reach
//!    nowhere, at the state itself;
//! 2. a state machine must declare at least one state that ENDS an execution — a
//!    `Succeed`, a `Fail`, or any state declaring `"End": true` — and one that
//!    declares none earns `MISSING_END_STATE`, located at the machine rather than at a
//!    field the document has not got;
//! 3. a field that reads a variable no run reaching that field has certainly assigned
//!    earns one `POSSIBLY_UNDEFINED_VARIABLE` WARNING per field, located at the field,
//!    in either query language and through either form of read; a variable every path
//!    to the read assigns earns none;
//! 4. `maxResults` caps that reported list and `truncated` says whether it was cut
//!    short, warnings included.
//!
//! The harness (`Emulator`, `error_code`, `ROLE_ARN`) is the one
//! `state_machine_lifecycle` builds and the validation readers are
//! `definition_validation`'s; `tests/all.rs` is this binary's crate root, so each is
//! reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    MISSING_END_STATE, MISSING_TRANSITION_TARGET, POSSIBLY_UNDEFINED_VARIABLE,
};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    SEVERITY_ERROR, SEVERITY_WARNING, VALIDATION_RESULT_FAIL, VALIDATION_RESULT_OK,
};

use crate::definition_validation::{diagnostics, result_of, validate};
use crate::state_machine_lifecycle::{error_code, Emulator, ROLE_ARN};

/// The name every row below points a transition at and declares nowhere, chosen so no
/// sentence could carry it by accident.
const UNDECLARED: &str = "NoSuchState";

/// One ERROR diagnostic as the wire reports it, with a `location`.
fn error_at(code: &str, location: &str, message: &str) -> Value {
    json!({
        "severity": SEVERITY_ERROR,
        "code": code,
        "location": location,
        "message": message,
    })
}

/// One WARNING diagnostic as the wire reports it.
fn warning_at(code: &str, location: &str, message: &str) -> Value {
    json!({
        "severity": SEVERITY_WARNING,
        "code": code,
        "location": location,
        "message": message,
    })
}

/// One definition, and the `(location, variable)` pairs its reads earn a warning for.
///
/// A row naming no pair is a definition whose every read resolves.
type VariableCase = (&'static str, Value, Vec<(&'static str, &'static str)>);

/// The reported diagnostics carrying `code`, in the order they were reported.
fn of_code(body: &Value, code: &str) -> Vec<Value> {
    diagnostics(body)
        .into_iter()
        .filter(|diagnostic| diagnostic.get("code").and_then(Value::as_str) == Some(code))
        .collect()
}

/// What `CreateStateMachine` answers for `definition`, as `(status, error code)`.
async fn created(emulator: &Emulator, name: &str, definition: &str) -> (u16, String) {
    let (status, body) = emulator
        .call(
            "CreateStateMachine",
            &json!({ "name": name, "definition": definition, "roleArn": ROLE_ARN }),
        )
        .await;
    (status, error_code(&body))
}

// ---------------------------------------------------------------------------
// Rule 1: a transition target no state declares, through every member that names one
// ---------------------------------------------------------------------------

/// One definition naming exactly ONE target no state declares, and the diagnostics it
/// earns.
struct TargetCase {
    case: &'static str,
    definition: String,
    location: String,
    /// The states the unresolved transition leaves with nothing reaching them, each at
    /// its own location: a dangling transition is also a state the graph does not
    /// reach, and both halves are reported.
    unreachable: Vec<&'static str>,
}

/// The rows rule 1 spans: WHICH member names the unresolved target, where in the
/// document that member sits (a state, a `Choice` rule, a catcher, the machine
/// itself), how deeply the machine is nested, and what the target is called.
fn target_cases() -> Vec<TargetCase> {
    let rows: Vec<(&'static str, Value, &'static str, Vec<&'static str>)> = vec![
        (
            "a state's own Next",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Next": UNDECLARED },
                    "Done": { "Type": "Succeed" },
                },
            }),
            "/States/A/Next",
            vec!["/States/Done"],
        ),
        (
            "the machine's own StartAt",
            json!({ "StartAt": UNDECLARED, "States": { "A": { "Type": "Succeed" } } }),
            "/StartAt",
            vec!["/States/A"],
        ),
        (
            "a Choice rule's own Next",
            json!({
                "StartAt": "C",
                "States": {
                    "C": {
                        "Type": "Choice",
                        "Choices": [
                            { "Variable": "$.x", "NumericEquals": 1, "Next": "Done" },
                            { "Variable": "$.x", "NumericEquals": 2, "Next": UNDECLARED },
                        ],
                        "Default": "Done",
                    },
                    "Done": { "Type": "Succeed" },
                },
            }),
            "/States/C/Choices[1]/Next",
            vec![],
        ),
        (
            "the Default beside a Choice's rules",
            json!({
                "StartAt": "C",
                "States": {
                    "C": {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.x", "NumericEquals": 1, "Next": "Done" }],
                        "Default": UNDECLARED,
                    },
                    "Done": { "Type": "Succeed" },
                },
            }),
            "/States/C/Default",
            vec![],
        ),
        (
            "a Catch catcher's Next",
            json!({
                "StartAt": "T",
                "States": {
                    "T": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                        "Catch": [
                            { "ErrorEquals": ["States.Timeout"], "Next": "Done" },
                            { "ErrorEquals": ["States.ALL"], "Next": UNDECLARED },
                        ],
                        "End": true,
                    },
                    "Done": { "Type": "Succeed" },
                },
            }),
            "/States/T/Catch[1]/Next",
            vec![],
        ),
        (
            "a Next inside a Map's ItemProcessor",
            json!({
                "StartAt": "M",
                "States": {
                    "M": {
                        "Type": "Map",
                        "ItemProcessor": {
                            "StartAt": "I",
                            "States": {
                                "I": { "Type": "Pass", "Next": UNDECLARED },
                                "Stop": { "Type": "Succeed" },
                            },
                        },
                        "End": true,
                    },
                },
            }),
            "/States/M/ItemProcessor/States/I/Next",
            vec!["/States/M/ItemProcessor/States/Stop"],
        ),
        (
            "a StartAt inside a Parallel branch",
            json!({
                "StartAt": "P",
                "States": {
                    "P": {
                        "Type": "Parallel",
                        "Branches": [{
                            "StartAt": UNDECLARED,
                            "States": { "B": { "Type": "Succeed" } },
                        }],
                        "End": true,
                    },
                },
            }),
            "/States/P/Branches[0]/StartAt",
            vec!["/States/P/Branches[0]/States/B"],
        ),
    ];

    rows.into_iter()
        .map(|(case, definition, location, unreachable)| TargetCase {
            case,
            definition: definition.to_string(),
            location: location.to_string(),
            unreachable,
        })
        .collect()
}

/// An unresolved transition target is reported at the member that names it, as the
/// target it named under the transition field the language calls `Next` — the same
/// construction whichever member that is — beside the states the dangling transition
/// leaves unreached, and it fails the definition, so a create with it is refused.
#[tokio::test]
async fn an_unresolved_transition_target_is_reported_at_the_member_that_names_it() {
    let emulator = Emulator::new();

    for (index, row) in target_cases().into_iter().enumerate() {
        let TargetCase {
            case,
            definition,
            location,
            unreachable,
        } = row;
        let body = validate(
            &emulator,
            case,
            &json!({ "definition": definition, "severity": SEVERITY_WARNING }),
        )
        .await;
        assert_eq!(
            result_of(&body),
            VALIDATION_RESULT_FAIL,
            "case {case:?}: a target no state declares fails the definition, got {body:?}"
        );
        let mut expected = vec![error_at(
            MISSING_TRANSITION_TARGET,
            &location,
            &format!("Missing 'Next' target: {UNDECLARED}"),
        )];
        for state in unreachable {
            let name = state.rsplit('/').next().unwrap_or_default();
            expected.push(error_at(
                MISSING_TRANSITION_TARGET,
                state,
                &format!("State \"{name}\" is not reachable."),
            ));
        }
        let found = diagnostics(&body);
        assert_eq!(
            found, expected,
            "case {case:?}: expected exactly the diagnostics {expected:?}, got {found:?}"
        );

        let (status, code) = created(&emulator, &format!("dangling-{index}"), &definition).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected the create to refuse the definition, got {status} {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a state machine must declare somewhere to stop
// ---------------------------------------------------------------------------

/// A state machine that declares no state ending an execution is reported as such,
/// located at the machine — and one that declares one, by any of the forms the
/// language gives for it, is not.
#[tokio::test]
async fn a_state_machine_declaring_no_terminal_state_is_reported() {
    let emulator = Emulator::new();

    // Rows differ along WHICH form declares the terminal state (a `Succeed`, a
    // `Fail`, `"End": true`), whether one is declared at all, and at which level of
    // nesting the machine missing one sits. The last element of each row is the state
    // the definition leaves unreached, if any: that is a problem of its own, and not
    // the one under test.
    let rows: Vec<(&str, Value, Option<&str>, Option<&str>)> = vec![
        (
            "a cycle with nothing that ends it",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Next": "B" },
                    "B": { "Type": "Pass", "Next": "A" },
                },
            }),
            // The machine as a whole is what is missing a terminal state, so the
            // diagnostic carries no location at all.
            Some(""),
            None,
        ),
        (
            "a single state looping onto itself",
            json!({
                "StartAt": "A",
                "States": { "A": { "Type": "Pass", "Next": "A" } },
            }),
            Some(""),
            None,
        ),
        (
            "a Map's ItemProcessor with nothing that ends it",
            json!({
                "StartAt": "M",
                "States": {
                    "M": {
                        "Type": "Map",
                        "ItemProcessor": {
                            "StartAt": "I",
                            "States": { "I": { "Type": "Pass", "Next": "I" } },
                        },
                        "End": true,
                    },
                },
            }),
            Some("/States/M/ItemProcessor"),
            None,
        ),
        (
            "a Parallel branch with nothing that ends it",
            json!({
                "StartAt": "P",
                "States": {
                    "P": {
                        "Type": "Parallel",
                        "Branches": [{
                            "StartAt": "B",
                            "States": { "B": { "Type": "Pass", "Next": "B" } },
                        }],
                        "End": true,
                    },
                },
            }),
            Some("/States/P/Branches[0]"),
            None,
        ),
        (
            // The rule is read off the states a machine DECLARES: what it reports is
            // that the workflow has no terminal state, and a declared one is one it
            // has, whether or not this definition's transitions lead to it — which is
            // reported on its own, as the state nothing reaches.
            "a cycle beside a terminal state no run of it reaches",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Next": "B" },
                    "B": { "Type": "Pass", "Next": "A" },
                    "Done": { "Type": "Succeed" },
                },
            }),
            None,
            Some("Done"),
        ),
        (
            "a cycle a Succeed state ends",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Next": "B" },
                    "B": {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.go", "IsPresent": true, "Next": "A" }],
                        "Default": "Done",
                    },
                    "Done": { "Type": "Succeed" },
                },
            }),
            None,
            None,
        ),
        (
            "a cycle a Fail state ends",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Next": "B" },
                    "B": {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.go", "IsPresent": true, "Next": "A" }],
                        "Default": "Stop",
                    },
                    "Stop": { "Type": "Fail", "Error": "Stopped" },
                },
            }),
            None,
            None,
        ),
        (
            "a chain one state of which declares End",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Next": "B" },
                    "B": { "Type": "Pass", "End": true },
                },
            }),
            None,
            None,
        ),
        (
            "a Map whose processor and whose machine each end",
            json!({
                "StartAt": "M",
                "States": {
                    "M": {
                        "Type": "Map",
                        "ItemProcessor": {
                            "StartAt": "I",
                            "States": { "I": { "Type": "Pass", "End": true } },
                        },
                        "End": true,
                    },
                },
            }),
            None,
            None,
        ),
    ];

    for (index, (case, definition, expected, unreached)) in rows.into_iter().enumerate() {
        let definition = definition.to_string();
        let body = validate(&emulator, case, &json!({ "definition": definition })).await;
        let found = of_code(&body, MISSING_END_STATE);
        match expected {
            Some(location) => {
                let expected = if location.is_empty() {
                    json!({
                        "severity": SEVERITY_ERROR,
                        "code": MISSING_END_STATE,
                        "message": "Workflow has no terminal state",
                    })
                } else {
                    error_at(MISSING_END_STATE, location, "Workflow has no terminal state")
                };
                assert_eq!(
                    found,
                    vec![expected.clone()],
                    "case {case:?}: expected the diagnostic {expected:?}, got {body:?}"
                );
                assert_eq!(
                    result_of(&body),
                    VALIDATION_RESULT_FAIL,
                    "case {case:?}: a machine that cannot stop fails verification, got {body:?}"
                );
                let (status, code) =
                    created(&emulator, &format!("endless-{index}"), &definition).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "InvalidDefinition"),
                    "case {case:?}: expected the create to refuse it, got {status} {code}"
                );
            }
            None => {
                assert!(
                    found.is_empty(),
                    "case {case:?}: a machine that declares somewhere to stop earns no \
                     {MISSING_END_STATE}, got {found:?}"
                );
                // A state nothing reaches is the only other problem these rows carry,
                // and it is an error of its own: the definition that carries one is
                // refused, the definition that carries none is created.
                let (expected_result, expected_create) = match unreached {
                    Some(state) => {
                        let unreachable = of_code(&body, MISSING_TRANSITION_TARGET);
                        let expected = vec![error_at(
                            MISSING_TRANSITION_TARGET,
                            &format!("/States/{state}"),
                            &format!("State \"{state}\" is not reachable."),
                        )];
                        assert_eq!(
                            unreachable, expected,
                            "case {case:?}: expected the diagnostics {expected:?}, got {body:?}"
                        );
                        (VALIDATION_RESULT_FAIL, 400)
                    }
                    None => (VALIDATION_RESULT_OK, 200),
                };
                assert_eq!(
                    result_of(&body),
                    expected_result,
                    "case {case:?}: expected the result {expected_result}, got {body:?}"
                );
                let (status, code) =
                    created(&emulator, &format!("ending-{index}"), &definition).await;
                assert_eq!(
                    status, expected_create,
                    "case {case:?}: expected the create to answer {expected_create}, got {status} \
                     {code}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a variable read where no run has certainly assigned it
// ---------------------------------------------------------------------------

/// A field reading a variable that some run can reach without having assigned it earns
/// one warning per field, located at that field; a variable every path to the read
/// assigns earns none.
#[tokio::test]
async fn a_variable_no_run_certainly_assigned_is_reported_per_field() {
    let emulator = Emulator::new();

    // Rows differ along the form of the read (a JSONata expression, a JSONPath
    // reference field), how many fields of one state read one, where the assignment
    // sits relative to the read (nowhere, a preceding state, one branch only, every
    // branch, the reading state itself), how deeply the reading state is nested, and
    // the terms that are NOT variables at all.
    let rows: Vec<VariableCase> = vec![
        (
            "a JSONata Output reading a variable nothing assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "A",
                "States": { "A": { "Type": "Pass", "Output": "{% $alpha %}", "End": true } },
            }),
            vec![("/States/A/Output", "alpha")],
        ),
        (
            "two fields of one Output, each reading its own variable",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "A",
                "States": {
                    "A": {
                        "Type": "Pass",
                        "Output": { "x": "{% $alpha %}", "y": "{% $beta %}" },
                        "End": true,
                    },
                },
            }),
            vec![
                ("/States/A/Output/x", "alpha"),
                ("/States/A/Output/y", "beta"),
            ],
        ),
        (
            "a chain of states each reading a variable nothing assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Output": "{% $alpha %}", "Next": "B" },
                    "B": { "Type": "Pass", "Output": "{% $beta %}", "Next": "C" },
                    "C": { "Type": "Pass", "Output": "{% $gamma %}", "End": true },
                },
            }),
            vec![
                ("/States/A/Output", "alpha"),
                ("/States/B/Output", "beta"),
                ("/States/C/Output", "gamma"),
            ],
        ),
        (
            "a variable a preceding state assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Assign": { "m": 1 }, "Next": "B" },
                    "B": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                },
            }),
            vec![],
        ),
        (
            "a variable the reading state itself assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "A",
                "States": {
                    "A": {
                        "Type": "Pass",
                        "Assign": { "m": "{% 1 %}" },
                        "Output": "{% $m %}",
                        "End": true,
                    },
                },
            }),
            vec![],
        ),
        (
            "a variable only one branch of a Choice assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "C",
                "States": {
                    "C": {
                        "Type": "Choice",
                        "Choices": [{ "Condition": "{% true %}", "Next": "S" }],
                        "Default": "J",
                    },
                    "S": { "Type": "Pass", "Assign": { "m": 1 }, "Next": "J" },
                    "J": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                },
            }),
            vec![("/States/J/Output", "m")],
        ),
        (
            "a variable every branch of a Choice assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "C",
                "States": {
                    "C": {
                        "Type": "Choice",
                        "Choices": [{ "Condition": "{% true %}", "Next": "S" }],
                        "Default": "T",
                    },
                    "S": { "Type": "Pass", "Assign": { "m": 1 }, "Next": "J" },
                    "T": { "Type": "Pass", "Assign": { "m": 2 }, "Next": "J" },
                    "J": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                },
            }),
            vec![],
        ),
        (
            "a variable the state's own Assign and its catcher's both assign",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "T",
                "States": {
                    "T": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                        "Assign": { "m": 1 },
                        "Catch": [{
                            "ErrorEquals": ["States.ALL"],
                            "Assign": { "m": 2 },
                            "Next": "J",
                        }],
                        "Next": "J",
                    },
                    "J": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                },
            }),
            vec![],
        ),
        (
            "a variable only a Catch catcher assigns, read on both of the state's paths",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "T",
                "States": {
                    "T": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                        "Catch": [{
                            "ErrorEquals": ["States.ALL"],
                            "Assign": { "m": 2 },
                            "Next": "Caught",
                        }],
                        "Next": "Ran",
                    },
                    // Reached because the work FAILED, which is the only path the
                    // catcher's own assignment runs on.
                    "Caught": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                    // Reached because it completed, so nothing assigned the variable.
                    "Ran": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                },
            }),
            vec![("/States/Ran/Output", "m")],
        ),
        (
            "a JSONPath reference field reading a variable nothing assigns",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Parameters": { "x.$": "$alpha" }, "End": true },
                },
            }),
            vec![("/States/A/Parameters/x.$", "alpha")],
        ),
        (
            "a JSONPath reference field reading a variable a preceding state assigns",
            json!({
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Assign": { "alpha.$": "$.in" }, "Next": "B" },
                    "B": { "Type": "Pass", "Parameters": { "x.$": "$alpha" }, "End": true },
                },
            }),
            vec![],
        ),
        (
            "a state of a Map's ItemProcessor reading a variable nothing assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "M",
                "States": {
                    "M": {
                        "Type": "Map",
                        "ItemProcessor": {
                            "StartAt": "I",
                            "States": {
                                "I": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                            },
                        },
                        "End": true,
                    },
                },
            }),
            vec![("/States/M/ItemProcessor/States/I/Output", "m")],
        ),
        (
            "a state of a Map's ItemProcessor reading one only the Map state assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "M",
                "States": {
                    "M": {
                        "Type": "Map",
                        // Evaluated when the iterations have COMPLETED, so a state
                        // inside the processor cannot count on it.
                        "Assign": { "m": 1 },
                        "ItemProcessor": {
                            "StartAt": "I",
                            "States": {
                                "I": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                            },
                        },
                        "End": true,
                    },
                },
            }),
            vec![("/States/M/ItemProcessor/States/I/Output", "m")],
        ),
        (
            "a state of a Map's ItemProcessor reading one the state before the Map assigns",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Assign": { "m": 1 }, "Next": "M" },
                    "M": {
                        "Type": "Map",
                        "ItemProcessor": {
                            "StartAt": "I",
                            "States": {
                                "I": { "Type": "Pass", "Output": "{% $m %}", "End": true },
                            },
                        },
                        "End": true,
                    },
                },
            }),
            vec![],
        ),
        (
            "the reserved context object, which is no variable",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Output": "{% $states.input %}", "End": true },
                },
            }),
            vec![],
        ),
        (
            "a function invocation, which is no variable",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "A",
                "States": {
                    "A": { "Type": "Pass", "Output": "{% $string(1) %}", "End": true },
                },
            }),
            vec![],
        ),
        (
            "the JSONPath context object, which is no variable",
            json!({
                "StartAt": "A",
                "States": {
                    "A": {
                        "Type": "Pass",
                        "Parameters": { "x.$": "$$.Execution.Name" },
                        "End": true,
                    },
                },
            }),
            vec![],
        ),
    ];

    for (case, definition, expected) in rows {
        let definition = definition.to_string();
        let body = validate(
            &emulator,
            case,
            &json!({ "definition": definition, "severity": SEVERITY_WARNING }),
        )
        .await;
        let expected: Vec<Value> = expected
            .iter()
            .map(|(location, name)| {
                warning_at(
                    POSSIBLY_UNDEFINED_VARIABLE,
                    location,
                    &format!("Variable '{name}' is possibly not defined."),
                )
            })
            .collect();
        let found = of_code(&body, POSSIBLY_UNDEFINED_VARIABLE);
        assert_eq!(
            found, expected,
            "case {case:?}: expected the warnings {expected:?}, got {body:?}"
        );
        // A warning never prevents a create: "the result can be OK even when
        // diagnostic warnings are present in the response".
        assert_eq!(
            result_of(&body),
            VALIDATION_RESULT_OK,
            "case {case:?}: a warning does not fail the definition, got {body:?}"
        );
    }

    // And the definition every row above warns about is still created.
    let definition = json!({
        "QueryLanguage": "JSONata",
        "StartAt": "A",
        "States": { "A": { "Type": "Pass", "Output": "{% $alpha %}", "End": true } },
    })
    .to_string();
    let (status, code) = created(&emulator, "warned-but-created", &definition).await;
    assert_eq!(
        status, 200,
        "a definition carrying only this warning is created, got {status} {code}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: maxResults caps the reported warnings, and truncated says so
// ---------------------------------------------------------------------------

/// `maxResults` bounds the warnings reported exactly as it bounds the errors, and
/// `truncated` is true exactly when it dropped one.
#[tokio::test]
async fn max_results_caps_the_reported_warnings_and_reports_the_cut() {
    let emulator = Emulator::new();

    /// How many states read a variable nothing assigns, so the warning count is
    /// known exactly.
    const WARNINGS: usize = 3;

    let definition = json!({
        "QueryLanguage": "JSONata",
        "StartAt": "A",
        "States": {
            "A": { "Type": "Pass", "Output": "{% $alpha %}", "Next": "B" },
            "B": { "Type": "Pass", "Output": "{% $beta %}", "Next": "C" },
            "C": { "Type": "Pass", "Output": "{% $gamma %}", "End": true },
        },
    })
    .to_string();

    // Rows differ along how many were asked for: below the count, at it, past it, and
    // 0, which asks for the documented default.
    for (case, asked, reported) in [
        ("one of three", 1_usize, 1_usize),
        ("two of three", 2, 2),
        ("all three", 3, WARNINGS),
        ("one more than there are", 4, WARNINGS),
        ("zero, which asks for the default", 0, WARNINGS),
    ] {
        let body = validate(
            &emulator,
            case,
            &json!({
                "definition": definition,
                "severity": SEVERITY_WARNING,
                "maxResults": asked,
            }),
        )
        .await;
        let found = diagnostics(&body);
        assert_eq!(
            found.len(),
            reported,
            "case {case:?}: expected {reported} diagnostics, got {found:?}"
        );
        assert_eq!(
            body.get("truncated").and_then(Value::as_bool),
            Some(reported < WARNINGS),
            "case {case:?}: truncated is true exactly when a diagnostic was dropped, got {body:?}"
        );
        assert_eq!(
            result_of(&body),
            VALIDATION_RESULT_OK,
            "case {case:?}: the result is the verdict on the definition, got {body:?}"
        );
    }

    // At the default severity the warnings are not reported at all, and nothing was
    // dropped that this request reports.
    let body = validate(
        &emulator,
        "the same definition at ERROR",
        &json!({ "definition": definition, "maxResults": 1 }),
    )
    .await;
    assert!(
        diagnostics(&body).is_empty(),
        "an ERROR-severity request reports no warning, got {body:?}"
    );
    assert_eq!(
        body.get("truncated").and_then(Value::as_bool),
        Some(false),
        "no diagnostic of the severity asked for was dropped, got {body:?}"
    );
}

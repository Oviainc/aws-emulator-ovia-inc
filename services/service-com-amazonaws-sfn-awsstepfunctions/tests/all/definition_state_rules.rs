// SPDX-License-Identifier: CC0-1.0

//! What the ASL static validator reports about ONE state: the type it declares, the
//! reserved terms its JSONata expressions read, and where a state written in the
//! other query language is blamed.
//!
//! `ValidateStateMachineDefinition` is the pre-deploy gate for these rules
//! ("validation will look for problems in your state machine definition and return a
//! result and a list of diagnostic elements"), and `CreateStateMachine` refuses a
//! definition for the first of the same walk's errors, so every row below is asserted
//! through both: what the validation reports and what a create does with the same
//! definition cannot disagree.
//!
//! The rules under test:
//!
//! 1. `Type` names one of the state types the Amazon States Language defines, and a
//!    state naming any other one is an ERROR — the catalog is
//!    [`STATE_TYPES`](service_com_amazonaws_sfn_awsstepfunctions::state::asl::STATE_TYPES),
//!    and every entry of it is driven here, so the rule cannot be satisfied by
//!    listing types one by one;
//! 2. a reserved `$states` term of a state's own outcome is an ERROR wherever the
//!    language does not bind it — `$states.errorOutput` outside the `Catch` that
//!    handles a failure, `$states.result` in a state whose work produces no result —
//!    reported as `UNSUPPORTED_JSONATA_EXPRESSION`, once per FIELD that reads it, at
//!    that field's own location;
//! 3. a state of a JSONata state machine that selects JSONPath is an ERROR located
//!    at the overriding STATE, and the mirror image (a JSONata state of a JSONPath
//!    machine) is no error at all.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    SCHEMA_VALIDATION_FAILED, STATE_TYPES, UNSUPPORTED_JSONATA_EXPRESSION,
};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    SEVERITY_ERROR, SEVERITY_WARNING, VALIDATION_RESULT_FAIL, VALIDATION_RESULT_OK,
};

use crate::state_machine_lifecycle::{error_code, Emulator, ROLE_ARN};

/// Every diagnostic a validation reported, as the triple a client keys off:
/// `(severity, code, location)`.
///
/// The request asks for `WARNING`, which reports BOTH severities, so a row asserting
/// the whole list also pins that no diagnostic beyond the ones it names is reported.
async fn diagnose(emulator: &Emulator, case: &str, definition: &str) -> (String, Vec<Value>) {
    let (status, body) = emulator
        .call(
            "ValidateStateMachineDefinition",
            &json!({ "definition": definition, "severity": SEVERITY_WARNING }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ValidateStateMachineDefinition reports a verdict rather than refusing, \
         got {status} with {body:?}"
    );
    let result = body
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or("<no result>")
        .to_string();
    let reported = body
        .get("diagnostics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|diagnostic| {
            json!([
                diagnostic.get("severity").cloned().unwrap_or(Value::Null),
                diagnostic.get("code").cloned().unwrap_or(Value::Null),
                diagnostic.get("location").cloned().unwrap_or(Value::Null),
            ])
        })
        .collect();
    (result, reported)
}

/// What `CreateStateMachine` does with the same definition: `Ok(())` when it is
/// created, and the `(status, errorCode)` of the refusal when it is not.
async fn create(emulator: &Emulator, name: &str, definition: &str) -> Result<(), (u16, String)> {
    let (status, body) = emulator
        .call(
            "CreateStateMachine",
            &json!({ "name": name, "definition": definition, "roleArn": ROLE_ARN }),
        )
        .await;
    if status == 200 {
        return Ok(());
    }
    Err((status, error_code(&body)))
}

/// Assert that `definition` carries exactly `expected` and that a create agrees with
/// the verdict that follows from it.
async fn expect(emulator: &Emulator, index: usize, case: &str, definition: &str, expected: &[Value]) {
    let (result, reported) = diagnose(emulator, case, definition).await;
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the diagnostics {expected:?}, got {reported:?}"
    );

    let fails = expected
        .iter()
        .any(|diagnostic| diagnostic[0] == json!(SEVERITY_ERROR));
    let expected_result = if fails {
        VALIDATION_RESULT_FAIL
    } else {
        VALIDATION_RESULT_OK
    };
    assert_eq!(
        result, expected_result,
        "case {case:?}: expected result {expected_result}, got {result} with {reported:?}"
    );

    let created = create(emulator, &format!("state-rule-{index}"), definition).await;
    if fails {
        assert_eq!(
            created,
            Err((400, "InvalidDefinition".to_string())),
            "case {case:?}: a definition the validation fails cannot be created, got {created:?}"
        );
    } else {
        assert_eq!(
            created,
            Ok(()),
            "case {case:?}: a definition the validation passes is created, got {created:?}"
        );
    }
}

/// One ERROR diagnostic, as the triple [`diagnose`] reports.
fn error_at(code: &str, location: &str) -> Value {
    json!([SEVERITY_ERROR, code, location])
}

// ---------------------------------------------------------------------------
// Rule 1: `Type` names a state type the language defines
// ---------------------------------------------------------------------------

/// A minimal state of `state_type`, inside a state machine that declares the
/// `Succeed` a continuing state transitions to — and nothing else, since a state no
/// run of the machine reaches is a problem of its own.
///
/// The members are the ones each type needs to satisfy every OTHER rule of the walk,
/// so the only thing a row can fail on is its `Type`: a continuing type carries
/// `Next`, a terminal one carries none, and `Choice` branches through its own members.
fn machine_of(state_type: &str) -> String {
    let state = match state_type {
        "Choice" => json!({
            "Type": "Choice",
            "Choices": [{ "Variable": "$.x", "NumericEquals": 1, "Next": "Done" }],
            "Default": "Done"
        }),
        "Fail" => json!({ "Type": "Fail", "Error": "Boom", "Cause": "on purpose" }),
        "Succeed" => json!({ "Type": "Succeed" }),
        "Map" => json!({
            "Type": "Map",
            "ItemProcessor": { "StartAt": "P", "States": { "P": { "Type": "Pass", "End": true } } },
            "Next": "Done"
        }),
        "Parallel" => json!({
            "Type": "Parallel",
            "Branches": [{ "StartAt": "B", "States": { "B": { "Type": "Pass", "End": true } } }],
            "Next": "Done"
        }),
        "Task" => json!({
            "Type": "Task",
            "Resource": "arn:aws:lambda:us-west-2:000000000000:function:f",
            "Next": "Done"
        }),
        "Wait" => json!({ "Type": "Wait", "Seconds": 1, "Next": "Done" }),
        // Every other type is driven with the members a `Pass` needs, which is the
        // least a state can carry: a row that fails does so on its `Type` alone.
        _ => json!({ "Type": state_type, "Next": "Done" }),
    };
    // A type that ENDS an execution transitions nowhere, so the machine declares it
    // alone; every other row names `Done` and so declares it.
    if matches!(state_type, "Fail" | "Succeed") {
        return json!({ "StartAt": "S", "States": { "S": state } }).to_string();
    }
    json!({
        "StartAt": "S",
        "States": { "S": state, "Done": { "Type": "Succeed" } }
    })
    .to_string()
}

/// Every type the language defines passes verification, and a type it does not define
/// is an ERROR at the state's own `Type` member — so a definition declaring one can
/// neither pass this gate nor be created.
#[tokio::test]
async fn a_type_the_language_does_not_define_is_refused() {
    let emulator = Emulator::new();

    // The accepted rows are the CATALOG itself
    // (`service_com_amazonaws_sfn_awsstepfunctions::state::asl::STATE_TYPES`, the
    // eight types the Amazon States Language's `Type` member selects), so no entry of
    // it can be forgotten here.
    assert_eq!(
        STATE_TYPES.len(),
        8,
        "case the catalog: expected the language's eight state types, got {STATE_TYPES:?}"
    );
    for (index, state_type) in STATE_TYPES.iter().enumerate() {
        let case = format!("the defined type {state_type}");
        expect(&emulator, index, &case, &machine_of(state_type), &[]).await;
    }

    // The refused rows differ along HOW the declared type misses the catalog: a name
    // the language has no such state for, a near-miss of a defined one, the wrong
    // case, a defined name with whitespace around it, and the empty string.
    let outside: Vec<&str> = vec![
        "NotARealStateType",
        "Succeeded",
        "pass",
        "TASK",
        " Wait",
        "Pass ",
        "",
    ];
    for (index, state_type) in outside.into_iter().enumerate() {
        let case = format!("the undefined type {state_type:?}");
        expect(
            &emulator,
            STATE_TYPES.len() + index,
            &case,
            &machine_of(state_type),
            &[error_at(SCHEMA_VALIDATION_FAILED, "/States/S/Type")],
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a reserved term is reported where the language does not bind it
// ---------------------------------------------------------------------------

/// One JSONata state machine, and the diagnostics its reserved-term reads earn.
struct Scope {
    case: &'static str,
    definition: String,
    expected: Vec<Value>,
}

/// A JSONata state machine whose state `S` is `state`, with a `Succeed` beside it for
/// a state that transitions.
fn jsonata_machine(state: Value) -> String {
    json!({
        "QueryLanguage": "JSONata",
        "StartAt": "S",
        "States": { "S": state, "Done": { "Type": "Succeed" } }
    })
    .to_string()
}

/// One ERROR for a reserved term read at `location`.
fn unsupported(location: &str) -> Value {
    error_at(UNSUPPORTED_JSONATA_EXPRESSION, location)
}

/// The rows rule 2 spans: which term is read, which state type reads it, which member
/// carries the expression, how deeply inside that member the field sits, and how many
/// fields read it.
fn scopes() -> Vec<Scope> {
    let result_reader = |state_type: &str, member: &str| {
        jsonata_machine(json!({
            "Type": state_type,
            member: { "x": "{% $states.result %}" },
            "Resource": "arn:aws:lambda:us-west-2:000000000000:function:f",
            "Next": "Done"
        }))
    };
    vec![
        // ---- $states.result: bound only where the state's work has one ----
        Scope {
            case: "a Pass state reads the result it never produces",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% $states.result %}" },
                "Next": "Done"
            })),
            expected: vec![unsupported("/States/S/Output/x")],
        },
        Scope {
            case: "a Wait state reads the result it never produces",
            definition: jsonata_machine(json!({
                "Type": "Wait",
                "Seconds": 1,
                "Output": { "x": "{% $states.result %}" },
                "Next": "Done"
            })),
            expected: vec![unsupported("/States/S/Output/x")],
        },
        Scope {
            case: "a Pass state assigns from the result it never produces",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Assign": { "seen": "{% $states.result.count %}" },
                "Next": "Done"
            })),
            expected: vec![unsupported("/States/S/Assign/seen")],
        },
        Scope {
            case: "a Task state reads its own result",
            definition: result_reader("Task", "Output"),
            expected: vec![],
        },
        Scope {
            case: "a Map state reads its own result",
            definition: jsonata_machine(json!({
                "Type": "Map",
                "ItemProcessor": {
                    "StartAt": "P",
                    "States": { "P": { "Type": "Pass", "End": true } }
                },
                "Output": { "x": "{% $states.result %}" },
                "Next": "Done"
            })),
            expected: vec![],
        },
        Scope {
            case: "a Parallel state reads its own result",
            definition: jsonata_machine(json!({
                "Type": "Parallel",
                "Branches": [{ "StartAt": "B", "States": { "B": { "Type": "Pass", "End": true } } }],
                "Output": { "x": "{% $states.result %}" },
                "Next": "Done"
            })),
            expected: vec![],
        },
        Scope {
            case: "a Task reads the result in the Catch that handles its failure",
            definition: jsonata_machine(json!({
                "Type": "Task",
                "Resource": "arn:aws:lambda:us-west-2:000000000000:function:f",
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "x": "{% $states.result %}" },
                    "Next": "Done"
                }],
                "Next": "Done"
            })),
            expected: vec![unsupported("/States/S/Catch[0]/Output/x")],
        },
        Scope {
            case: "a Pass state reads its input",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% $states.input.x %}" },
                "Next": "Done"
            })),
            expected: vec![],
        },
        // ---- $states.errorOutput: bound only inside a Catch ----
        Scope {
            case: "two Output fields of a Task read the error output",
            definition: jsonata_machine(json!({
                "Type": "Task",
                "Resource": "arn:aws:lambda:us-west-2:000000000000:function:f",
                "Output": {
                    "error": "{% $states.errorOutput.Error %}",
                    "cause": "{% $states.errorOutput.Cause %}"
                },
                "Next": "Done"
            })),
            expected: vec![
                unsupported("/States/S/Output/cause"),
                unsupported("/States/S/Output/error"),
            ],
        },
        Scope {
            case: "a Task reads the error output in the Catch that handles its failure",
            definition: jsonata_machine(json!({
                "Type": "Task",
                "Resource": "arn:aws:lambda:us-west-2:000000000000:function:f",
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "error": "{% $states.errorOutput.Error %}" },
                    "Next": "Done"
                }],
                "Next": "Done"
            })),
            expected: vec![],
        },
        Scope {
            case: "a Pass state reads the error output from a nested field",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "detail": { "why": "{% $states.errorOutput %}" } },
                "Next": "Done"
            })),
            expected: vec![unsupported("/States/S/Output/detail/why")],
        },
        Scope {
            case: "a Pass state reads the error output from inside a list",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "all": ["kept", "{% $states.errorOutput %}"] },
                "Next": "Done"
            })),
            expected: vec![unsupported("/States/S/Output/all[1]")],
        },
        Scope {
            case: "a Pass state whose whole Output is the error output",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": "{% $states.errorOutput %}",
                "Next": "Done"
            })),
            expected: vec![unsupported("/States/S/Output")],
        },
        Scope {
            case: "a Pass state reads both terms out of scope",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "a": "{% $states.errorOutput %}", "b": "{% $states.result %}" },
                "Next": "Done"
            })),
            expected: vec![
                unsupported("/States/S/Output/a"),
                unsupported("/States/S/Output/b"),
            ],
        },
    ]
}

/// Each reserved term is reported exactly where the language does not bind it, under
/// the code that says the problem is the EXPRESSION, once per field that reads it.
#[tokio::test]
async fn a_reserved_term_read_out_of_scope_is_refused_per_field() {
    let emulator = Emulator::new();

    for (
        index,
        Scope {
            case,
            definition,
            expected,
        },
    ) in scopes().into_iter().enumerate()
    {
        expect(&emulator, index, case, &definition, &expected).await;
    }
}

/// The diagnostic states the FIELD the expression read: the term is not bound in that
/// scope, so there is no such field to read, and the message names the field expression
/// that was written — the term with the member accessors after it — and nothing of the
/// state or the member that carried it.
#[tokio::test]
async fn the_diagnostic_states_the_field_the_expression_read() {
    let emulator = Emulator::new();

    // Rows differ along WHICH term is out of scope, which member of which state type
    // reads it, and whether the written field expression is the bare term or the term
    // followed by the member the expression actually wanted.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "the result of a state that produces none",
            jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% $states.result %}" },
                "Next": "Done"
            })),
            "$states.result",
        ),
        (
            "the result read in a Catch",
            jsonata_machine(json!({
                "Type": "Task",
                "Resource": "arn:aws:lambda:us-west-2:000000000000:function:f",
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Assign": { "seen": "{% $states.result %}" },
                    "Next": "Done"
                }],
                "Next": "Done"
            })),
            "$states.result",
        ),
        (
            "the error output outside a Catch",
            jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "why": "{% $states.errorOutput.Cause %}" },
                "Next": "Done"
            })),
            "$states.errorOutput.Cause",
        ),
        (
            "a member of the result of a state that produces none",
            jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% $states.result.count %}" },
                "Next": "Done"
            })),
            "$states.result.count",
        ),
    ];

    for (case, definition, term) in rows {
        let (status, body) = emulator
            .call(
                "ValidateStateMachineDefinition",
                &json!({ "definition": definition }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a verdict, got {status} with {body:?}"
        );
        let messages: Vec<String> = body
            .get("diagnostics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|diagnostic| {
                diagnostic
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
                    .to_string()
            })
            .collect();
        assert_eq!(
            messages,
            vec![format!("Field '{term}' does not exist.")],
            "case {case:?}: expected the service's sentence for a field that is not \
             bound, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the overriding STATE is what a cross-language override is blamed on
// ---------------------------------------------------------------------------

/// A state machine of one query language holding a state that selects the other one:
/// the override is refused in one direction only, and the diagnostic names the state
/// that declares it.
#[tokio::test]
async fn a_cross_language_state_is_blamed_on_the_state_itself() {
    let emulator = Emulator::new();

    // Rows differ along WHICH direction the override goes and how deeply the
    // overriding state is nested.
    let rows: Vec<(&str, String, Vec<Value>)> = vec![
        (
            "a JSONPath state of a JSONata machine",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "P",
                "States": {
                    "P": { "Type": "Pass", "QueryLanguage": "JSONPath", "Next": "Q" },
                    "Q": { "Type": "Pass", "Output": { "v": "{% $states.input.v %}" }, "End": true }
                }
            })
            .to_string(),
            vec![error_at(SCHEMA_VALIDATION_FAILED, "/States/P")],
        ),
        (
            "a JSONPath state inside a JSONata machine's ItemProcessor",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "M",
                "States": {
                    "M": {
                        "Type": "Map",
                        "ItemProcessor": {
                            "StartAt": "P",
                            "States": {
                                "P": { "Type": "Pass", "QueryLanguage": "JSONPath", "End": true }
                            }
                        },
                        "End": true
                    }
                }
            })
            .to_string(),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                "/States/M/ItemProcessor/States/P",
            )],
        ),
        (
            "a JSONPath state inside a JSONata machine's Parallel branch",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "R",
                "States": {
                    "R": {
                        "Type": "Parallel",
                        "Branches": [{
                            "StartAt": "B",
                            "States": {
                                "B": { "Type": "Pass", "QueryLanguage": "JSONPath", "End": true }
                            }
                        }],
                        "End": true
                    }
                }
            })
            .to_string(),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                "/States/R/Branches[0]/States/B",
            )],
        ),
        (
            "the mirror image: a JSONata state of a JSONPath machine",
            json!({
                "StartAt": "P",
                "States": {
                    "P": {
                        "Type": "Pass",
                        "QueryLanguage": "JSONata",
                        "Output": { "v": "{% $states.input.v %}" },
                        "End": true
                    }
                }
            })
            .to_string(),
            vec![],
        ),
    ];

    for (index, (case, definition, expected)) in rows.into_iter().enumerate() {
        expect(&emulator, index, case, &definition, &expected).await;
    }
}

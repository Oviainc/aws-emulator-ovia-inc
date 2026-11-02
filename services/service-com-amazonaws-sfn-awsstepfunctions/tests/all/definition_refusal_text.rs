// SPDX-License-Identifier: CC0-1.0

//! The TEXT the ASL validation walk hands a caller: the diagnostic a reserved term
//! read out of scope earns, the diagnostic a per-state query-language override earns,
//! and how a diagnostic is stated inside the `InvalidDefinition` a create or an update
//! is refused with.
//!
//! Driven end to end over this service's own wire protocol through the production
//! router (`crate::state_machine_lifecycle::Emulator`), so what is asserted is the
//! `diagnostics[].message` and the `$.message` a signed SDK request observes.
//!
//! The rules under test, each over a table whose rows differ along what the rule
//! generalises over rather than on the one definition that exposed it:
//!
//! 1. a diagnostic for a reserved `$states` term the expression's scope does not bind
//!    names the FIELD EXPRESSION that was written — the term together with the member
//!    accessors after it — so two fields reading two different members of the same
//!    term are two different sentences;
//! 2. a state of a JSONata state machine that selects JSONPath earns one fixed
//!    sentence about the `QueryLanguage` member, with the state it is about identified
//!    by `location` alone;
//! 3. `InvalidDefinition` states one subject per ERROR diagnostic the walk computed
//!    for the submitted definition, each `<code>: <message> at <location>` — the place
//!    joined to the detail by the bare word ` at ` — separated by `", "`.
//!
//! The harness (`Emulator`, `create_machine`, `ROLE_ARN`) and the `failure` reader are
//! the ones `state_machine_lifecycle` and `error_messages` already build; `tests/all.rs`
//! is this binary's crate root, so each is reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    SCHEMA_VALIDATION_FAILED, UNSUPPORTED_JSONATA_EXPRESSION,
};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    INVALID_STATE_MACHINE_DEFINITION, SEVERITY_ERROR, SEVERITY_WARNING, VALIDATION_RESULT_FAIL,
};

use crate::error_messages::failure;
use crate::state_machine_lifecycle::{create_machine, Emulator, HELLO_DEFINITION, ROLE_ARN};

/// A JSONata state machine whose state `S` is `state`, with a `Succeed` beside it for a
/// state that transitions.
fn jsonata_machine(state: Value) -> String {
    json!({
        "QueryLanguage": "JSONata",
        "StartAt": "S",
        "States": { "S": state, "Done": { "Type": "Succeed" } }
    })
    .to_string()
}

/// Every diagnostic `definition` earns as `(severity, code, location, message)`, in the
/// order the walk reports them — which is deterministic: the definition is walked in
/// the parsed document's own member order.
///
/// The request asks for `WARNING`, which reports BOTH severities, so a row asserting
/// the whole list also pins that nothing beyond what it names is reported.
async fn diagnostics(emulator: &Emulator, case: &str, definition: &str) -> Vec<Vec<String>> {
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
    body.get("diagnostics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|diagnostic| {
            ["severity", "code", "location", "message"]
                .iter()
                .map(|member| {
                    diagnostic
                        .get(*member)
                        .and_then(Value::as_str)
                        .unwrap_or("<absent>")
                        .to_string()
                })
                .collect()
        })
        .collect()
}

/// One ERROR diagnostic as [`diagnostics`] reports it.
fn error_at(code: &str, location: &str, message: &str) -> Vec<String> {
    vec![
        SEVERITY_ERROR.to_string(),
        code.to_string(),
        location.to_string(),
        message.to_string(),
    ]
}

/// What the service says about a field an expression read that its scope does not bind.
fn field_does_not_exist(field: &str) -> String {
    format!("Field '{field}' does not exist.")
}

// ---------------------------------------------------------------------------
// Rule 1: the diagnostic names the field expression that was written
// ---------------------------------------------------------------------------

/// One definition, and the `(location, message)` pairs its out-of-scope reads earn.
struct FieldRead {
    case: &'static str,
    definition: String,
    expected: Vec<(String, String)>,
}

/// The rows rule 1 spans: which reserved term is read, how many member accessors are
/// written after it, which member and state type carries the expression, how deeply
/// inside that member the field sits, and whether the field expression is the whole
/// value or part of a larger expression.
fn field_reads() -> Vec<FieldRead> {
    let at = |location: &str, field: &str| (location.to_string(), field_does_not_exist(field));
    vec![
        FieldRead {
            case: "the bare term, which is the whole field expression",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% $states.result %}" },
                "Next": "Done"
            })),
            expected: vec![at("/States/S/Output/x", "$states.result")],
        },
        FieldRead {
            case: "one member accessor after the term",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% $states.result.count %}" },
                "Next": "Done"
            })),
            expected: vec![at("/States/S/Output/x", "$states.result.count")],
        },
        FieldRead {
            case: "two accessors after the term",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Assign": { "seen": "{% $states.result.detail.code %}" },
                "Next": "Done"
            })),
            expected: vec![at("/States/S/Assign/seen", "$states.result.detail.code")],
        },
        FieldRead {
            case: "an accessor carrying digits and an underscore",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% $states.result.item_2 %}" },
                "Next": "Done"
            })),
            expected: vec![at("/States/S/Output/x", "$states.result.item_2")],
        },
        FieldRead {
            case: "the field expression inside a larger expression",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% 'seen ' & $states.errorOutput.Cause & '!' %}" },
                "Next": "Done"
            })),
            expected: vec![at("/States/S/Output/x", "$states.errorOutput.Cause")],
        },
        FieldRead {
            case: "two members of the same term, one per field",
            definition: jsonata_machine(json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                "Arguments": {},
                "Output": {
                    "error": "{% $states.errorOutput.Error %}",
                    "cause": "{% $states.errorOutput.Cause %}"
                },
                "Next": "Done"
            })),
            expected: vec![
                at("/States/S/Output/cause", "$states.errorOutput.Cause"),
                at("/States/S/Output/error", "$states.errorOutput.Error"),
            ],
        },
        FieldRead {
            case: "a field nested in a list inside the member",
            definition: jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "all": ["kept", "{% $states.errorOutput.Error %}"] },
                "Next": "Done"
            })),
            expected: vec![at("/States/S/Output/all/1", "$states.errorOutput.Error")],
        },
        FieldRead {
            case: "the term read in the Catch that handles a failure",
            definition: jsonata_machine(json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                "Arguments": {},
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Assign": { "seen": "{% $states.result.Payload %}" },
                    "Next": "Done"
                }],
                "Next": "Done"
            })),
            expected: vec![at(
                "/States/S/Catch/0/Assign/seen",
                "$states.result.Payload",
            )],
        },
    ]
}

/// Each diagnostic for a reserved term read out of scope states the field expression
/// the definition wrote, so two fields reading two members of the same term read as the
/// two distinct problems they are.
#[tokio::test]
async fn an_out_of_scope_read_is_stated_as_the_field_expression_written() {
    let emulator = Emulator::new();

    for FieldRead {
        case,
        definition,
        expected,
    } in field_reads()
    {
        let expected: Vec<Vec<String>> = expected
            .iter()
            .map(|(location, message)| {
                error_at(UNSUPPORTED_JSONATA_EXPRESSION, location, message)
            })
            .collect();
        let reported = diagnostics(&emulator, case, &definition).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the diagnostics {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the query-language clash is one fixed sentence, located at the state
// ---------------------------------------------------------------------------

/// The sentence the service answers a per-state `JSONPath` override inside a JSONata
/// state machine with, which carries nothing of the state it is about.
const QUERY_LANGUAGE_CLASH: &str =
    "'QueryLanguage' can not be 'JSONPath' if set to 'JSONata' for whole state machine";

/// A JSONata state machine whose state named `name` overrides the query language, at
/// the top level and once per nesting the language declares a machine in.
fn clashing_machines(name: &str) -> Vec<(&'static str, String, String)> {
    let overriding = json!({ "Type": "Pass", "QueryLanguage": "JSONPath", "End": true });
    vec![
        (
            "a state of the machine itself",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": name,
                "States": { name: overriding.clone() }
            })
            .to_string(),
            format!("/States/{name}"),
        ),
        (
            "a state of a Map's ItemProcessor",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "M",
                "States": {
                    "M": {
                        "Type": "Map",
                        "ItemProcessor": { "StartAt": name, "States": { name: overriding.clone() } },
                        "End": true
                    }
                }
            })
            .to_string(),
            format!("/States/M/ItemProcessor/States/{name}"),
        ),
        (
            "a state of a Parallel branch",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "R",
                "States": {
                    "R": {
                        "Type": "Parallel",
                        "Branches": [{ "StartAt": name, "States": { name: overriding } }],
                        "End": true
                    }
                }
            })
            .to_string(),
            format!("/States/R/Branches/0/States/{name}"),
        ),
    ]
}

/// The clash earns the one sentence about the `QueryLanguage` member whatever the
/// overriding state is called and wherever it sits, and the state it is about is named
/// by `location`: the message asserted below is one FIXED string across rows whose
/// state names differ, which is what shows no name is interpolated into it.
#[tokio::test]
async fn the_query_language_clash_is_one_sentence_located_at_the_state() {
    let emulator = Emulator::new();

    // Rows differ along the overriding state's NAME — including a name no sentence
    // could carry by accident — and along how deeply that state is nested.
    for name in ["P", "StateWithAVeryParticularName", "s-1"] {
        for (where_it_sits, definition, location) in clashing_machines(name) {
            let case = format!("the state {name:?} as {where_it_sits}");
            let reported = diagnostics(&emulator, &case, &definition).await;
            let expected = vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &location,
                QUERY_LANGUAGE_CLASH,
            )];
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the diagnostics {expected:?}, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: how `InvalidDefinition` states a diagnostic
// ---------------------------------------------------------------------------

/// The definitions rule 3 spans: a definition refused for an unbindable JSONata field,
/// for a query-language clash, for a state type the language does not define, for a
/// structural rule about a state as a whole (so the location is the state itself), and
/// one carrying SEVERAL error diagnostics.
fn refused_definitions() -> Vec<(&'static str, String)> {
    vec![
        (
            "an unbindable JSONata field",
            jsonata_machine(json!({
                "Type": "Pass",
                "Output": { "x": "{% $states.result %}" },
                "Next": "Done"
            })),
        ),
        (
            "a query-language clash",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": "P",
                "States": { "P": { "Type": "Pass", "QueryLanguage": "JSONPath", "End": true } }
            })
            .to_string(),
        ),
        (
            "a state type the language does not define",
            json!({
                "StartAt": "P",
                "States": { "P": { "Type": "Transform", "End": true } }
            })
            .to_string(),
        ),
        (
            "a continuing state that says nowhere to go",
            json!({
                "StartAt": "P",
                "States": { "P": { "Type": "Pass" } }
            })
            .to_string(),
        ),
        (
            "two fields the scope does not bind",
            jsonata_machine(json!({
                "Type": "Pass",
                "Output": {
                    "a": "{% $states.errorOutput.Error %}",
                    "b": "{% $states.result.count %}"
                },
                "Next": "Done"
            })),
        ),
    ]
}

/// A create and an update refuse a definition with `InvalidDefinition` whose message
/// quotes the diagnostics the SAME walk reports for it, one subject per error stated as
/// `<code>: <message> at <location>` — the place joined by the bare word, never wrapped
/// in anything — and nothing else.
#[tokio::test]
async fn invalid_definition_states_the_diagnostic_and_joins_its_location_with_at() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "definition-refusal", HELLO_DEFINITION).await;

    for (index, (case, definition)) in refused_definitions().into_iter().enumerate() {
        // The subjects are built from what the validation computed for this very
        // definition, so what the refusal states and what the validation reports
        // cannot disagree.
        let reported = diagnostics(&emulator, case, &definition).await;
        let (status, body) = emulator
            .call(
                "ValidateStateMachineDefinition",
                &json!({ "definition": definition }),
            )
            .await;
        assert_eq!(
            (status, body.get("result").and_then(Value::as_str)),
            (200, Some(VALIDATION_RESULT_FAIL)),
            "case {case:?}: expected the validation to fail the definition, got {body:?}"
        );
        let subjects: Vec<String> = reported
            .iter()
            .filter(|diagnostic| diagnostic[0] == SEVERITY_ERROR)
            .map(|diagnostic| {
                let (code, location, message) = (&diagnostic[1], &diagnostic[2], &diagnostic[3]);
                // A diagnostic about the state machine AS A WHOLE carries no
                // location — there is no field in the document to place it at — so
                // the subject for one is the code and the detail alone.
                if location == "<absent>" {
                    return format!("{code}: {message}");
                }
                format!("{code}: {message} at {location}")
            })
            .collect();
        assert!(
            !subjects.is_empty(),
            "case {case:?}: expected at least one ERROR diagnostic, got {reported:?}"
        );
        let expected = subjects.join(", ");

        for (operation, request) in [
            (
                "CreateStateMachine",
                json!({
                    "name": format!("definition-refusal-{index}"),
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                }),
            ),
            (
                "UpdateStateMachine",
                json!({ "stateMachineArn": machine, "definition": definition }),
            ),
        ] {
            let case = format!("{case} on {operation}");
            let (status, code, message) = failure(&emulator, &case, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidDefinition"),
                "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with message \
                 {message:?}"
            );
            let quoted = message
                .strip_prefix(&format!("{INVALID_STATE_MACHINE_DEFINITION}: '"))
                .and_then(|rest| rest.strip_suffix('\''))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a message of the form \
                         \"{INVALID_STATE_MACHINE_DEFINITION}: '<subject>'\", got {message:?}"
                    )
                });
            assert_eq!(
                quoted, expected,
                "case {case:?}: expected the walk's subjects {expected:?}, got {quoted:?}"
            );
            assert!(
                !message.contains("(at "),
                "case {case:?}: the location is joined by the bare word, got {message:?}"
            );
        }
    }
}

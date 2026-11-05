// SPDX-License-Identifier: CC0-1.0

//! What the ASL static validator reports about the FIELDS a definition carries and
//! about the transition graph its states form, driven end to end over this service's
//! own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production router.
//!
//! The rules under test, each over a table whose rows differ along what the rule
//! generalises over rather than along the one definition that exposed it:
//!
//! 1. the fields the Amazon States Language declares on a state are a CLOSED set per
//!    state type, and a state carrying a field outside its type's set earns one
//!    `SCHEMA_VALIDATION_FAILED` ERROR per field, located at the STATE and stating the
//!    field — the same rule, and the same sentence, for a `Retry` retrier's and a
//!    `Catch` catcher's own closed sets, which also say which fields each must carry;
//! 2. `States.ALL` is admitted only as the single entry of an `ErrorEquals` list and
//!    only in the last entry of the array holding it;
//! 3. a diagnostic about an element of an array addresses it with the index
//!    BRACKETED onto the array's field name, never as a path segment of its own;
//! 4. the two halves of the transition-graph rule are reported under one code: a
//!    transition target no state declares, named as the value of the field the
//!    language calls `Next` whichever member declared it, and a state nothing in the
//!    definition names;
//! 5. what `CreateStateMachine` and `UpdateStateMachine` refuse a definition for is
//!    what the validation reports about it, stated one subject per diagnostic.
//!
//! The harness (`Emulator`, `error_code`, `ROLE_ARN`, `create_machine`) is the one
//! `state_machine_lifecycle` builds and the validation readers are
//! `definition_validation`'s; `tests/all.rs` is this binary's crate root, so each is
//! reached through `crate::<suite>`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    state_fields, MISSING_TRANSITION_TARGET, SCHEMA_VALIDATION_FAILED, STATE_TYPES,
};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    INVALID_STATE_MACHINE_DEFINITION, SEVERITY_ERROR, SEVERITY_WARNING, VALIDATION_RESULT_FAIL,
};

use crate::definition_validation::{diagnostics, result_of, validate};
use crate::state_machine_lifecycle::{
    create_machine, error_code, Emulator, HELLO_DEFINITION, ROLE_ARN,
};

/// The name every state under test is declared under.
const STATE: &str = "Work";

/// The `Resource` a `Task` state under test names.
const RESOURCE: &str = "arn:aws:states:::aws-sdk:sts:getCallerIdentity";

/// One ERROR diagnostic as the wire reports it.
fn error_at(code: &str, location: &str, message: &str) -> Value {
    json!({
        "severity": SEVERITY_ERROR,
        "code": code,
        "location": location,
        "message": message,
    })
}

/// A state machine whose one state under test is `state`, with the `Succeed` a
/// continuing state transitions to declared beside it.
fn machine(state: Value) -> String {
    json!({
        "StartAt": STATE,
        "States": { STATE: state, "Done": { "Type": "Succeed" } },
    })
    .to_string()
}

/// A minimal valid state of `state_type` that transitions to `Done`, as the rows
/// below start from.
///
/// The members are the ones the type needs to satisfy every OTHER rule of the walk, so
/// a row that fails does so on the field it ADDS.
fn minimal(state_type: &str) -> Value {
    match state_type {
        "Choice" => json!({
            "Type": "Choice",
            "Choices": [{ "Variable": "$.x", "NumericEquals": 1, "Next": "Done" }],
            "Default": "Done",
        }),
        "Fail" => json!({ "Type": "Fail", "Error": "Boom", "Cause": "on purpose" }),
        "Succeed" => json!({ "Type": "Succeed" }),
        "Map" => json!({
            "Type": "Map",
            "ItemProcessor": { "StartAt": "P", "States": { "P": { "Type": "Pass", "End": true } } },
            "Next": "Done",
        }),
        "Parallel" => json!({
            "Type": "Parallel",
            "Branches": [{ "StartAt": "B", "States": { "B": { "Type": "Pass", "End": true } } }],
            "Next": "Done",
        }),
        "Task" => json!({ "Type": "Task", "Resource": RESOURCE, "Next": "Done" }),
        "Wait" => json!({ "Type": "Wait", "Seconds": 1, "Next": "Done" }),
        _ => json!({ "Type": state_type, "Next": "Done" }),
    }
}

/// What `CreateStateMachine` answers for `definition`, as
/// `(status, error code, message)`.
async fn created(emulator: &Emulator, name: &str, definition: &str) -> (u16, String, String) {
    let (status, body) = emulator
        .call(
            "CreateStateMachine",
            &json!({ "name": name, "definition": definition, "roleArn": ROLE_ARN }),
        )
        .await;
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, error_code(&body), message)
}

// ---------------------------------------------------------------------------
// Rule 1: a state's fields are the closed set its TYPE declares
// ---------------------------------------------------------------------------

/// A state carrying a field its type does not declare is an ERROR stating that field,
/// located at the state; a field the type DOES declare is never reported that way.
///
/// The rows are the CATALOG itself: for every one of the language's state types
/// (`service_com_amazonaws_sfn_awsstepfunctions::state::asl::STATE_TYPES`) every field
/// any other type declares is driven against it
/// (`…::state::asl::state_fields`, the per-type sets), so the rule is pinned over
/// every (type, field) pair rather than over the pair the issue cited.
#[tokio::test]
async fn a_field_the_state_type_does_not_declare_is_reported_at_the_state() {
    let emulator = Emulator::new();

    // Every field the language declares on any state, which is what each type's own
    // set is judged against.
    let every_field: BTreeSet<&str> = STATE_TYPES
        .iter()
        .flat_map(|state_type| state_fields(state_type).iter().copied())
        .collect();
    assert!(
        every_field.contains("Retry") && every_field.contains("Catch"),
        "case the catalog: expected the per-type sets to declare the error-handling \
         fields, got {every_field:?}"
    );

    for state_type in STATE_TYPES {
        let declared: BTreeSet<&str> = state_fields(state_type).iter().copied().collect();
        assert!(
            !declared.is_empty(),
            "case {state_type:?}: every type the language defines declares fields, got none"
        );

        for field in &every_field {
            let case = format!("the field {field:?} on a {state_type} state");
            let mut state = minimal(state_type);
            let members = state.as_object_mut().expect("a state is an object");
            // The value is immaterial: what is reported is the field's NAME, so one
            // value serves every row and no row can pass by carrying a value the
            // field would be refused for anyway.
            members.insert((*field).to_string(), json!("x"));
            let definition = machine(state);

            let body = validate(
                &emulator,
                &case,
                &json!({ "definition": &definition, "severity": SEVERITY_WARNING }),
            )
            .await;
            let reported = diagnostics(&body);
            let expected = error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}"),
                &format!("Field '{field}' is not supported"),
            );
            if declared.contains(field) {
                assert!(
                    !reported.contains(&expected),
                    "case {case:?}: a field the type declares is not reported as \
                     unsupported, got {reported:?}"
                );
                continue;
            }
            assert!(
                reported.contains(&expected),
                "case {case:?}: expected the diagnostic {expected:?}, got {reported:?}"
            );
            assert_eq!(
                result_of(&body),
                VALIDATION_RESULT_FAIL,
                "case {case:?}: a field the type does not declare fails the definition, got \
                 {body:?}"
            );
        }
    }
}

/// The same rule over a `Retry` retrier's and a `Catch` catcher's own closed sets,
/// which also say which fields each entry must carry — reported at the ENTRY, with the
/// index bracketed onto the array's field name.
///
/// Rows differ along WHICH array the entry sits in, which field is missing or
/// undeclared, where in the array the entry sits, and which state type declares the
/// array.
#[tokio::test]
async fn a_retrier_and_a_catcher_are_judged_against_the_fields_they_declare() {
    let emulator = Emulator::new();

    // (case, the members to add to a Task state, the diagnostics expected)
    let rows: Vec<(&str, Value, Vec<Value>)> = vec![
        (
            "a retrier that names no errors",
            json!({ "Retry": [{ "MaxAttempts": 2 }] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Retry[0]"),
                "These fields are required: [ErrorEquals]",
            )],
        ),
        (
            "a catcher that names neither the errors it takes nor where it goes",
            json!({ "Catch": [{ "ResultPath": "$.error" }] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Catch[0]"),
                "These fields are required: [ErrorEquals, Next]",
            )],
        ),
        (
            "a catcher that names no state to continue at",
            json!({ "Catch": [{ "ErrorEquals": ["States.ALL"] }] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Catch[0]"),
                "These fields are required: [Next]",
            )],
        ),
        (
            "a retrier carrying a field only a catcher declares",
            json!({ "Retry": [{ "ErrorEquals": ["States.Timeout"], "Next": "Done" }] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Retry[0]"),
                "Field 'Next' is not supported",
            )],
        ),
        (
            "a catcher carrying a field only a retrier declares",
            json!({
                "Catch": [
                    { "ErrorEquals": ["States.Timeout"], "Next": "Done" },
                    { "ErrorEquals": ["States.ALL"], "Next": "Done", "MaxAttempts": 2 },
                ],
            }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Catch[1]"),
                "Field 'MaxAttempts' is not supported",
            )],
        ),
        (
            "a retrier carrying a field the language declares nowhere",
            json!({ "Retry": [{ "ErrorEquals": ["States.ALL"], "Backoff": 2 }] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Retry[0]"),
                "Field 'Backoff' is not supported",
            )],
        ),
        (
            "a Retry that is not a list of retriers at all",
            json!({ "Retry": { "ErrorEquals": ["States.ALL"] } }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Retry"),
                "'Retry' must be an array",
            )],
        ),
        (
            "a catcher that is not an object",
            json!({ "Catch": ["States.ALL"] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Catch[0]"),
                "every entry of 'Catch' must be a JSON object",
            )],
        ),
        (
            "a retrier whose ErrorEquals is one name rather than a list of them",
            json!({ "Retry": [{ "ErrorEquals": "States.ALL" }] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Retry[0]/ErrorEquals"),
                "'ErrorEquals' must be an array",
            )],
        ),
        (
            "a retrier that names no error at all",
            json!({ "Retry": [{ "ErrorEquals": [] }] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Retry[0]/ErrorEquals"),
                "'ErrorEquals' must name at least one error",
            )],
        ),
        (
            "a catcher naming an error that is not a name",
            json!({ "Catch": [{ "ErrorEquals": ["States.Timeout", 7], "Next": "Done" }] }),
            vec![error_at(
                SCHEMA_VALIDATION_FAILED,
                &format!("/States/{STATE}/Catch[0]/ErrorEquals[1]"),
                "'ErrorEquals' must name errors as strings",
            )],
        ),
        (
            "the retriers and catchers the language declares, carried in full",
            json!({
                "Retry": [{
                    "ErrorEquals": ["States.Timeout", "Lambda.TooManyRequestsException"],
                    "IntervalSeconds": 2,
                    "MaxAttempts": 4,
                    "BackoffRate": 1.5,
                    "MaxDelaySeconds": 10,
                    "JitterStrategy": "FULL",
                    "Comment": "every declared field",
                }],
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Next": "Done",
                    "ResultPath": "$.error",
                    "Comment": "every declared field",
                }],
            }),
            vec![],
        ),
    ];

    for (index, (case, extra, expected)) in rows.into_iter().enumerate() {
        let mut state = minimal("Task");
        let members = state.as_object_mut().expect("a state is an object");
        for (member, value) in extra.as_object().expect("the members are an object") {
            members.insert(member.clone(), value.clone());
        }
        let definition = machine(state);

        let body = validate(
            &emulator,
            case,
            &json!({ "definition": &definition, "severity": SEVERITY_WARNING }),
        )
        .await;
        let reported = diagnostics(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the diagnostics {expected:?}, got {reported:?}"
        );

        // What a create does with the same definition follows from the same walk.
        let (status, code, message) =
            created(&emulator, &format!("handler-{index}"), &definition).await;
        if expected.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a definition the validation passes is created, got {status} \
                 {code} {message:?}"
            );
            continue;
        }
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected the create to refuse the definition, got {status} {code}"
        );
        for diagnostic in &expected {
            let subject = format!(
                "{SCHEMA_VALIDATION_FAILED}: {} at {}",
                diagnostic["message"].as_str().unwrap_or_default(),
                diagnostic["location"].as_str().unwrap_or_default(),
            );
            assert!(
                message.contains(&subject),
                "case {case:?}: expected the refusal to state {subject:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: where `States.ALL` may be named
// ---------------------------------------------------------------------------

/// The sentence the service answers an `ErrorEquals` naming the catch-all beside
/// another name, or in an entry another entry follows.
const ALL_ALONE: &str = "States.ALL must appear alone and at end of list";

/// `States.ALL` is admitted only as the single entry of an `ErrorEquals` list, and
/// only in the last entry of the array that holds that list.
///
/// Rows differ along WHICH array holds the entry (`Retry`, `Catch`), how many entries
/// it holds, where in the array the entry naming the catch-all sits, and where in its
/// `ErrorEquals` the catch-all itself sits.
#[tokio::test]
async fn states_all_is_admitted_only_alone_and_in_the_last_entry() {
    let emulator = Emulator::new();

    // (case, the member, its entries, the locations the catch-all is reported at)
    let rows: Vec<(&str, &str, Value, Vec<String>)> = vec![
        (
            "the catch-all alone, in the only retrier",
            "Retry",
            json!([{ "ErrorEquals": ["States.ALL"] }]),
            vec![],
        ),
        (
            "the catch-all first in a retrier's list",
            "Retry",
            json!([{ "ErrorEquals": ["States.ALL", "States.Timeout"] }]),
            vec![format!("/States/{STATE}/Retry[0]/ErrorEquals[0]")],
        ),
        (
            "the catch-all last in a retrier's list, but not alone",
            "Retry",
            json!([{ "ErrorEquals": ["States.Timeout", "States.ALL"] }]),
            vec![format!("/States/{STATE}/Retry[0]/ErrorEquals[1]")],
        ),
        (
            "a retrier naming the catch-all with another retrier after it",
            "Retry",
            json!([
                { "ErrorEquals": ["States.ALL"] },
                { "ErrorEquals": ["States.Timeout"] },
            ]),
            vec![format!("/States/{STATE}/Retry[0]/ErrorEquals[0]")],
        ),
        (
            "a retrier naming the catch-all last of three",
            "Retry",
            json!([
                { "ErrorEquals": ["States.Timeout"] },
                { "ErrorEquals": ["Lambda.ServiceException"] },
                { "ErrorEquals": ["States.ALL"] },
            ]),
            vec![],
        ),
        (
            "a catcher naming the catch-all with another catcher after it",
            "Catch",
            json!([
                { "ErrorEquals": ["States.ALL"], "Next": "Done" },
                { "ErrorEquals": ["States.Timeout"], "Next": "Done" },
            ]),
            vec![format!("/States/{STATE}/Catch[0]/ErrorEquals[0]")],
        ),
        (
            "a catcher naming the catch-all beside a custom name, last of two",
            "Catch",
            json!([
                { "ErrorEquals": ["States.Timeout"], "Next": "Done" },
                { "ErrorEquals": ["MyError", "States.ALL"], "Next": "Done" },
            ]),
            vec![format!("/States/{STATE}/Catch[1]/ErrorEquals[1]")],
        ),
        (
            "two entries each naming the catch-all",
            "Catch",
            json!([
                { "ErrorEquals": ["States.ALL"], "Next": "Done" },
                { "ErrorEquals": ["States.ALL", "MyError"], "Next": "Done" },
            ]),
            vec![
                format!("/States/{STATE}/Catch[0]/ErrorEquals[0]"),
                format!("/States/{STATE}/Catch[1]/ErrorEquals[0]"),
            ],
        ),
        (
            "a catcher naming the catch-all alone and last",
            "Catch",
            json!([
                { "ErrorEquals": ["States.Timeout"], "Next": "Done" },
                { "ErrorEquals": ["States.ALL"], "Next": "Done" },
            ]),
            vec![],
        ),
    ];

    for (index, (case, member, entries, locations)) in rows.into_iter().enumerate() {
        let mut state = minimal("Task");
        state
            .as_object_mut()
            .expect("a state is an object")
            .insert(member.to_string(), entries);
        let definition = machine(state);

        let body = validate(
            &emulator,
            case,
            &json!({ "definition": &definition, "severity": SEVERITY_WARNING }),
        )
        .await;
        let expected: Vec<Value> = locations
            .iter()
            .map(|location| error_at(SCHEMA_VALIDATION_FAILED, location, ALL_ALONE))
            .collect();
        let reported = diagnostics(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the diagnostics {expected:?}, got {reported:?}"
        );

        let (status, code, message) =
            created(&emulator, &format!("catch-all-{index}"), &definition).await;
        if locations.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a definition the validation passes is created, got {status} \
                 {code} {message:?}"
            );
            continue;
        }
        // The refusal states one subject per error, in the words the validation
        // reported it in.
        let subjects: Vec<String> = locations
            .iter()
            .map(|location| format!("{SCHEMA_VALIDATION_FAILED}: {ALL_ALONE} at {location}"))
            .collect();
        let expected_message = format!(
            "{INVALID_STATE_MACHINE_DEFINITION}: '{}'",
            subjects.join(", ")
        );
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "InvalidDefinition", expected_message.as_str()),
            "case {case:?}: expected 400 InvalidDefinition stating {expected_message:?}, got \
             {status} {code} {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: an array element carries a bracketed index
// ---------------------------------------------------------------------------

/// Every diagnostic about an element of an array addresses that element as
/// `<Field>[<index>]`, so no location carries an index as a path segment of its own.
///
/// Rows differ along WHICH array the element belongs to — a `Catch`, a `Choices`, a
/// `Parallel`'s `Branches`, an `ErrorEquals`, and a list inside a payload a state
/// builds — and along the index itself.
#[tokio::test]
async fn an_array_element_is_located_with_a_bracketed_index() {
    let emulator = Emulator::new();

    // (case, the definition, the location of the diagnostic about the element)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the Next of the second catcher",
            machine(json!({
                "Type": "Task",
                "Resource": RESOURCE,
                "Catch": [
                    { "ErrorEquals": ["States.Timeout"], "Next": "Done" },
                    { "ErrorEquals": ["States.ALL"], "Next": "Nowhere" },
                ],
                "Next": "Done",
            })),
            format!("/States/{STATE}/Catch[1]/Next"),
        ),
        (
            "the Next of the first Choice rule",
            machine(json!({
                "Type": "Choice",
                "Choices": [
                    { "Variable": "$.x", "NumericEquals": 1, "Next": "Nowhere" },
                    { "Variable": "$.x", "NumericEquals": 2, "Next": "Done" },
                ],
                "Default": "Done",
            })),
            format!("/States/{STATE}/Choices[0]/Next"),
        ),
        (
            "the second branch of a Parallel",
            machine(json!({
                "Type": "Parallel",
                "Branches": [
                    { "StartAt": "B", "States": { "B": { "Type": "Pass", "End": true } } },
                    { "States": { "B": { "Type": "Pass", "End": true } } },
                ],
                "Next": "Done",
            })),
            format!("/States/{STATE}/Branches[1]"),
        ),
        (
            "the catch-all in a retrier's ErrorEquals",
            machine(json!({
                "Type": "Task",
                "Resource": RESOURCE,
                "Retry": [{ "ErrorEquals": ["States.ALL", "States.Timeout"] }],
                "Next": "Done",
            })),
            format!("/States/{STATE}/Retry[0]/ErrorEquals[0]"),
        ),
        (
            "an element of a list a JSONata state puts out",
            json!({
                "QueryLanguage": "JSONata",
                "StartAt": STATE,
                "States": {
                    STATE: {
                        "Type": "Pass",
                        "Output": { "all": ["kept", "{% $states.result %}"] },
                        "Next": "Done",
                    },
                    "Done": { "Type": "Succeed" },
                },
            })
            .to_string(),
            format!("/States/{STATE}/Output/all[1]"),
        ),
    ];

    for (case, definition, location) in rows {
        let body = validate(
            &emulator,
            case,
            &json!({ "definition": definition, "severity": SEVERITY_WARNING }),
        )
        .await;
        let reported: Vec<String> = diagnostics(&body)
            .iter()
            .map(|diagnostic| {
                diagnostic
                    .get("location")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
                    .to_string()
            })
            .collect();
        assert!(
            reported.contains(&location),
            "case {case:?}: expected a diagnostic located at {location:?}, got {reported:?}"
        );
        // No location addresses an element the other way round: an index is never a
        // segment of its own.
        for reported in &reported {
            let numbered = reported
                .split('/')
                .any(|segment| segment.chars().all(|character| character.is_ascii_digit()) && !segment.is_empty());
            assert!(
                !numbered,
                "case {case:?}: expected every location to bracket its indexes, got \
                 {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the transition graph, read both ways
// ---------------------------------------------------------------------------

/// An unresolved transition target names the target as the value of `Next` whichever
/// member declared it, and a state nothing names is reported at that state — both
/// under the one code the transition graph's problems are reported under.
///
/// Rows differ along WHICH member declares the transition (a state's own `Next`, a
/// `Choice` rule's, the `Default` beside them, a `Catch` catcher's, the machine's own
/// `StartAt`), where the state nothing names sits (the definition, a `Map`'s
/// processor, a `Parallel` branch), and which members DO name a state, which earn no
/// diagnostic at all.
#[tokio::test]
async fn the_transition_graph_is_reported_from_both_ends() {
    let emulator = Emulator::new();

    // (case, the definition, the diagnostics expected)
    let rows: Vec<(&str, String, Vec<Value>)> = vec![
        (
            "a state's own Next naming nothing",
            json!({
                "StartAt": STATE,
                "States": {
                    STATE: { "Type": "Pass", "Next": "Nowhere" },
                    "Done": { "Type": "Succeed" },
                },
            })
            .to_string(),
            vec![
                error_at(
                    MISSING_TRANSITION_TARGET,
                    &format!("/States/{STATE}/Next"),
                    "Missing 'Next' target: Nowhere",
                ),
                error_at(
                    MISSING_TRANSITION_TARGET,
                    "/States/Done",
                    "State \"Done\" is not reachable.",
                ),
            ],
        ),
        (
            "a Choice's Default naming nothing, which the language still calls Next",
            json!({
                "StartAt": STATE,
                "States": {
                    STATE: {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.x", "NumericEquals": 1, "Next": "Done" }],
                        "Default": "Elsewhere",
                    },
                    "Done": { "Type": "Succeed" },
                },
            })
            .to_string(),
            vec![error_at(
                MISSING_TRANSITION_TARGET,
                &format!("/States/{STATE}/Default"),
                "Missing 'Next' target: Elsewhere",
            )],
        ),
        (
            "the machine's own StartAt naming nothing, which the language still calls Next",
            json!({
                "StartAt": "Bootstrap",
                "States": { STATE: { "Type": "Succeed" } },
            })
            .to_string(),
            vec![
                error_at(
                    MISSING_TRANSITION_TARGET,
                    "/StartAt",
                    "Missing 'Next' target: Bootstrap",
                ),
                error_at(
                    MISSING_TRANSITION_TARGET,
                    &format!("/States/{STATE}"),
                    &format!("State \"{STATE}\" is not reachable."),
                ),
            ],
        ),
        (
            "a state only a Catch catcher names, which is a state something names",
            json!({
                "StartAt": STATE,
                "States": {
                    STATE: {
                        "Type": "Task",
                        "Resource": RESOURCE,
                        "Catch": [{ "ErrorEquals": ["States.ALL"], "Next": "Recover" }],
                        "End": true,
                    },
                    "Recover": { "Type": "Succeed" },
                },
            })
            .to_string(),
            vec![],
        ),
        (
            "a state only a Choice rule names",
            json!({
                "StartAt": STATE,
                "States": {
                    STATE: {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.x", "IsPresent": true, "Next": "Done" }],
                        "Default": "Stop",
                    },
                    "Done": { "Type": "Succeed" },
                    "Stop": { "Type": "Fail", "Error": "Stopped" },
                },
            })
            .to_string(),
            vec![],
        ),
        (
            "a state nothing names inside a Map's processor",
            json!({
                "StartAt": STATE,
                "States": {
                    STATE: {
                        "Type": "Map",
                        "ItemProcessor": {
                            "StartAt": "P",
                            "States": {
                                "P": { "Type": "Pass", "End": true },
                                "Orphan": { "Type": "Succeed" },
                            },
                        },
                        "End": true,
                    },
                },
            })
            .to_string(),
            vec![error_at(
                MISSING_TRANSITION_TARGET,
                &format!("/States/{STATE}/ItemProcessor/States/Orphan"),
                "State \"Orphan\" is not reachable.",
            )],
        ),
        (
            "a state nothing names inside a Parallel branch",
            json!({
                "StartAt": STATE,
                "States": {
                    STATE: {
                        "Type": "Parallel",
                        "Branches": [{
                            "StartAt": "B",
                            "States": {
                                "B": { "Type": "Pass", "End": true },
                                "Orphan": { "Type": "Succeed" },
                            },
                        }],
                        "End": true,
                    },
                },
            })
            .to_string(),
            vec![error_at(
                MISSING_TRANSITION_TARGET,
                &format!("/States/{STATE}/Branches[0]/States/Orphan"),
                "State \"Orphan\" is not reachable.",
            )],
        ),
    ];

    for (index, (case, definition, expected)) in rows.into_iter().enumerate() {
        let body = validate(
            &emulator,
            case,
            &json!({ "definition": &definition, "severity": SEVERITY_WARNING }),
        )
        .await;
        let reported = diagnostics(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the diagnostics {expected:?}, got {reported:?}"
        );

        let (status, code, message) =
            created(&emulator, &format!("graph-{index}"), &definition).await;
        if expected.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a definition the validation passes is created, got {status} \
                 {code} {message:?}"
            );
            continue;
        }
        // Every error the walk found is stated, one subject per diagnostic, in the
        // order it was found: a caller fixing the definition is told all of them.
        let subjects: Vec<String> = expected
            .iter()
            .map(|diagnostic| {
                format!(
                    "{MISSING_TRANSITION_TARGET}: {} at {}",
                    diagnostic["message"].as_str().unwrap_or_default(),
                    diagnostic["location"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        let expected_message = format!(
            "{INVALID_STATE_MACHINE_DEFINITION}: '{}'",
            subjects.join(", ")
        );
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "InvalidDefinition", expected_message.as_str()),
            "case {case:?}: expected 400 InvalidDefinition stating {expected_message:?}, got \
             {status} {code} {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: an update reads the same walk as a create
// ---------------------------------------------------------------------------

/// `UpdateStateMachine` refuses a definition for exactly what the validation reports
/// about it, in the same words — so the three operations that read the walk cannot
/// disagree.
///
/// Rows differ along which rule the definition breaks, so what is pinned is that the
/// update states the walk's subjects rather than one definition's refusal.
#[tokio::test]
async fn an_update_states_the_same_subjects_as_the_validation() {
    let emulator = Emulator::new();
    let machine_arn = create_machine(&emulator, "updated", HELLO_DEFINITION).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "a catcher whose Next names nothing",
            machine(json!({
                "Type": "Task",
                "Resource": RESOURCE,
                "Catch": [{ "ErrorEquals": ["States.ALL"], "Next": "HandleError" }],
                "Next": "Done",
            })),
        ),
        (
            "a retrier naming the catch-all beside another name",
            machine(json!({
                "Type": "Task",
                "Resource": RESOURCE,
                "Retry": [{ "ErrorEquals": ["States.ALL", "States.Timeout"] }],
                "Next": "Done",
            })),
        ),
        (
            "a Wait state carrying a field only a Task declares",
            machine(json!({ "Type": "Wait", "Seconds": 1, "Retry": [], "Next": "Done" })),
        ),
    ];

    for (case, definition) in rows {
        let body = validate(
            &emulator,
            case,
            &json!({ "definition": &definition, "severity": SEVERITY_ERROR }),
        )
        .await;
        let subjects: Vec<String> = diagnostics(&body)
            .iter()
            .map(|diagnostic| {
                format!(
                    "{}: {} at {}",
                    diagnostic["code"].as_str().unwrap_or_default(),
                    diagnostic["message"].as_str().unwrap_or_default(),
                    diagnostic["location"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert!(
            !subjects.is_empty(),
            "case {case:?}: expected the validation to report an error, got {body:?}"
        );

        let (status, updated) = emulator
            .call(
                "UpdateStateMachine",
                &json!({ "stateMachineArn": &machine_arn, "definition": &definition }),
            )
            .await;
        let expected_message = format!(
            "{INVALID_STATE_MACHINE_DEFINITION}: '{}'",
            subjects.join(", ")
        );
        let message = updated
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            (status, error_code(&updated).as_str(), message),
            (400, "InvalidDefinition", expected_message.as_str()),
            "case {case:?}: expected the update to state {expected_message:?}, got {status} \
             with {updated:?}"
        );
    }
}

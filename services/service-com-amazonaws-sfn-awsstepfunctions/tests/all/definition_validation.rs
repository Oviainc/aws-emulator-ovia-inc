// SPDX-License-Identifier: CC0-1.0

//! `ValidateStateMachineDefinition`: what the service reports about a definition it
//! was handed, "without creating a state machine resource".
//!
//! The rules under test:
//!
//! 1. `result` is `FAIL` exactly when the definition carries a problem that would
//!    prevent creating or updating a state machine with it, and `OK` otherwise — so
//!    what this operation reports and what `CreateStateMachine` does with the SAME
//!    definition cannot disagree. Warnings do not make a definition FAIL;
//! 2. a diagnostic says WHERE it was found, in the `/States/<StateName>/<FieldName>`
//!    form the `location` member documents;
//! 3. `severity` selects which diagnostics are REPORTED and never what the result
//!    is: `ERROR` (the default) reports only errors, `WARNING` reports both;
//! 4. `maxResults` bounds how many diagnostics are reported — 0 means its documented
//!    default of 100 — and `truncated` says whether the list was cut short;
//! 5. validating persists nothing, so no state machine exists afterwards.
//!
//! The harness (`Emulator`, `error_code`) is the one `state_machine_lifecycle`
//! builds; `tests/all.rs` is this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    DEFAULT_MAX_DIAGNOSTICS, MAX_LOCATION_CHARS, MAX_STATE_NAME_CHARS, MISSING_TRANSITION_TARGET,
    NO_DOLLAR, PASS_RESULT_IS_STATIC, SCHEMA_VALIDATION_FAILED,
};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    SEVERITY_ERROR, SEVERITY_WARNING, VALIDATION_RESULT_FAIL, VALIDATION_RESULT_OK,
};

use crate::state_machine_lifecycle::{error_code, Emulator, HELLO_DEFINITION, ROLE_ARN};

/// One definition the table below spans, and whether a state machine can be created
/// with it.
struct Case {
    /// What makes this row different from the others.
    case: String,
    definition: String,
    /// Whether the definition passes verification: `result` is `OK` and a create
    /// with it succeeds. A definition that only earns WARNINGS is still creatable.
    creatable: bool,
}

/// The definitions both rule-1 tests read: every structural rule this service
/// validates, broken and obeyed, in the JSONPath query language and in JSONata.
///
/// Rows differ along WHICH rule is at stake, which query language the state is
/// written in, which level declares that language (the definition's, one state's),
/// and how deeply the offending state is nested.
fn cases() -> Vec<Case> {
    let long_name = "n".repeat(MAX_STATE_NAME_CHARS + 1);
    let rows: Vec<(&str, String, bool)> = vec![
        // ---------------- the document itself ----------------
        ("a document that is not JSON", "{ this is not json".to_string(), false),
        ("a definition that is blank", "   ".to_string(), false),
        ("a JSON array", r#"[{"Type":"Pass"}]"#.to_string(), false),
        (
            "no StartAt",
            r#"{"States":{"A":{"Type":"Succeed"}}}"#.to_string(),
            false,
        ),
        (
            "a StartAt no state declares",
            r#"{"StartAt":"Missing","States":{"A":{"Type":"Succeed"}}}"#.to_string(),
            false,
        ),
        (
            "an empty States map",
            r#"{"StartAt":"A","States":{}}"#.to_string(),
            false,
        ),
        // ---------------- one state ----------------
        (
            "a state that is not an object",
            r#"{"StartAt":"A","States":{"A":"Succeed"}}"#.to_string(),
            false,
        ),
        (
            "a state with no Type",
            r#"{"StartAt":"A","States":{"A":{"End":true}}}"#.to_string(),
            false,
        ),
        (
            "a Next no state declares",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"Nowhere"}}}"#.to_string(),
            false,
        ),
        (
            "a Succeed state carrying Next",
            r#"{"StartAt":"A","States":{"A":{"Type":"Succeed","Next":"A"}}}"#.to_string(),
            false,
        ),
        (
            "a Pass state with neither Next nor End",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass"}}}"#.to_string(),
            false,
        ),
        (
            "a definition no state of which ends an execution",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"B"},"B":{"Type":"Pass","Next":"A"}}}"#.to_string(),
            false,
        ),
        (
            "a Parallel branch no state of which ends an execution",
            r#"{"StartAt":"P","States":{"P":{"Type":"Parallel","Branches":[{"StartAt":"B","States":{"B":{"Type":"Pass","Next":"B"}}}],"End":true}}}"#.to_string(),
            false,
        ),
        (
            "a state name one character past the bound",
            format!(r#"{{"StartAt":"{long_name}","States":{{"{long_name}":{{"Type":"Succeed"}}}}}}"#),
            false,
        ),
        // ---------------- a nested state machine ----------------
        (
            "an ItemProcessor state transitioning nowhere",
            r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"I","States":{"I":{"Type":"Pass","Next":"Gone"}}},"End":true}}}"#.to_string(),
            false,
        ),
        (
            "a Parallel branch with no StartAt",
            r#"{"StartAt":"P","States":{"P":{"Type":"Parallel","Branches":[{"States":{"B":{"Type":"Succeed"}}}],"End":true}}}"#.to_string(),
            false,
        ),
        // ---------------- the query language ----------------
        (
            "a QueryLanguage the language does not define",
            r#"{"QueryLanguage":"JMESPath","StartAt":"A","States":{"A":{"Type":"Succeed"}}}"#.to_string(),
            false,
        ),
        (
            "a JSONata state carrying a Pass Result",
            r#"{"QueryLanguage":"JSONata","StartAt":"A","States":{"A":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#.to_string(),
            false,
        ),
        (
            "a JSONata state carrying an InputPath",
            r#"{"QueryLanguage":"JSONata","StartAt":"A","States":{"A":{"Type":"Pass","InputPath":"$.in","End":true}}}"#.to_string(),
            false,
        ),
        (
            "a JSONata state carrying a Parameters payload template",
            r#"{"QueryLanguage":"JSONata","StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"x.$":"$.y"},"End":true}}}"#.to_string(),
            false,
        ),
        (
            "a JSONPath state under a JSONata state machine",
            r#"{"QueryLanguage":"JSONata","StartAt":"A","States":{"A":{"Type":"Pass","QueryLanguage":"JSONPath","End":true}}}"#.to_string(),
            false,
        ),
        (
            "a JSONata state reading the error output outside a Catch",
            r#"{"QueryLanguage":"JSONata","StartAt":"A","States":{"A":{"Type":"Pass","Output":"{% $states.errorOutput %}","End":true}}}"#.to_string(),
            false,
        ),
        (
            "a JSONata Choice rule carrying a Variable",
            r#"{"QueryLanguage":"JSONata","StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Variable":"$.x","NumericEquals":1,"Next":"D"}],"Default":"D"},"D":{"Type":"Succeed"}}}"#.to_string(),
            false,
        ),
        (
            "a JSONata Choice rule carrying a Variable under an Or",
            r#"{"QueryLanguage":"JSONata","StartAt":"C","States":{"C":{"Type":"Choice","Choices":[{"Or":[{"Variable":"$.x","NumericEquals":1}],"Next":"D"}],"Default":"D"},"D":{"Type":"Succeed"}}}"#.to_string(),
            false,
        ),
        (
            "an ItemProcessor state carrying a ResultPath under a JSONata machine",
            r#"{"QueryLanguage":"JSONata","StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"I","States":{"I":{"Type":"Pass","ResultPath":"$.out","End":true}}},"End":true}}}"#.to_string(),
            false,
        ),
        (
            "one JSONata state of a JSONPath machine carrying a ResultPath",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","QueryLanguage":"JSONata","ResultPath":"$.out","End":true}}}"#.to_string(),
            false,
        ),
        // ---------------- definitions that pass verification ----------------
        ("a Pass state with a static Result", HELLO_DEFINITION.to_string(), true),
        (
            "a Task state whose Resource names no integration call",
            r#"{"StartAt":"A","States":{"A":{"Type":"Task","Resource":"arn:aws:lambda:us-west-2:000000000000:function:f","End":true}}}"#.to_string(),
            true,
        ),
        (
            "a JSONPath Choice state, whose rule names a Variable",
            r#"{"StartAt":"A","States":{"A":{"Type":"Choice","Choices":[{"Variable":"$.x","NumericEquals":1,"Next":"B"}],"Default":"B"},"B":{"Type":"Succeed"}}}"#.to_string(),
            true,
        ),
        (
            // A cycle is valid ASL as long as the machine declares somewhere to
            // stop: the loop is bounded at RUN time, not at create time.
            "a definition whose Next chain is a cycle out of which a state ends",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"B"},"B":{"Type":"Choice","Choices":[{"Variable":"$.go","IsPresent":true,"Next":"A"}],"Default":"Done"},"Done":{"Type":"Succeed"}}}"#.to_string(),
            true,
        ),
        (
            "a JSONata definition that assigns and reads variables",
            r#"{"QueryLanguage":"JSONata","StartAt":"A","States":{"A":{"Type":"Pass","Assign":{"m":"{% $x %}"},"Output":"{% $m %}","End":true}}}"#.to_string(),
            true,
        ),
        (
            "one JSONata state of a JSONPath state machine",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","QueryLanguage":"JSONata","Output":"{% $states.input %}","End":true}}}"#.to_string(),
            true,
        ),
        (
            "a JSONata state reading the error output in its own Catch",
            r#"{"QueryLanguage":"JSONata","StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"I","States":{"I":{"Type":"Pass","End":true}}},"Catch":[{"ErrorEquals":["States.ALL"],"Output":"{% $states.errorOutput %}","Next":"Done"}],"Next":"Done"},"Done":{"Type":"Succeed"}}}"#.to_string(),
            true,
        ),
        (
            "a JSONPath machine whose nested processor reads a ResultPath",
            r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","ItemProcessor":{"StartAt":"I","States":{"I":{"Type":"Pass","ResultPath":"$.out","End":true}}},"End":true}}}"#.to_string(),
            true,
        ),
        // Warnings only: the definition is created, and the result stays OK.
        (
            "a Pass state whose static Result reads as a path",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":"$.x","End":true}}}"#.to_string(),
            true,
        ),
        (
            "a Parameters member that holds a path without the .$ suffix",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"copy":"$.x"},"End":true}}}"#.to_string(),
            true,
        ),
    ];

    rows.into_iter()
        .map(|(case, definition, creatable)| Case {
            case: case.to_string(),
            definition,
            creatable,
        })
        .collect()
}

/// One `ValidateStateMachineDefinition` call, requiring a 200.
///
/// `pub(crate)` because the sibling suites read a validation the same way: one
/// reader, so two suites cannot disagree about what the response says.
pub(crate) async fn validate(emulator: &Emulator, case: &str, body: &Value) -> Value {
    let (status, parsed) = emulator.call("ValidateStateMachineDefinition", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ValidateStateMachineDefinition, got {status} with body \
         {parsed:?}"
    );
    parsed
}

/// The `result` a validation reported.
pub(crate) fn result_of(body: &Value) -> &str {
    body.get("result")
        .and_then(Value::as_str)
        .unwrap_or("<no result>")
}

/// The diagnostics a validation reported, in the order it reported them.
pub(crate) fn diagnostics(body: &Value) -> Vec<Value> {
    body.get("diagnostics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One member of every reported diagnostic.
pub(crate) fn members(body: &Value, member: &str) -> Vec<String> {
    diagnostics(body)
        .iter()
        .map(|diagnostic| {
            diagnostic
                .get(member)
                .and_then(Value::as_str)
                .unwrap_or("<absent>")
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: the result is the verdict on creating a state machine with it
// ---------------------------------------------------------------------------

/// Every definition is reported `FAIL` when it breaks one of the Amazon States
/// Language's rules and `OK` when it breaks none, with at least one ERROR
/// diagnostic for each failure and none at all for a definition that passes.
#[tokio::test]
async fn the_result_reports_whether_the_definition_passes_verification() {
    let emulator = Emulator::new();

    for Case {
        case,
        definition,
        creatable,
    } in cases()
    {
        let body = validate(
            &emulator,
            &case,
            &json!({ "definition": definition, "severity": SEVERITY_ERROR }),
        )
        .await;
        let expected = if creatable {
            VALIDATION_RESULT_OK
        } else {
            VALIDATION_RESULT_FAIL
        };
        assert_eq!(
            result_of(&body),
            expected,
            "case {case:?}: expected result {expected}, got {body:?}"
        );

        let found = diagnostics(&body);
        if creatable {
            assert!(
                found.is_empty(),
                "case {case:?}: a definition that passes verification carries no ERROR \
                 diagnostic, got {found:?}"
            );
            continue;
        }
        assert!(
            !found.is_empty(),
            "case {case:?}: expected at least one diagnostic, got {body:?}"
        );
        for diagnostic in &found {
            assert_eq!(
                diagnostic.get("severity").and_then(Value::as_str),
                Some(SEVERITY_ERROR),
                "case {case:?}: an ERROR-severity request reports only errors, got {diagnostic:?}"
            );
            for member in ["code", "message"] {
                let value = diagnostic.get(member).and_then(Value::as_str).unwrap_or("");
                assert!(
                    !value.is_empty(),
                    "case {case:?}: every diagnostic states its {member}, got {diagnostic:?}"
                );
            }
        }
    }
}

/// The SAME definitions through `CreateStateMachine`: a create succeeds for exactly
/// the definitions the validation reported `OK`, and is refused with
/// `InvalidDefinition` for exactly those it reported `FAIL`.
///
/// This is the rule the operation's own documentation states — "The result value
/// will be OK when your workflow definition can be successfully created or updated"
/// — so it is asserted against the create rather than against a second expectation
/// table.
#[tokio::test]
async fn the_result_agrees_with_what_a_create_does_with_the_same_definition() {
    let emulator = Emulator::new();

    for (index, Case { case, definition, creatable }) in cases().into_iter().enumerate() {
        let validated = validate(
            &emulator,
            &case,
            &json!({ "definition": definition.clone() }),
        )
        .await;
        let (status, body) = emulator
            .call(
                "CreateStateMachine",
                &json!({
                    "name": format!("validated-{index}"),
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;

        let reported_ok = result_of(&validated) == VALIDATION_RESULT_OK;
        assert_eq!(
            reported_ok,
            status == 200,
            "case {case:?}: the validation reported {:?} while the create answered {status} with \
             {body:?}",
            result_of(&validated)
        );
        if creatable {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to succeed, got {status} with {body:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidDefinition"),
                "case {case:?}: expected 400 InvalidDefinition, got {status} with {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a diagnostic says where it was found
// ---------------------------------------------------------------------------

/// A diagnostic a field or a state owns reports its `location`, "in the format:
/// `/States/<StateName>/<FieldName>`" — including inside a nested state machine,
/// where the path to the state is the path through the state that declares it.
#[tokio::test]
async fn a_diagnostic_reports_where_the_problem_is() {
    let emulator = Emulator::new();

    // Rows differ along WHICH field owns the problem and how deeply it is nested.
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "a transition target on a top-level state",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"Nowhere"}}}"#,
            "/States/A/Next",
        ),
        (
            "a JSONPath member on a JSONata state",
            r#"{"QueryLanguage":"JSONata","StartAt":"A","States":{"A":{"Type":"Pass","Result":{"m":1},"End":true}}}"#,
            "/States/A/Result",
        ),
        (
            "a transition target inside an ItemProcessor",
            r#"{"StartAt":"M","States":{"M":{"Type":"Map","ItemProcessor":{"StartAt":"I","States":{"I":{"Type":"Pass","Next":"Gone"}}},"End":true}}}"#,
            "/States/M/ItemProcessor/States/I/Next",
        ),
        (
            // A required field an object does not carry belongs to the OBJECT —
            // there is no position in the document for a field that is absent — so
            // the diagnostic is located at the nested machine itself.
            "a missing StartAt inside a Parallel branch",
            r#"{"StartAt":"P","States":{"P":{"Type":"Parallel","Branches":[{"States":{"B":{"Type":"Succeed"}}}],"End":true}}}"#,
            "/States/P/Branches/0",
        ),
        (
            "a payload-template member of a JSONPath state",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Parameters":{"copy":"$.x"},"End":true}}}"#,
            "/States/A/Parameters/copy",
        ),
    ];

    for (case, definition, expected) in cases {
        // `WARNING` reports both severities, so one request covers the error rows
        // and the warning row alike.
        let body = validate(
            &emulator,
            case,
            &json!({ "definition": definition, "severity": SEVERITY_WARNING }),
        )
        .await;
        let locations = members(&body, "location");
        assert!(
            locations.iter().any(|location| location == expected),
            "case {case:?}: expected a diagnostic located at {expected:?}, got {locations:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: severity selects what is reported, never the result
// ---------------------------------------------------------------------------

/// `severity` filters the reported list: `ERROR` (the default) reports only errors,
/// `WARNING` reports warnings beside them. The `result` is the same either way,
/// because it is the verdict on the definition rather than on the request.
#[tokio::test]
async fn severity_selects_which_diagnostics_are_reported() {
    let emulator = Emulator::new();

    // A definition carrying one of each: a `Next` that names no state (ERROR) and a
    // `Result` that reads as a path (WARNING).
    const BOTH: &str = r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":"$.x","Next":"Nowhere"},"Done":{"Type":"Succeed"}}}"#;
    // A definition carrying warnings only: a static `Result` that reads as a path,
    // and two payload-template members that read as a path and as an intrinsic.
    const WARNINGS_ONLY: &str = r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Result":"$.x","Parameters":{"copy":"$.y","built":"States.Format('{}', $.y)"},"End":true}}}"#;

    // Rows differ along which severity was asked for (including none at all, which
    // is the documented default of ERROR) and which severities the definition
    // carries.
    let cases: Vec<(&str, Value, &str, Vec<&str>)> = vec![
        (
            "errors and warnings, no severity asked for",
            json!({ "definition": BOTH }),
            VALIDATION_RESULT_FAIL,
            vec![MISSING_TRANSITION_TARGET],
        ),
        (
            "errors and warnings at ERROR",
            json!({ "definition": BOTH, "severity": SEVERITY_ERROR }),
            VALIDATION_RESULT_FAIL,
            vec![MISSING_TRANSITION_TARGET],
        ),
        (
            "errors and warnings at WARNING",
            json!({ "definition": BOTH, "severity": SEVERITY_WARNING }),
            VALIDATION_RESULT_FAIL,
            vec![MISSING_TRANSITION_TARGET, PASS_RESULT_IS_STATIC],
        ),
        (
            "warnings only, no severity asked for",
            json!({ "definition": WARNINGS_ONLY }),
            VALIDATION_RESULT_OK,
            vec![],
        ),
        (
            "warnings only at WARNING",
            json!({ "definition": WARNINGS_ONLY, "severity": SEVERITY_WARNING }),
            VALIDATION_RESULT_OK,
            vec![NO_DOLLAR, PASS_RESULT_IS_STATIC],
        ),
    ];

    for (case, request, expected_result, expected_codes) in cases {
        let body = validate(&emulator, case, &request).await;
        assert_eq!(
            result_of(&body),
            expected_result,
            "case {case:?}: expected result {expected_result}, got {body:?}"
        );
        let codes: BTreeSet<String> = members(&body, "code").into_iter().collect();
        let expected: BTreeSet<String> =
            expected_codes.iter().map(|code| code.to_string()).collect();
        assert_eq!(
            codes, expected,
            "case {case:?}: expected exactly the codes {expected:?}, got {codes:?}"
        );
    }

    // A severity the model does not declare is refused by the modelled enum, not
    // answered with a guess.
    let (status, body) = emulator
        .call(
            "ValidateStateMachineDefinition",
            &json!({ "definition": HELLO_DEFINITION, "severity": "INFO" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "an unmodelled severity is refused with ValidationException, got {status} with {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: maxResults bounds the list, and truncated says so
// ---------------------------------------------------------------------------

/// `maxResults` caps how many diagnostics are reported and `truncated` reports
/// whether more were found; 0 means the documented default of 100, and the result
/// stays `FAIL` however few diagnostics were reported.
#[tokio::test]
async fn max_results_bounds_the_reported_diagnostics() {
    let emulator = Emulator::new();

    // One state per diagnostic: each transitions to a state no definition declares,
    // so the count of errors is known exactly.
    const STATES: usize = 12;
    let mut states: Vec<String> = (0..STATES)
        .map(|index| format!(r#""S{index}":{{"Type":"Pass","Next":"Nowhere"}}"#))
        .collect();
    // Somewhere to stop, so the twelve dangling transitions are the only problem
    // this definition carries.
    states.push(r#""Done":{"Type":"Succeed"}"#.to_string());
    let definition = format!(
        r#"{{"StartAt":"S0","States":{{{}}}}}"#,
        states.join(",")
    );

    // Rows differ along how many were asked for: below the count, at it, past it,
    // at the published maximum, and 0 — which is the documented default.
    for (case, asked, reported) in [
        ("one of twelve", 1_usize, 1_usize),
        ("two of twelve", 2, 2),
        ("five of twelve", 5, 5),
        ("eleven of twelve", 11, 11),
        ("all twelve", 12, 12),
        ("one more than there are", 13, STATES),
        ("the published maximum", DEFAULT_MAX_DIAGNOSTICS, STATES),
        ("zero, which asks for the default", 0, STATES),
    ] {
        let body = validate(
            &emulator,
            case,
            &json!({ "definition": definition, "maxResults": asked }),
        )
        .await;
        assert_eq!(
            result_of(&body),
            VALIDATION_RESULT_FAIL,
            "case {case:?}: the result is the verdict on the definition, not on how many \
             diagnostics were asked for, got {body:?}"
        );
        let found = diagnostics(&body);
        assert_eq!(
            found.len(),
            reported,
            "case {case:?}: expected {reported} diagnostics, got {found:?}"
        );
        assert_eq!(
            body.get("truncated").and_then(Value::as_bool),
            Some(reported < STATES),
            "case {case:?}: truncated is true exactly when more diagnostics were found than \
             reported, got {body:?}"
        );
        for code in members(&body, "code") {
            assert_eq!(
                code, MISSING_TRANSITION_TARGET,
                "case {case:?}: every diagnostic of this definition is a missing transition \
                 target, got {code:?}"
            );
        }
    }

    // The modelled range (0..100) is refused outside its bounds rather than clamped
    // in silence.
    for (case, asked) in [
        ("one past the published maximum", 101),
        ("below the published minimum", -1),
    ] {
        let (status, body) = emulator
            .call(
                "ValidateStateMachineDefinition",
                &json!({ "definition": definition, "maxResults": asked }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: validating creates nothing
// ---------------------------------------------------------------------------

/// "You can validate that a state machine definition is correct without creating a
/// state machine resource": after validating, both a definition that passes and one
/// that fails, the service holds no state machine at all.
#[tokio::test]
async fn validating_a_definition_persists_nothing() {
    let emulator = Emulator::new();

    for (case, definition) in [
        ("a definition that passes verification", HELLO_DEFINITION),
        (
            "a definition that fails verification",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"Nowhere"}}}"#,
        ),
    ] {
        let body = validate(&emulator, case, &json!({ "definition": definition })).await;
        assert!(
            body.get("result").is_some(),
            "case {case:?}: expected a result, got {body:?}"
        );
        let listed = emulator.ok("ListStateMachines", &json!({})).await;
        let machines = listed
            .get("stateMachines")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            machines.is_empty(),
            "case {case:?}: validating creates no state machine, got {listed:?}"
        );
    }
}

/// The walk's own ceiling cannot hide an error behind warnings.
///
/// Its budget is per severity and it keeps looking while it can still report an
/// error, so a definition whose warnings run past that budget still reports the
/// rule it breaks — and `CreateStateMachine`, which reads the same walk, still
/// refuses it. A definition that runs past the ERROR budget reports `truncated`
/// instead of a list that grows with the definition.
#[tokio::test]
async fn warnings_do_not_crowd_out_an_error() {
    let emulator = Emulator::new();

    /// Far more states than the per-severity budget (the documented maximum of
    /// 100, plus one so `truncated` can be reported).
    const MANY: usize = 150;

    /// A definition of `MANY` states that each earn one diagnostic of the given
    /// shape, plus — when `with_error` — one state named last that breaks a rule.
    fn crowded(member: &str, value: &str, with_error: bool) -> String {
        let mut states: Vec<String> = (0..MANY)
            .map(|index| format!(r#""S{index:03}":{{"Type":"Pass","{member}":"{value}","End":true}}"#))
            .collect();
        if with_error {
            states.push(r#""ZLast":{"Type":"Pass","Next":"Nowhere"}"#.to_string());
        }
        format!(r#"{{"StartAt":"S000","States":{{{}}}}}"#, states.join(","))
    }

    // 150 PASS_RESULT_IS_STATIC warnings and one MISSING_TRANSITION_TARGET error.
    let behind_warnings = crowded("Result", "$.x", true);
    // The same 150 warnings and no error at all.
    let warnings_only = crowded("Result", "$.x", false);

    // At the default severity the error is the only diagnostic reported, however
    // many warnings the definition carries.
    let body = validate(
        &emulator,
        "an error behind more warnings than the budget, at ERROR",
        &json!({ "definition": behind_warnings }),
    )
    .await;
    assert_eq!(
        result_of(&body),
        VALIDATION_RESULT_FAIL,
        "an error found after the warning budget is spent still fails the definition, got {body:?}"
    );
    assert_eq!(
        members(&body, "code"),
        vec![MISSING_TRANSITION_TARGET.to_string()],
        "expected the one error of this definition, got {body:?}"
    );
    assert_eq!(
        body.get("truncated").and_then(Value::as_bool),
        Some(false),
        "no ERROR-severity diagnostic was dropped, so nothing was truncated, got {body:?}"
    );

    // Asking for warnings reports them up to the budget, and says so.
    let body = validate(
        &emulator,
        "an error behind more warnings than the budget, at WARNING",
        &json!({ "definition": behind_warnings, "severity": SEVERITY_WARNING }),
    )
    .await;
    assert_eq!(
        result_of(&body),
        VALIDATION_RESULT_FAIL,
        "the result is the same whichever severity was asked for, got {body:?}"
    );
    assert_eq!(
        diagnostics(&body).len(),
        DEFAULT_MAX_DIAGNOSTICS,
        "expected the default maximum of diagnostics, got {body:?}"
    );
    assert_eq!(
        body.get("truncated").and_then(Value::as_bool),
        Some(true),
        "more diagnostics were found than reported, so truncated is true, got {body:?}"
    );

    // The create reads the same walk, so it refuses exactly this definition and
    // accepts the one that carries warnings only.
    let (status, created) = emulator
        .call(
            "CreateStateMachine",
            &json!({
                "name": "crowded-error",
                "definition": behind_warnings,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&created).as_str()),
        (400, "InvalidDefinition"),
        "an error behind the warning budget still refuses the create, got {status} with {created:?}"
    );
    let (status, created) = emulator
        .call(
            "CreateStateMachine",
            &json!({
                "name": "crowded-warnings",
                "definition": warnings_only.clone(),
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "warnings alone never refuse a create, got {status} with {created:?}"
    );
    let body = validate(
        &emulator,
        "warnings only, at ERROR",
        &json!({ "definition": warnings_only }),
    )
    .await;
    assert_eq!(
        result_of(&body),
        VALIDATION_RESULT_OK,
        "a definition carrying warnings only passes verification, got {body:?}"
    );
    assert_eq!(
        body.get("truncated").and_then(Value::as_bool),
        Some(false),
        "no diagnostic of the severity asked for was dropped, got {body:?}"
    );

    // Past the ERROR budget, the reported list is capped and truncated says so.
    let many_errors = crowded("Next", "Nowhere", false);
    let body = validate(
        &emulator,
        "more errors than the budget",
        &json!({ "definition": many_errors }),
    )
    .await;
    assert_eq!(
        result_of(&body),
        VALIDATION_RESULT_FAIL,
        "every state of this definition transitions nowhere, got {body:?}"
    );
    assert_eq!(
        diagnostics(&body).len(),
        DEFAULT_MAX_DIAGNOSTICS,
        "expected the default maximum of diagnostics, got {body:?}"
    );
    assert_eq!(
        body.get("truncated").and_then(Value::as_bool),
        Some(true),
        "more errors were found than reported, so truncated is true, got {body:?}"
    );
}

/// A diagnostic's `location` is bounded however deeply the definition nests and
/// however long its state names are: the path to a state is clipped as it is built,
/// so neither the response nor the work of one request grows with the product of
/// the nesting and the number of states.
#[tokio::test]
async fn a_location_is_bounded_however_deeply_the_definition_nests() {
    let emulator = Emulator::new();

    // Every state is named at the longest a state name may be, and the machines
    // nest 20 deep, so an unclipped path would be far past the bound.
    let name = "p".repeat(MAX_STATE_NAME_CHARS);
    let mut definition = format!(
        r#"{{"StartAt":"{name}","States":{{"{name}":{{"Type":"Pass","Next":"Nowhere"}}}}}}"#
    );
    for _ in 0..20 {
        definition = format!(
            r#"{{"StartAt":"{name}","States":{{"{name}":{{"Type":"Map","ItemProcessor":{definition},"End":true}}}}}}"#
        );
    }

    let case = "a deeply nested definition whose state names are at the bound";
    let body = validate(
        &emulator,
        case,
        &json!({ "definition": definition, "severity": SEVERITY_WARNING }),
    )
    .await;
    assert_eq!(
        result_of(&body),
        VALIDATION_RESULT_FAIL,
        "case {case:?}: the innermost state transitions nowhere, got {body:?}"
    );
    let locations = members(&body, "location");
    assert!(
        !locations.is_empty(),
        "case {case:?}: expected a located diagnostic, got {body:?}"
    );
    for location in &locations {
        assert!(
            location.chars().count() <= MAX_LOCATION_CHARS,
            "case {case:?}: expected every location within {MAX_LOCATION_CHARS} characters, got \
             one of {} ({location:?})",
            location.chars().count()
        );
    }
}

/// A state written in JSONata must carry none of the data-flow members the other
/// query language reads its data through: that language has members of its own
/// (`Arguments`, `Output`, `Assign`), so each JSONPath member is a field of a
/// language the state is not written in, reported as a schema error against the
/// member itself.
#[tokio::test]
async fn a_jsonata_state_carrying_a_jsonpath_member_is_a_schema_error() {
    let emulator = Emulator::new();

    // Rows differ along WHICH JSONPath-only member the JSONata state carries.
    for (case, member, value) in [
        ("a Pass Result", "Result", json!({ "msg": "hi" })),
        ("an InputPath", "InputPath", json!("$.in")),
        ("an OutputPath", "OutputPath", json!("$.out")),
        ("a ResultSelector", "ResultSelector", json!({ "x.$": "$.y" })),
        ("a ToleratedFailureCountPath", "ToleratedFailureCountPath", json!("$.limit")),
    ] {
        let definition = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "A",
            "States": { "A": { "Type": "Pass", member: value, "End": true } },
        })
        .to_string();
        let body = validate(
            &emulator,
            case,
            &json!({ "definition": definition, "severity": SEVERITY_ERROR }),
        )
        .await;
        assert_eq!(
            result_of(&body),
            VALIDATION_RESULT_FAIL,
            "case {case:?}: a JSONata state carrying a JSONPath member fails verification, got \
             {body:?}"
        );
        let codes = members(&body, "code");
        assert!(
            codes.iter().any(|code| code == SCHEMA_VALIDATION_FAILED),
            "case {case:?}: expected a {SCHEMA_VALIDATION_FAILED} diagnostic, got {codes:?}"
        );
        let locations = members(&body, "location");
        let expected = format!("/States/A/{member}");
        assert!(
            locations.contains(&expected),
            "case {case:?}: expected a diagnostic located at {expected:?}, got {locations:?}"
        );
    }
}

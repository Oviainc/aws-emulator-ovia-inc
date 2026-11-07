// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestState` — running ONE state by itself — driven over this
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.TestState`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. the call INTERPRETS the single state the request carries over the input it
//!    carries, and answers what that state produced: the `status` it ended in, the
//!    `output` its data flow computed, the `nextState` it names and, when it failed,
//!    the `error`/`cause` — with no state machine and no execution around it, so a
//!    state whose own work this emulator cannot perform ends `FAILED` with ASL's
//!    `States.Runtime` rather than a fabricated result;
//! 2. `stateName` picks the state out of a fully-formed definition, and the state it
//!    picks is the one that runs;
//! 3. `inspectionLevel` decides what is reported: `INFO` (the default) answers the
//!    final output alone, while `DEBUG` and `TRACE` add the document the state carried
//!    after each filter of the Amazon States Language's data flow;
//! 4. a `mock` stands in for the invocation a `Task`, `Map` or `Parallel` state would
//!    make — its result runs through the state's result-side filters, and its error
//!    output is answered by the state's own `Retry` (`RETRIABLE`) and `Catch`
//!    (`CAUGHT_ERROR`) before it is reported as `FAILED`;
//! 5. a request this operation cannot serve is refused with that reason's own modelled
//!    error, never with a 501 and never with a fabricated run;
//! 6. the call PERSISTS nothing: "you can test a state without creating a state
//!    machine or updating an existing state machine".
//!
//! Each table below spans one class of those rules; the recorded `Pass`-state call
//! this suite's issue was filed about is ONE row of the first one.
//!
//! The harness (`Emulator`, `string`, `document`) is the one `state_machine_lifecycle`
//! and `sync_execution` already build; `tests/all.rs` is this binary's crate root, so
//! it is reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{string, Emulator, ROLE_ARN};
use crate::sync_execution::document;

/// A fully-formed definition whose states differ in what each one does, so a
/// `stateName` can pick any of them.
const ROUTER_DEFINITION: &str = r#"{
    "StartAt": "Route",
    "States": {
        "Route": {
            "Type": "Choice",
            "Choices": [{ "Variable": "$.n", "NumericGreaterThan": 0, "Next": "Double" }],
            "Default": "Zero"
        },
        "Double": { "Type": "Pass", "Parameters": { "n.$": "States.MathAdd($.n, $.n)" }, "Next": "Done" },
        "Zero": { "Type": "Pass", "Result": { "n": 0 }, "Next": "Done" },
        "Done": { "Type": "Succeed" }
    }
}"#;

/// A `Task` state carrying every member a mocked invocation runs through: an input
/// template, both result-side filters, a retrier and a catcher.
const MOCKED_TASK: &str = r#"{
    "Type": "Task",
    "Resource": "arn:aws:states:::lambda:invoke",
    "Parameters": { "FunctionName": "summer", "Payload.$": "$" },
    "ResultSelector": { "total.$": "$.Payload.sum" },
    "ResultPath": "$.result",
    "Retry": [{
        "ErrorEquals": ["Lambda.TooManyRequestsException"],
        "MaxAttempts": 2,
        "IntervalSeconds": 3,
        "BackoffRate": 2
    }],
    "Catch": [{ "ErrorEquals": ["States.TaskFailed"], "Next": "Recover", "ResultPath": "$.failure" }],
    "Next": "After"
}"#;

/// The state's `inspectionData`, or `None` when the response carries none.
fn inspection(body: &Value) -> Option<&Value> {
    body.get("inspectionData").filter(|data| !data.is_null())
}

/// Assert that `member` of the response's `inspectionData` carries `expected` as a
/// JSON document, or no value at all when `expected` is `None`.
fn assert_stage(case: &str, body: &Value, member: &str, expected: Option<&str>) {
    let data = inspection(body)
        .unwrap_or_else(|| panic!("case {case:?}: expected inspectionData in {body:?}"));
    let actual = document(data, member);
    let expected = expected.map(|text| {
        serde_json::from_str::<Value>(text).expect("the expected inspection stage is JSON")
    });
    assert_eq!(
        actual, expected,
        "case {case:?}: expected inspectionData.{member} {expected:?}, got {data:?}"
    );
}

/// `TestState`'s request body for one single-state definition.
fn test_request(definition: &str, input: Option<&str>) -> Value {
    let mut request = json!({ "definition": definition, "roleArn": ROLE_ARN });
    if let Some(input) = input {
        request["input"] = Value::String(input.to_string());
    }
    request
}

// ---------------------------------------------------------------------------
// What the state produced
// ---------------------------------------------------------------------------

/// One row of the single-state table.
struct StateCase {
    case: &'static str,
    /// The definition of the ONE state under test.
    definition: &'static str,
    /// The `input` the request carries, or `None` to send none at all.
    input: Option<&'static str>,
    /// The `TestExecutionStatus` the state must end in.
    status: &'static str,
    /// The JSON output the state must answer, if any.
    output: Option<&'static str>,
    /// The state the response must name as the next one, if any.
    next_state: Option<&'static str>,
    /// The `error` a failing state must answer, if any.
    error: Option<&'static str>,
}

/// `TestState` answers what the single state it was given produced, computed from
/// that state's own definition and the input the request carried.
///
/// Rows vary along what the state DOES (a static result, a payload template, path
/// data flow, a wait, a choice, a succeed, a fail), the input it runs on (an object,
/// none at all), where it says it goes next (`Next`, `End`, neither, a choice rule, a
/// default), and whether this emulator can perform its work at all.
#[tokio::test]
async fn test_state_answers_what_the_single_state_produced() {
    let cases = [
        StateCase {
            // The shape of the recorded call this suite's issue was filed about: a
            // single `Pass` state tested with no execution anywhere.
            case: "a Pass state's static result",
            definition: r#"{"Type":"Pass","Result":{"msg":"hi"},"End":true}"#,
            input: Some("{}"),
            status: "SUCCEEDED",
            output: Some(r#"{"msg":"hi"}"#),
            next_state: None,
            error: None,
        },
        StateCase {
            case: "a Pass state carries its input through to the state it names",
            definition: r#"{"Type":"Pass","Next":"Second"}"#,
            input: Some(r#"{"n":1}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"n":1}"#),
            next_state: Some("Second"),
            error: None,
        },
        StateCase {
            case: "no input at all is tested on the empty object",
            definition: r#"{"Type":"Pass","End":true}"#,
            input: None,
            status: "SUCCEEDED",
            output: Some("{}"),
            next_state: None,
            error: None,
        },
        StateCase {
            case: "a state carrying neither Next nor End has no next state",
            definition: r#"{"Type":"Pass","Result":7}"#,
            input: Some("{}"),
            status: "SUCCEEDED",
            output: Some("7"),
            next_state: None,
            error: None,
        },
        StateCase {
            case: "Parameters build the result from the input and an intrinsic",
            definition: r#"{"Type":"Pass","Parameters":{"who.$":"$.name","size.$":"States.ArrayLength($.list)"},"End":true}"#,
            input: Some(r#"{"name":"ada","list":[1,2,3]}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"who":"ada","size":3}"#),
            next_state: None,
            error: None,
        },
        StateCase {
            case: "InputPath, ResultPath and OutputPath place the result",
            definition: r#"{"Type":"Pass","InputPath":"$.detail","Result":"ok","ResultPath":"$.verdict","OutputPath":"$.verdict","End":true}"#,
            input: Some(r#"{"detail":{"n":2}}"#),
            status: "SUCCEEDED",
            output: Some(r#""ok""#),
            next_state: None,
            error: None,
        },
        StateCase {
            // A `Wait` is answered now rather than slept through: a test that waited
            // for this one would take ten minutes.
            case: "a Wait state answers its effective input without sleeping",
            definition: r#"{"Type":"Wait","Seconds":600,"Next":"After"}"#,
            input: Some(r#"{"k":1}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"k":1}"#),
            next_state: Some("After"),
            error: None,
        },
        StateCase {
            case: "a Choice rule that matches names the state it goes to",
            definition: r#"{"Type":"Choice","Choices":[{"Variable":"$.n","NumericGreaterThan":1,"Next":"Big"}],"Default":"Small"}"#,
            input: Some(r#"{"n":7}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"n":7}"#),
            next_state: Some("Big"),
            error: None,
        },
        StateCase {
            case: "a Choice no rule matched falls to its Default",
            definition: r#"{"Type":"Choice","Choices":[{"Variable":"$.n","NumericGreaterThan":1,"Next":"Big"}],"Default":"Small"}"#,
            input: Some(r#"{"n":0}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"n":0}"#),
            next_state: Some("Small"),
            error: None,
        },
        StateCase {
            case: "a Choice with no Default and no match reports ASL's own error",
            definition: r#"{"Type":"Choice","Choices":[{"Variable":"$.n","NumericEquals":1,"Next":"P"}]}"#,
            input: Some(r#"{"n":2}"#),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("States.Runtime"),
        },
        StateCase {
            case: "a Succeed state ends with the input it was given",
            definition: r#"{"Type":"Succeed"}"#,
            input: Some(r#"{"done":true}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"done":true}"#),
            next_state: None,
            error: None,
        },
        StateCase {
            case: "a Fail state fails with the error it declares",
            definition: r#"{"Type":"Fail","Error":"Boom","Cause":"on purpose"}"#,
            input: Some("{}"),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("Boom"),
        },
        StateCase {
            case: "a Fail state reads its error from the state's input",
            definition: r#"{"Type":"Fail","ErrorPath":"$.err","CausePath":"$.why"}"#,
            input: Some(r#"{"err":"FromInput","why":"because"}"#),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("FromInput"),
        },
        StateCase {
            case: "a Task state's service integration is not run",
            definition: r#"{"Type":"Task","Resource":"arn:aws:lambda:us-west-2:000000000000:function:f","End":true}"#,
            input: Some(r#"{"first_name":"Alejandro"}"#),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("States.Runtime"),
        },
        StateCase {
            // A JSONata state is evaluated here as it is in an execution: its
            // `Output` is the document the state puts out.
            case: "a JSONata state is evaluated",
            definition: r#"{"Type":"Pass","QueryLanguage":"JSONata","Output":"{% { 'sum': $states.input.a + $states.input.b } %}","End":true}"#,
            input: Some(r#"{"a":2,"b":3}"#),
            status: "SUCCEEDED",
            output: Some(r#"{"sum":5}"#),
            next_state: None,
            error: None,
        },
        StateCase {
            case: "a path that selects no value fails the state",
            definition: r#"{"Type":"Pass","InputPath":"$.absent","End":true}"#,
            input: Some(r#"{"present":1}"#),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("States.Runtime"),
        },
        StateCase {
            // Only a REFERENCE path "can identify only a single node": a wildcard
            // walks every child, so it is refused rather than answered with whichever
            // child the document happens to carry.
            case: "a wildcard is not a reference path",
            definition: r#"{"Type":"Pass","InputPath":"$.*","End":true}"#,
            input: Some(r#"{"only":1}"#),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("States.Runtime"),
        },
        StateCase {
            case: "a wildcard subscript is not a reference path",
            definition: r#"{"Type":"Pass","InputPath":"$[*]","End":true}"#,
            input: Some("[1]"),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("States.Runtime"),
        },
        StateCase {
            // A union selects as many nodes as it lists, and a path's text does not
            // bound that count, so it is refused BEFORE the engine builds the list.
            case: "a union of member names is not a reference path",
            definition: r#"{"Type":"Pass","InputPath":"$['a','a','a']","End":true}"#,
            input: Some(r#"{"a":1}"#),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("States.Runtime"),
        },
        StateCase {
            // The quotes a union is written in are refused too, however the subscript
            // is spelled: the guard is on the segments, not on the outer quotes.
            case: "a quoted member name carrying a quote is not a reference path",
            definition: r#"{"Type":"Pass","OutputPath":"$[\"a\",\"a\"]","End":true}"#,
            input: Some(r#"{"a":1}"#),
            status: "FAILED",
            output: None,
            next_state: None,
            error: Some("States.Runtime"),
        },
        StateCase {
            // A member name that is not a bare identifier still reaches its one node:
            // the engine is handed the canonical, quoted form of the segments.
            case: "a bracketed member name selects its one node",
            definition: r#"{"Type":"Pass","InputPath":"$['a.b'][1]","End":true}"#,
            input: Some(r#"{"a.b":[10,11]}"#),
            status: "SUCCEEDED",
            output: Some("11"),
            next_state: None,
            error: None,
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let label = case.case;
        let answered = emulator
            .ok("TestState", &test_request(case.definition, case.input))
            .await;

        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some(case.status),
            "case {label:?}: expected status {:?}, got {answered:?}",
            case.status
        );
        assert_eq!(
            document(&answered, "output"),
            case.output
                .map(|output| serde_json::from_str(output).expect("the expected output is JSON")),
            "case {label:?}: expected output {:?}, got {answered:?}",
            case.output
        );
        assert_eq!(
            answered.get("nextState").and_then(Value::as_str),
            case.next_state,
            "case {label:?}: expected nextState {:?}, got {answered:?}",
            case.next_state
        );
        assert_eq!(
            answered.get("error").and_then(Value::as_str),
            case.error,
            "case {label:?}: expected error {:?}, got {answered:?}",
            case.error
        );
        if case.error.is_some_and(|error| error.starts_with("States.")) {
            assert!(
                answered
                    .get("cause")
                    .and_then(Value::as_str)
                    .is_some_and(|cause| !cause.is_empty()),
                "case {label:?}: a failure this emulator raised must say what happened, got \
                 {answered:?}"
            );
        }
        // The default level answers the final output alone.
        assert!(
            inspection(&answered).is_none(),
            "case {label:?}: INFO is the default level and reports no inspectionData, got \
             {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// stateName: which state of a definition runs
// ---------------------------------------------------------------------------

/// `stateName` "denotes the particular state within a state machine definition to be
/// tested", and the state it names is the one that runs — every other state of the
/// same definition is left alone.
///
/// Rows span EVERY state the one definition declares, so the rule is pinned on each
/// state type it carries rather than on one of them.
#[tokio::test]
async fn state_name_picks_the_state_of_the_definition_that_runs() {
    // (the stateName, the output that state produces, the state it goes to)
    let cases: [(&str, &str, Option<&str>); 4] = [
        ("Route", r#"{"n":3}"#, Some("Double")),
        ("Double", r#"{"n":6}"#, Some("Done")),
        ("Zero", r#"{"n":0}"#, Some("Done")),
        ("Done", r#"{"n":3}"#, None),
    ];

    let emulator = Emulator::new();
    for (state_name, output, next_state) in cases {
        let answered = emulator
            .ok(
                "TestState",
                &json!({
                    "definition": ROUTER_DEFINITION,
                    "stateName": state_name,
                    "input": r#"{"n":3}"#,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {state_name:?}: expected the state to run, got {answered:?}"
        );
        assert_eq!(
            document(&answered, "output"),
            Some(serde_json::from_str(output).expect("the expected output is JSON")),
            "case {state_name:?}: expected output {output}, got {answered:?}"
        );
        assert_eq!(
            answered.get("nextState").and_then(Value::as_str),
            next_state,
            "case {state_name:?}: expected nextState {next_state:?}, got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// inspectionLevel: what is reported
// ---------------------------------------------------------------------------

/// Every stage of the data flow a state carrying all of them produces, which the two
/// data-flow levels report and `INFO` does not.
///
/// `afterResultSelector` is absent because the Amazon States Language gives a `Pass`
/// state a `Result`, not a `ResultSelector`, and the HTTP `request`/`response` are
/// absent because they are an HTTP Task's, which no state tested here is.
#[tokio::test]
async fn the_inspection_level_decides_what_is_reported() {
    const DEFINITION: &str = r#"{"Type":"Pass","InputPath":"$.detail","Parameters":{"doubled.$":"States.MathAdd($.n,$.n)"},"ResultPath":"$.out","End":true}"#;
    const INPUT: &str = r#"{"detail":{"n":2},"keep":true}"#;

    // (the level the request names, whether the data flow is reported)
    let cases: [(&str, Option<&str>, bool); 4] = [
        ("no level at all defaults to INFO", None, false),
        ("the modelled INFO level", Some("INFO"), false),
        ("the modelled DEBUG level", Some("DEBUG"), true),
        ("the modelled TRACE level", Some("TRACE"), true),
    ];

    let emulator = Emulator::new();
    for (case, level, reported) in cases {
        let mut request = json!({
            "definition": DEFINITION,
            "input": INPUT,
            "roleArn": ROLE_ARN,
            "variables": r#"{"x":1}"#,
        });
        if let Some(level) = level {
            request["inspectionLevel"] = Value::String(level.to_string());
        }
        let answered = emulator.ok("TestState", &request).await;

        // Whatever the level, the state ran and the output is the same.
        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the state to run, got {answered:?}"
        );
        assert_eq!(
            document(&answered, "output"),
            Some(json!({ "detail": { "n": 2 }, "keep": true, "out": { "doubled": 4 } })),
            "case {case:?}: expected the placed result, got {answered:?}"
        );

        if !reported {
            assert!(
                inspection(&answered).is_none(),
                "case {case:?}: this level reports the final output alone, got {answered:?}"
            );
            continue;
        }

        // Each stage holds the document the state carried AFTER that filter.
        assert_stage(case, &answered, "input", Some(INPUT));
        assert_stage(case, &answered, "afterInputPath", Some(r#"{"n":2}"#));
        assert_stage(case, &answered, "afterParameters", Some(r#"{"doubled":4}"#));
        assert_stage(case, &answered, "result", Some(r#"{"doubled":4}"#));
        assert_stage(case, &answered, "afterResultSelector", None);
        assert_stage(
            case,
            &answered,
            "afterResultPath",
            Some(r#"{"detail":{"n":2},"keep":true,"out":{"doubled":4}}"#),
        );
        // "The set will include variables assigned in the state and variables set up
        // as test state input": this state assigns none.
        assert_stage(case, &answered, "variables", Some(r#"{"x":1}"#));
        let data = inspection(&answered).expect("the data flow is reported");
        for absent in ["request", "response", "errorDetails"] {
            assert!(
                data.get(absent).is_none_or(Value::is_null),
                "case {case:?}: no state tested here has a {absent}, got {data:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// mock: the invocation it stands in for
// ---------------------------------------------------------------------------

/// One row of the mocked-result table.
struct MockedResultCase {
    case: &'static str,
    /// The definition of the ONE state under test.
    definition: &'static str,
    /// The `mock.result` the request carries.
    result: &'static str,
    input: &'static str,
    /// The output the state's result-side filters place the mocked result into.
    output: Value,
}

/// A mocked RESULT is the state's raw result: it runs through the state's own
/// result-side filters, and the state goes to the state it names.
///
/// Rows span the three state types a mock may be specified for (`Task`, `Map`,
/// `Parallel`) and, for the `Task`, the filters a mocked result runs through.
#[tokio::test]
async fn a_mocked_result_runs_through_the_state_s_own_filters() {
    let cases = [
        MockedResultCase {
            case: "a Task's ResultSelector and ResultPath place the mocked result",
            definition: MOCKED_TASK,
            result: r#"{"Payload":{"sum":7}}"#,
            input: r#"{"a":3,"b":4}"#,
            output: json!({ "a": 3, "b": 4, "result": { "total": 7 } }),
        },
        MockedResultCase {
            case: "a Map state's iterations are the mocked result",
            definition: r#"{"Type":"Map","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"ResultPath":"$.seen","Next":"After"}"#,
            result: r#"["one","two"]"#,
            input: r#"{"items":[1,2]}"#,
            output: json!({ "items": [1, 2], "seen": ["one", "two"] }),
        },
        MockedResultCase {
            case: "a Parallel state's branches are the mocked result",
            definition: r#"{"Type":"Parallel","Branches":[{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}}],"OutputPath":"$[0]","Next":"After"}"#,
            result: r#"[{"branch":"first"}]"#,
            input: "{}",
            output: json!({ "branch": "first" }),
        },
    ];

    let emulator = Emulator::new();
    for MockedResultCase {
        case,
        definition,
        result,
        input,
        output,
    } in cases
    {
        let answered = emulator
            .ok(
                "TestState",
                &json!({
                    "definition": definition,
                    "input": input,
                    "roleArn": ROLE_ARN,
                    "inspectionLevel": "DEBUG",
                    "mock": { "result": result },
                }),
            )
            .await;
        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the mocked state to produce its output, got {answered:?}"
        );
        assert_eq!(
            document(&answered, "output"),
            Some(output.clone()),
            "case {case:?}: expected output {output:?}, got {answered:?}"
        );
        assert_eq!(
            answered.get("nextState").and_then(Value::as_str),
            Some("After"),
            "case {case:?}: expected the state the definition names, got {answered:?}"
        );
        // The mocked result is the state's RAW result, before the result-side filters.
        assert_stage(case, &answered, "result", Some(result));
    }
}

/// One row of the mocked-error table.
struct MockedErrorCase {
    case: &'static str,
    /// The error name the mocked error output carries.
    error: &'static str,
    /// `stateConfiguration.retrierRetryCount`, or `None` to send none at all.
    taken: Option<i64>,
    /// The `TestExecutionStatus` the state's own error handling answers with.
    status: &'static str,
    /// The state a caught failure carries the error output to, if any.
    next_state: Option<&'static str>,
    /// The retry interval a RETRIABLE failure's retrier waits, in seconds.
    interval: Option<i64>,
}

/// A mocked ERROR OUTPUT is answered by the state's own error handling: a `Retry`
/// retrier that still has an attempt left makes the state `RETRIABLE`, a `Catch`
/// catcher makes it `CAUGHT_ERROR` and carries the error output to its own `Next`,
/// and a failure neither answers for is `FAILED`.
///
/// Rows vary along the error the mock names — including the one name the catcher's
/// documented wildcard does not take over — and the number of attempts the request
/// says have already been taken, which is what crosses the retrier's `MaxAttempts`.
#[tokio::test]
async fn a_mocked_error_is_answered_by_the_state_s_own_retry_and_catch() {
    let cases = [
        MockedErrorCase {
            case: "a retrier with every attempt left",
            error: "Lambda.TooManyRequestsException",
            taken: None,
            status: "RETRIABLE",
            next_state: None,
            interval: Some(3),
        },
        MockedErrorCase {
            case: "the same retrier one attempt in backs off",
            error: "Lambda.TooManyRequestsException",
            taken: Some(1),
            status: "RETRIABLE",
            next_state: None,
            interval: Some(6),
        },
        MockedErrorCase {
            case: "the retrier out of attempts falls through to the catcher",
            error: "Lambda.TooManyRequestsException",
            taken: Some(2),
            status: "CAUGHT_ERROR",
            next_state: Some("Recover"),
            interval: None,
        },
        MockedErrorCase {
            case: "an error a catcher names",
            error: "States.TaskFailed",
            taken: None,
            status: "CAUGHT_ERROR",
            next_state: Some("Recover"),
            interval: None,
        },
        MockedErrorCase {
            // "States.TaskFailed acts as a wildcard that matches any known error name
            // except for States.Timeout": the one error this catcher does not take
            // over, so the state fails with it.
            case: "the one error the catcher's wildcard excludes",
            error: "States.Timeout",
            taken: None,
            status: "FAILED",
            next_state: None,
            interval: None,
        },
    ];

    let emulator = Emulator::new();
    for MockedErrorCase {
        case,
        error,
        taken,
        status,
        next_state,
        interval,
    } in cases
    {
        let mut request = json!({
            "definition": MOCKED_TASK,
            "input": r#"{"a":1}"#,
            "roleArn": ROLE_ARN,
            "inspectionLevel": "DEBUG",
            "mock": { "errorOutput": { "error": error, "cause": "the mock said so" } },
        });
        if let Some(taken) = taken {
            request["stateConfiguration"] = json!({ "retrierRetryCount": taken });
        }
        let answered = emulator.ok("TestState", &request).await;

        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some(status),
            "case {case:?}: expected status {status:?}, got {answered:?}"
        );
        assert_eq!(
            answered.get("error").and_then(Value::as_str),
            Some(error),
            "case {case:?}: expected the mocked error, got {answered:?}"
        );
        assert_eq!(
            answered.get("cause").and_then(Value::as_str),
            Some("the mock said so"),
            "case {case:?}: expected the mocked cause, got {answered:?}"
        );
        assert_eq!(
            answered.get("nextState").and_then(Value::as_str),
            next_state,
            "case {case:?}: expected nextState {next_state:?}, got {answered:?}"
        );

        let details = inspection(&answered)
            .and_then(|data| data.get("errorDetails"))
            .filter(|details| !details.is_null());
        match (status, next_state) {
            // A retried state owes an attempt: the response says which retrier and
            // how long it waits.
            ("RETRIABLE", _) => {
                let details = details
                    .unwrap_or_else(|| panic!("case {case:?}: expected errorDetails in {answered:?}"));
                assert_eq!(
                    details.get("retryIndex").and_then(Value::as_i64),
                    Some(0),
                    "case {case:?}: expected the retrier that matched, got {details:?}"
                );
                assert_eq!(
                    details
                        .get("retryBackoffIntervalSeconds")
                        .and_then(Value::as_i64),
                    interval,
                    "case {case:?}: expected a {interval:?} second interval, got {details:?}"
                );
            }
            // A caught state's output is the error output, placed through the
            // catcher's own `ResultPath`.
            ("CAUGHT_ERROR", _) => {
                let details = details
                    .unwrap_or_else(|| panic!("case {case:?}: expected errorDetails in {answered:?}"));
                assert_eq!(
                    details.get("catchIndex").and_then(Value::as_i64),
                    Some(0),
                    "case {case:?}: expected the catcher that matched, got {details:?}"
                );
                assert_eq!(
                    document(&answered, "output"),
                    Some(json!({
                        "a": 1,
                        "failure": { "Error": error, "Cause": "the mock said so" },
                    })),
                    "case {case:?}: expected the caught error output, got {answered:?}"
                );
            }
            _ => {
                assert!(
                    answered.get("output").is_none_or(Value::is_null),
                    "case {case:?}: a failed state has no output, got {answered:?}"
                );
                assert!(
                    details.is_none(),
                    "case {case:?}: nothing handled this failure, got {answered:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The requests it refuses
// ---------------------------------------------------------------------------

/// A request this operation cannot serve is refused with that reason's own modelled
/// error — never with a 501, and never with a fabricated run.
///
/// Rows vary along what is wrong with the request: the definition's JSON, its form,
/// the state the `stateName` names, the input's JSON, the role ARN, the variables,
/// the inspection level, and the combinations `mock` and `context` may not be sent in.
#[tokio::test]
async fn test_state_refuses_a_request_it_cannot_serve() {
    const PASS_STATE: &str = r#"{"Type":"Pass","End":true}"#;
    const MAP_STATE: &str = r#"{"Type":"Map","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}"#;
    const PARALLEL_STATE: &str = r#"{"Type":"Parallel","Branches":[{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}}],"End":true}"#;

    // (case, the request body, the expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a definition that is not JSON",
            json!({ "definition": "not json" }),
            "InvalidDefinition",
        ),
        (
            "a definition that is a state with no Type",
            json!({ "definition": r#"{"Result":1,"End":true}"# }),
            "InvalidDefinition",
        ),
        (
            "a fully-formed definition with no stateName is not a state",
            json!({ "definition": ROUTER_DEFINITION }),
            "InvalidDefinition",
        ),
        (
            "a stateName with a definition that is only a state",
            json!({ "definition": PASS_STATE, "stateName": "Hello" }),
            "InvalidDefinition",
        ),
        (
            "a terminal state carrying Next",
            json!({ "definition": r#"{"Type":"Succeed","Next":"Nowhere"}"# }),
            "InvalidDefinition",
        ),
        (
            "a stateName the definition does not declare",
            json!({ "definition": ROUTER_DEFINITION, "stateName": "Absent" }),
            "ValidationException",
        ),
        (
            "an input that is not JSON",
            json!({ "definition": PASS_STATE, "input": "not json" }),
            "InvalidExecutionInput",
        ),
        (
            "an input that is a truncated document",
            json!({ "definition": PASS_STATE, "input": "{\"a\":" }),
            "InvalidExecutionInput",
        ),
        (
            "a roleArn that is not an ARN",
            json!({ "definition": PASS_STATE, "roleArn": "not-an-arn" }),
            "InvalidArn",
        ),
        (
            "a roleArn that is not an IAM role",
            json!({ "definition": PASS_STATE, "roleArn": "arn:aws:iam::000000000000:user/someone" }),
            "InvalidArn",
        ),
        (
            "variables that are not a JSON object",
            json!({ "definition": PASS_STATE, "variables": "[1,2]" }),
            "ValidationException",
        ),
        (
            "variables that are not JSON at all",
            json!({ "definition": PASS_STATE, "variables": "x=1" }),
            "ValidationException",
        ),
        (
            "an inspectionLevel outside the modelled enum",
            json!({ "definition": PASS_STATE, "inspectionLevel": "SHOUT" }),
            "ValidationException",
        ),
        (
            "a mock on a state type that takes none",
            json!({ "definition": PASS_STATE, "mock": { "result": "{}" } }),
            "ValidationException",
        ),
        (
            "a Map state with no mock to stand in for its iterations",
            json!({ "definition": MAP_STATE }),
            "ValidationException",
        ),
        (
            "a Parallel state with no mock to stand in for its branches",
            json!({ "definition": PARALLEL_STATE }),
            "ValidationException",
        ),
        (
            "a mock naming both a result and an errorOutput",
            json!({
                "definition": MOCKED_TASK,
                "mock": { "result": "{}", "errorOutput": { "error": "Boom" } },
            }),
            "ValidationException",
        ),
        (
            "a mock naming neither a result nor an errorOutput",
            json!({ "definition": MOCKED_TASK, "mock": {} }),
            "ValidationException",
        ),
        (
            "a mock whose result is not JSON",
            json!({ "definition": MOCKED_TASK, "mock": { "result": "not json" } }),
            "ValidationException",
        ),
        (
            "a context with no mock in the same request",
            json!({ "definition": PASS_STATE, "context": "{}" }),
            "ValidationException",
        ),
        (
            "a context that is not a JSON object",
            json!({
                "definition": MOCKED_TASK,
                "mock": { "result": "{}" },
                "context": "[1]",
            }),
            "ValidationException",
        ),
    ];

    let emulator = Emulator::new();
    for (case, request, expected) in cases {
        let (status, code) = emulator.err("TestState", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected a 400 {expected}, got {status} {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the call leaves behind
// ---------------------------------------------------------------------------

/// "You can test a state without creating a state machine or updating an existing
/// state machine": the call persists nothing, so the state machines and executions a
/// later caller sees are exactly the ones that were there before it.
///
/// Rows span the outcomes a test can end in — a state that succeeded, one that
/// failed, and one whose mocked error was caught — because a call that minted a
/// record would do it on any of them.
#[tokio::test]
async fn testing_a_state_persists_nothing() {
    let emulator = Emulator::new();

    // One real state machine, so the assertion is that the store is UNCHANGED rather
    // than that it is empty.
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "untouched",
                "definition": crate::state_machine_lifecycle::HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let machine = string(&created, "stateMachineArn");

    let cases: [(&str, Value); 3] = [
        (
            "a state that succeeded",
            json!({ "definition": r#"{"Type":"Pass","Result":1,"End":true}"#, "input": "{}" }),
        ),
        (
            "a state that failed",
            json!({ "definition": r#"{"Type":"Fail","Error":"Boom"}"#, "input": "{}" }),
        ),
        (
            "a state whose mocked error was caught",
            json!({
                "definition": MOCKED_TASK,
                "input": "{}",
                "mock": { "errorOutput": { "error": "States.TaskFailed", "cause": "mocked" } },
            }),
        ),
    ];

    for (case, mut request) in cases {
        request["roleArn"] = Value::String(ROLE_ARN.to_string());
        let answered = emulator.ok("TestState", &request).await;
        assert!(
            answered.get("status").and_then(Value::as_str).is_some(),
            "case {case:?}: expected the state's status, got {answered:?}"
        );

        let store = emulator.store();
        let store = store.lock();
        assert_eq!(
            store.state_machines.len(),
            1,
            "case {case:?}: testing a state creates no state machine, got {:?}",
            store.state_machines.keys().collect::<Vec<_>>()
        );
        assert!(
            store.state_machines.contains_key(&machine),
            "case {case:?}: the state machine that was there is still there, got {:?}",
            store.state_machines.keys().collect::<Vec<_>>()
        );
        assert!(
            store.executions.is_empty(),
            "case {case:?}: testing a state starts no execution, got {:?}",
            store.executions.keys().collect::<Vec<_>>()
        );
    }

    // And the same test answers the same thing twice: nothing about it was consumed.
    let (first, second) = (
        emulator
            .ok(
                "TestState",
                &json!({
                    "definition": r#"{"Type":"Pass","Parameters":{"n.$":"States.MathAdd($.n,1)"},"End":true}"#,
                    "input": r#"{"n":41}"#,
                    "roleArn": ROLE_ARN,
                    "inspectionLevel": "DEBUG",
                }),
            )
            .await,
        emulator
            .ok(
                "TestState",
                &json!({
                    "definition": r#"{"Type":"Pass","Parameters":{"n.$":"States.MathAdd($.n,1)"},"End":true}"#,
                    "input": r#"{"n":41}"#,
                    "roleArn": ROLE_ARN,
                    "inspectionLevel": "DEBUG",
                }),
            )
            .await,
    );
    assert_eq!(
        first, second,
        "testing the same state twice answers the same thing"
    );
    assert_eq!(
        document(&first, "output"),
        Some(json!({ "n": 42 })),
        "expected the state's computed output, got {first:?}"
    );
}

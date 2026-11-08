// SPDX-License-Identifier: CC0-1.0

//! The interpreter's DATA fidelity: what a `Choice` state assigns, what a comparator
//! it cannot apply decides, what `States.Hash` computes, what a `Catch` reading the
//! error output recovers, and what a JSON number a state carries comes back as.
//!
//! Each suite pins one rule over a TABLE whose rows differ along what that rule
//! generalises over, driven end to end over this service's own wire protocol through
//! the production `MultiServiceRouter`:
//!
//! * a `Choice` rule's own `Assign` is the SELECTED rule's, applied where the state's
//!   own would be — so the state-level `Assign` applies only where no rule matched;
//! * a value comparator that cannot be applied to the operands' TYPES does not match,
//!   while a structural defect of the rule still fails the execution;
//! * a `Catch` whose `Output` reads `$states.errorOutput` recovers the execution at the
//!   catcher's `Next`;
//! * a JSON number a state carries through keeps the digits it was written with.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`)
//! is the one `state_machine_lifecycle` builds; the `Catch` suite drives the harness
//! `integration_failure_error_names` builds, because its task's call has to be served
//! by a mounted service for real.

use serde_json::{json, Value};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// Run `definition` over `input` and describe the execution once it has settled.
async fn ran(emulator: &Emulator, name: &str, definition: &Value, input: &str) -> Value {
    let machine = create_machine(emulator, name, &definition.to_string()).await;
    let execution = start_execution(emulator, &machine, "only", input).await;
    ::tokio::time::sleep(PAST_THE_EXECUTION_DELAY).await;
    describe_execution(emulator, &execution).await
}

/// The `output` of a described execution, parsed.
fn output(described: &Value) -> Value {
    described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// A Choice rule's own Assign
// ---------------------------------------------------------------------------

/// A JSONata `Choice` state's assignments are the SELECTED rule's: the state-level
/// `Assign` is applied only where NO rule matched and the `Default` was taken, so a
/// variable a matched rule does not declare is not one the state's own `Assign` writes
/// either.
///
/// What a state assigned is read off the event the state is LEFT by ("map of variable
/// name and value as a serialized JSON representation"), which observes the rule
/// directly and dereferences no variable: an expression reading a variable NOTHING in
/// scope assigned is its own question, which this suite does not prejudge.
///
/// The rows differ along which rule is selected (the first of three whose conditions
/// ALL hold, the second alone, a matched rule that declares no `Assign`, and the
/// `Default` that is no rule at all) — the dimensions the rule generalises over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_choice_rules_own_assign_is_the_selected_rules() {
    // Every rule's condition holds for `n >= 10`; the second and third for `n >= 5`.
    // The state and the first two rules all declare `tag`, so WHOSE assignment the
    // state makes is exactly the question.
    let router = json!({
        "QueryLanguage": "JSONata",
        "StartAt": "Pick",
        "States": {
            "Pick": {
                "Type": "Choice",
                "Assign": { "tag": "from-state-level", "seen": "{% $states.input.n %}" },
                "Choices": [
                    {
                        "Condition": "{% $states.input.n >= 10 %}",
                        "Assign": { "branch": "first", "tag": "from-first-rule" },
                        "Next": "Report",
                    },
                    {
                        "Condition": "{% $states.input.n >= 5 %}",
                        "Assign": { "branch": "second", "tag": "from-second-rule" },
                        "Next": "Report",
                    },
                    {
                        // A rule that MATCHES and declares no `Assign` of its own, so
                        // nothing is assigned on its path at all.
                        "Condition": "{% $states.input.n >= 2 %}",
                        "Next": "Report",
                    },
                ],
                "Default": "Report",
            },
            "Report": { "Type": "Pass", "End": true }
        }
    });

    // (case, the input, the variables the `Choice` must report having assigned —
    // `None` for a path that assigns none at all)
    let rows: Vec<(&str, &str, Option<Value>)> = vec![
        (
            "every rule matches, so the FIRST rule's own Assign is the state's",
            r#"{"n":50}"#,
            Some(json!({ "branch": "\"first\"", "tag": "\"from-first-rule\"" })),
        ),
        (
            "the second rule is the first to match, so its own Assign is the state's",
            r#"{"n":7}"#,
            Some(json!({ "branch": "\"second\"", "tag": "\"from-second-rule\"" })),
        ),
        (
            // The matched rule declares no `Assign`, and the state's own is NOT
            // applied on a matched path, so the state assigned nothing.
            "the matched rule declares no Assign, so nothing is assigned",
            r#"{"n":3}"#,
            None,
        ),
        (
            "no rule matches, so the state's own Assign is the one that applies",
            r#"{"n":1}"#,
            Some(json!({ "seen": "1", "tag": "\"from-state-level\"" })),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, input, expected)) in rows.into_iter().enumerate() {
        let machine =
            create_machine(&emulator, &format!("choice-assign-{index}"), &router.to_string()).await;
        let execution = start_execution(&emulator, &machine, "only", input).await;
        ::tokio::time::sleep(PAST_THE_EXECUTION_DELAY).await;
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let events = history(&emulator, &execution, json!({})).await;
        let left = events
            .iter()
            .find(|event| {
                event.get("type").and_then(Value::as_str) == Some("ChoiceStateExited")
            })
            .unwrap_or_else(|| panic!("case {case:?}: expected a ChoiceStateExited in {events:?}"));
        let details = left
            .get("stateExitedEventDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected stateExitedEventDetails in {left:?}"));
        let actual = details.get("assignedVariables");
        assert_eq!(
            actual,
            expected.as_ref(),
            "case {case:?}: expected the variables {expected:?}, got {actual:?}"
        );
    }
}

/// A `Choice` rule's own `Output` is the state's where that rule declares one, and the
/// state's own `Output` decides it otherwise — over the variables the SELECTED rule's
/// `Assign` has just written, which is what the states after the `Choice` read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_selected_choice_rules_output_decides_the_states_output() {
    // (case, the rules, the state-level Assign, the state-level Output, the input,
    //  the expected output)
    let rows: Vec<(&str, Value, Value, Value, &str, Value)> = vec![
        (
            "the selected rule declares an Output of its own",
            json!([{
                "Condition": "{% $states.input.n > 0 %}",
                "Assign": { "seen": "{% $states.input.n %}" },
                "Output": { "from": "rule", "seen": "{% $seen %}" },
                "Next": "Done",
            }]),
            Value::Null,
            json!({ "from": "state" }),
            r#"{"n":3}"#,
            json!({ "from": "rule", "seen": 3 }),
        ),
        (
            "the selected rule declares none, so the state's own Output decides",
            json!([{
                "Condition": "{% $states.input.n > 0 %}",
                "Assign": { "seen": "{% $states.input.n * 2 %}" },
                "Next": "Done",
            }]),
            Value::Null,
            json!({ "from": "state", "seen": "{% $seen %}" }),
            r#"{"n":3}"#,
            json!({ "from": "state", "seen": 6 }),
        ),
        (
            "neither declares one, so the state's own input goes through",
            json!([{ "Condition": "{% $states.input.n > 0 %}", "Next": "Done" }]),
            Value::Null,
            Value::Null,
            r#"{"n":3}"#,
            json!({ "n": 3 }),
        ),
        (
            // The recorded shape: two rules whose conditions BOTH hold, each
            // declaring the same variable as the state does. What a state AFTER the
            // `Choice` reads must be the first matching rule's value — not the second
            // rule's and not the state-level one.
            "both rules match and a later state reads the FIRST rule's variable",
            json!([
                {
                    "Condition": "{% $states.input.n >= 10 %}",
                    "Assign": { "tag": "from-first-rule" },
                    "Next": "Done",
                },
                {
                    "Condition": "{% $states.input.n >= 5 %}",
                    "Assign": { "tag": "from-second-rule" },
                    "Next": "Done",
                },
            ]),
            json!({ "tag": "from-state-level" }),
            json!({ "tag": "{% $tag %}" }),
            r#"{"n":50}"#,
            json!({ "tag": "from-first-rule" }),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, choices, state_assign, state_output, input, expected)) in
        rows.into_iter().enumerate()
    {
        let mut pick = json!({ "Type": "Choice", "Choices": choices, "Default": "Done" });
        if !state_assign.is_null() {
            pick["Assign"] = state_assign;
        }
        if !state_output.is_null() {
            pick["Output"] = state_output;
        }
        let definition = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "Pick",
            "States": {
                "Pick": pick,
                "Done": { "Type": "Pass", "End": true }
            }
        });
        let described =
            ran(&emulator, &format!("choice-output-{index}"), &definition, input).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let actual = output(&described);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected}, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// A comparator that cannot be applied to the data
// ---------------------------------------------------------------------------

/// A value comparator whose two operands are not of the TYPE it compares does not
/// match: the `And`/`Or`/`Not` around it evaluates normally and the next rule or the
/// `Default` is taken, so the execution succeeds.
///
/// The rows differ along WHICH comparator family cannot be applied, whether the
/// mismatch is on the rule's own variable or on the document the `*Path` operand
/// selects, and whether the non-match is read through a nested `And`, `Or` or `Not` —
/// the dimensions the rule generalises over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_comparator_the_data_does_not_fit_does_not_match() {
    // (case, the first rule's condition, the input, the state the run must reach)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            // The recorded shape: a numeric comparator against a string-valued path.
            "NumericEqualsPath against a string-valued path",
            json!({ "Variable": "$.n", "NumericEqualsPath": "$.word", "Next": "Matched" }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
        (
            "NumericGreaterThan against a string-valued variable",
            json!({ "Variable": "$.word", "NumericGreaterThan": 1, "Next": "Matched" }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
        (
            "StringEquals against a number-valued variable",
            json!({ "Variable": "$.n", "StringEquals": "3", "Next": "Matched" }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
        (
            "StringMatches against a number-valued variable",
            json!({ "Variable": "$.n", "StringMatches": "3*", "Next": "Matched" }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
        (
            "BooleanEquals against a string-valued variable",
            json!({ "Variable": "$.word", "BooleanEquals": true, "Next": "Matched" }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
        (
            "TimestampLessThan against a string that is no timestamp",
            json!({
                "Variable": "$.word",
                "TimestampLessThan": "2026-01-01T00:00:00Z",
                "Next": "Matched",
            }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
        (
            "both arms of an Or are comparators the data does not fit",
            json!({
                "Or": [
                    { "Variable": "$.n", "NumericEqualsPath": "$.word" },
                    { "Variable": "$.word", "NumericEquals": 3 },
                ],
                "Next": "Matched",
            }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
        (
            "one arm of an And is a comparator the data does not fit",
            json!({
                "And": [
                    { "Variable": "$.n", "NumericEquals": 3 },
                    { "Variable": "$.n", "StringEquals": "3" },
                ],
                "Next": "Matched",
            }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
        (
            // A `Not` over a non-match HOLDS, so this rule is the one that matches:
            // the non-match is a value the tree goes on computing with.
            "a Not over a comparator the data does not fit holds",
            json!({
                "Not": { "Variable": "$.n", "StringEquals": "3" },
                "Next": "Matched",
            }),
            r#"{"n":3,"word":"zeta"}"#,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, rule, input)) in rows.into_iter().enumerate() {
        // The `Not` row is the one rule that MATCHES; every other row's rule does not,
        // so the `Default` is taken. Either way the execution succeeds and the output
        // says which state ran.
        let matches = case.starts_with("a Not over");
        let definition = json!({
            "StartAt": "Pick",
            "States": {
                "Pick": { "Type": "Choice", "Choices": [rule], "Default": "Fell" },
                "Matched": { "Type": "Pass", "Result": { "took": "rule" }, "End": true },
                "Fell": { "Type": "Pass", "Result": { "took": "default" }, "End": true }
            }
        });
        let described =
            ran(&emulator, &format!("choice-type-{index}"), &definition, input).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the non-match to route rather than fail, got {described:?}"
        );
        let expected = json!({ "took": if matches { "rule" } else { "default" } });
        let actual = output(&described);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected}, got {actual}"
        );
    }
}

/// A STRUCTURAL defect of a choice rule still fails the execution: the rule says
/// nothing that can be decided about any data, so it is not a non-match.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_structurally_broken_choice_rule_still_fails_the_execution() {
    // (case, the rule, the input)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a *Path comparator whose operand is not a path string",
            json!({ "Variable": "$.n", "NumericEqualsPath": 3, "Next": "Matched" }),
            r#"{"n":3}"#,
        ),
        (
            "a comparator name the language does not define",
            json!({ "Variable": "$.n", "NumericBetween": 3, "Next": "Matched" }),
            r#"{"n":3}"#,
        ),
        (
            "an ordering the language does not define over booleans",
            json!({ "Variable": "$.flag", "BooleanLessThan": true, "Next": "Matched" }),
            r#"{"n":3,"flag":true}"#,
        ),
        (
            "a comparator the language does not define at all",
            json!({ "Variable": "$.n", "Approximately": 3, "Next": "Matched" }),
            r#"{"n":3}"#,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, rule, input)) in rows.into_iter().enumerate() {
        let definition = json!({
            "StartAt": "Pick",
            "States": {
                "Pick": { "Type": "Choice", "Choices": [rule], "Default": "Fell" },
                "Matched": { "Type": "Pass", "End": true },
                "Fell": { "Type": "Pass", "End": true }
            }
        });
        let described =
            ran(&emulator, &format!("choice-broken-{index}"), &definition, input).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the defect to fail the execution, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some("States.Runtime"),
            "case {case:?}: expected States.Runtime, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A number keeps the digits it was written with
// ---------------------------------------------------------------------------

/// A JSON number a state carries through comes back with the digits it was written
/// with, whether the caller wrote them in the execution input or the definition wrote
/// them in a state's own literal.
///
/// The rows differ along WHERE the number was written and how far past a binary
/// double's 53 bits of precision it is: 2^53 itself, which a double still holds
/// exactly, the first integer past it, and a 30-digit integer no double holds at all.
/// The definitions are written as TEXT, because a number put through a JSON builder
/// here would be rounded before the service ever saw it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_number_keeps_the_digits_it_was_written_with() {
    /// The published bound on the integers a binary double holds exactly (2^53).
    const EXACT_TO: &str = "9007199254740992";
    /// The first integer past it, which a double rounds to the one below.
    const PAST_IT: &str = "9007199254740993";
    /// Far past it: the digits an id, an amount or a counter can carry.
    const THIRTY_DIGITS: &str = "123456789012345678901234567890";

    /// A one-`Pass` definition whose state carries `members`.
    fn passing(members: &str) -> String {
        format!(r#"{{"StartAt":"Carry","States":{{"Carry":{{"Type":"Pass",{members}}}}}}}"#)
    }

    // (case, the definition text, the execution input, the output text the run reports)
    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "a 30-digit integer written in a Pass state's Result",
            passing(&format!(r#""Result":{{"bigInt":{THIRTY_DIGITS}}},"End":true"#)),
            "{}".to_string(),
            format!(r#"{{"bigInt":{THIRTY_DIGITS}}}"#),
        ),
        (
            "the integer a double is still exact to",
            passing(&format!(r#""Result":{{"bigInt":{EXACT_TO}}},"End":true"#)),
            "{}".to_string(),
            format!(r#"{{"bigInt":{EXACT_TO}}}"#),
        ),
        (
            "the first integer past that bound",
            passing(&format!(r#""Result":{{"bigInt":{PAST_IT}}},"End":true"#)),
            "{}".to_string(),
            format!(r#"{{"bigInt":{PAST_IT}}}"#),
        ),
        (
            "a 30-digit integer the CALLER wrote in the execution input",
            passing(r#""End":true"#),
            format!(r#"{{"bigInt":{THIRTY_DIGITS}}}"#),
            format!(r#"{{"bigInt":{THIRTY_DIGITS}}}"#),
        ),
        (
            "two such integers inside an array a Result writes",
            passing(&format!(
                r#""Result":{{"ids":[{THIRTY_DIGITS},{PAST_IT}]}},"End":true"#
            )),
            "{}".to_string(),
            format!(r#"{{"ids":[{THIRTY_DIGITS},{PAST_IT}]}}"#),
        ),
        (
            "a 30-digit integer a Result writes and OutputPath selects out of",
            passing(&format!(
                r#""Result":{{"wrapped":{{"bigInt":{THIRTY_DIGITS}}}}},"OutputPath":"$.wrapped","End":true"#
            )),
            "{}".to_string(),
            format!(r#"{{"bigInt":{THIRTY_DIGITS}}}"#),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, definition, input, expected)) in rows.into_iter().enumerate() {
        let machine = create_machine(&emulator, &format!("digits-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", &input).await;
        ::tokio::time::sleep(PAST_THE_EXECUTION_DELAY).await;
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let actual = described
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A Catch whose Output reads $states.errorOutput
// ---------------------------------------------------------------------------

/// A `Catch` inside a state machine a `Task` STARTED recovers that execution too: the
/// catcher is read in the child's own walk, so a child whose failure its own catcher
/// takes over SUCCEEDS, and the parent's run-a-job task succeeds with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_catch_in_a_child_state_machine_recovers_that_execution() {
    use std::time::{Duration, Instant};

    use crate::integration_failure_error_names::Emulator as WithTargets;

    /// The name the mounted DynamoDB service's refusal is reported under.
    const REFUSED: &str = "DynamoDB.ResourceNotFoundException";

    let emulator = WithTargets::new();
    let child = json!({
        "QueryLanguage": "JSONata",
        "StartAt": "Write",
        "States": {
            "Write": {
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "recovered": true, "error": "{% $states.errorOutput.Error %}" },
                    "Next": "Recovered",
                }],
                "End": true,
            },
            "Recovered": { "Type": "Pass", "End": true }
        }
    })
    .to_string();
    let created = emulator
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": "catch-child",
                "definition": child,
                "roleArn": crate::state_machine_lifecycle::ROLE_ARN,
                "type": "STANDARD",
            }),
        )
        .await;
    let child_arn = created["stateMachineArn"]
        .as_str()
        .expect("the child state machine was created")
        .to_string();

    // The rows differ along the pattern the parent's `Resource` asks for: the
    // fire-and-forget call, and both revisions of the run-a-job pattern, which WAIT
    // for the child and take its outcome as the task's own.
    let rows: Vec<(&str, &str)> = vec![
        ("the plain integration, which does not wait", ""),
        ("the run-a-job pattern", ".sync"),
        ("its second revision, which parses the child's output", ".sync:2"),
    ];

    for (index, (case, suffix)) in rows.into_iter().enumerate() {
        let parent = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "RunChild",
            "States": {
                "RunChild": {
                    "Type": "Task",
                    "Resource": format!("arn:aws:states:::states:startExecution{suffix}"),
                    "Arguments": { "StateMachineArn": &child_arn, "Input": {} },
                    "End": true,
                }
            }
        })
        .to_string();
        let created = emulator
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("catch-parent-{index}"),
                    "definition": parent,
                    "roleArn": crate::state_machine_lifecycle::ROLE_ARN,
                    "type": "STANDARD",
                }),
            )
            .await;
        let machine = created["stateMachineArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no stateMachineArn in {created}"));
        let started = emulator
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
            )
            .await;
        let execution = started["executionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no executionArn in {started}"))
            .to_string();

        // A run-a-job parent is parked at the child until it ends, so the parent's own
        // outcome arrives a few poll intervals after the child's.
        let began = Instant::now();
        let described = loop {
            let described = emulator
                .sfn("DescribeExecution", &json!({ "executionArn": execution }))
                .await;
            if described["status"].as_str() != Some("RUNNING") {
                break described;
            }
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "case {case:?}: expected a terminal status, got {described}"
            );
            ::tokio::time::sleep(Duration::from_millis(25)).await;
        };
        assert_eq!(
            described["status"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: expected the child's own catcher to recover it, got {described}"
        );

        // And the CHILD execution the task started really recovered: its own
        // `DescribeExecution` says so, with the document its catcher built.
        let children = emulator
            .sfn("ListExecutions", &json!({ "stateMachineArn": &child_arn }))
            .await;
        let child_execution = children["executions"]
            .as_array()
            .and_then(|executions| executions.first())
            .and_then(|execution| execution["executionArn"].as_str())
            .unwrap_or_else(|| panic!("case {case:?}: expected a child execution in {children}"))
            .to_string();
        let child_described = emulator
            .sfn(
                "DescribeExecution",
                &json!({ "executionArn": child_execution }),
            )
            .await;
        assert_eq!(
            child_described["status"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: expected the child execution to have recovered, got {child_described}"
        );
        assert_eq!(
            output(&child_described),
            json!({ "recovered": true, "error": REFUSED }),
            "case {case:?}: expected the catcher's own document, got {child_described}"
        );
    }
}

/// The PARENT's `Catch` recovers a run-a-job task whose child execution failed: the
/// child's failure is the task's own, so the catcher reading `$states.errorOutput`
/// takes it over and the parent carries on at the catcher's `Next`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_catch_on_a_run_a_job_task_recovers_a_failed_child() {
    use std::time::{Duration, Instant};

    use crate::integration_failure_error_names::Emulator as WithTargets;

    let emulator = WithTargets::new();
    // The child carries NO catcher of its own, so its failure is what the parent's
    // task has to answer for.
    let child = json!({
        "QueryLanguage": "JSONata",
        "StartAt": "Write",
        "States": {
            "Write": {
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "End": true,
            }
        }
    })
    .to_string();
    let created = emulator
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": "uncaught-child",
                "definition": child,
                "roleArn": crate::state_machine_lifecycle::ROLE_ARN,
                "type": "STANDARD",
            }),
        )
        .await;
    let child_arn = created["stateMachineArn"]
        .as_str()
        .expect("the child state machine was created")
        .to_string();

    // The rows differ along how the catcher names the errors it takes over, which for
    // a run-a-job task is the failure of the work the job did.
    let rows: Vec<(&str, Value)> = vec![
        ("a catcher naming every error", json!(["States.ALL"])),
        (
            "a catcher naming the failure of a task's own work",
            json!(["States.TaskFailed"]),
        ),
    ];

    for (index, (case, error_equals)) in rows.into_iter().enumerate() {
        let parent = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "RunChild",
            "States": {
                "RunChild": {
                    "Type": "Task",
                    "Resource": "arn:aws:states:::states:startExecution.sync:2",
                    "Arguments": { "StateMachineArn": &child_arn, "Input": {} },
                    "Catch": [{
                        "ErrorEquals": error_equals,
                        "Output": {
                            "recovered": true,
                            "error": "{% $states.errorOutput.Error %}",
                            "cause": "{% $states.errorOutput.Cause %}",
                        },
                        "Next": "Recovered",
                    }],
                    "End": true,
                },
                "Recovered": { "Type": "Pass", "End": true }
            }
        })
        .to_string();
        let created = emulator
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("uncaught-parent-{index}"),
                    "definition": parent,
                    "roleArn": crate::state_machine_lifecycle::ROLE_ARN,
                    "type": "STANDARD",
                }),
            )
            .await;
        let machine = created["stateMachineArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no stateMachineArn in {created}"));
        let started = emulator
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
            )
            .await;
        let execution = started["executionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no executionArn in {started}"))
            .to_string();

        let began = Instant::now();
        let described = loop {
            let described = emulator
                .sfn("DescribeExecution", &json!({ "executionArn": execution }))
                .await;
            if described["status"].as_str() != Some("RUNNING") {
                break described;
            }
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "case {case:?}: expected a terminal status, got {described}"
            );
            ::tokio::time::sleep(Duration::from_millis(25)).await;
        };
        assert_eq!(
            described["status"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: expected the parent's catcher to recover it, got {described}"
        );
        let recovered = output(&described);
        assert_eq!(
            recovered.get("recovered"),
            Some(&json!(true)),
            "case {case:?}: expected the catcher's own document, got {recovered}"
        );
        assert!(
            recovered
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|error| !error.is_empty()),
            "case {case:?}: expected the error output to name the failure, got {recovered}"
        );
    }
}
///
/// The rows differ along what the catcher carries and how it names the errors it takes
/// over, so the rule is pinned over the members a JSONata catcher may declare rather
/// than over one of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_catch_reading_the_error_output_recovers_the_execution() {
    use crate::integration_failure_error_names::Emulator as WithTargets;

    /// The name the mounted DynamoDB service's refusal of a write to a table nobody
    /// created is reported under.
    const REFUSED: &str = "DynamoDB.ResourceNotFoundException";

    // (case, the Task state, the state that recovers, what the run must put out)
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "a catcher whose Output reads the error output",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "recovered": true, "error": "{% $states.errorOutput.Error %}" },
                    "Next": "Recovered",
                }],
                "End": true,
            }),
            json!({ "Type": "Pass", "End": true }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            "a catcher whose whole Output IS the error output",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "Catch": [{
                    "ErrorEquals": ["States.TaskFailed"],
                    "Output": "{% $states.errorOutput %}",
                    "Next": "Recovered",
                }],
                "End": true,
            }),
            json!({
                "Type": "Pass",
                "Output": { "recovered": true, "error": "{% $states.input.Error %}" },
                "End": true,
            }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            "a catcher naming the service's own error, whose Assign carries it on",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "Catch": [{
                    "ErrorEquals": [REFUSED],
                    "Assign": { "why": "{% $states.errorOutput.Error %}" },
                    "Output": { "recovered": true },
                    "Next": "Recovered",
                }],
                "End": true,
            }),
            json!({
                "Type": "Pass",
                "Output": { "recovered": true, "error": "{% $why %}" },
                "End": true,
            }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            "a catcher that declares neither, which still recovers the execution",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "Catch": [{ "ErrorEquals": ["States.ALL"], "Next": "Recovered" }],
                "End": true,
            }),
            json!({
                "Type": "Pass",
                "Output": { "recovered": true, "error": "{% $states.input.Error %}" },
                "End": true,
            }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            // The catcher takes over only once the `Retry` is spent, so this row
            // pins that the two together still reach the catcher.
            "a catcher behind a Retry the failure has spent",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "Retry": [{ "ErrorEquals": ["States.ALL"], "MaxAttempts": 1, "IntervalSeconds": 0 }],
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "recovered": true, "error": "{% $states.errorOutput.Error %}" },
                    "Next": "Recovered",
                }],
                "End": true,
            }),
            json!({ "Type": "Pass", "End": true }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            // A state-level `Output` belongs to the SUCCESS path, so the catcher's own
            // is what a caught failure puts out.
            "a catcher beside a state-level Output the failure does not take",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "Assign": { "why": "never" },
                "Output": { "recovered": false },
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "recovered": true, "error": "{% $states.errorOutput.Error %}" },
                    "Next": "Recovered",
                }],
                "End": true,
            }),
            json!({ "Type": "Pass", "End": true }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            // The call the AWS SDK integration makes, which names the same work
            // through the other `Resource` spelling.
            "the same call made through the aws-sdk integration",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:dynamodb:putItem",
                "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "recovered": true, "error": "{% $states.errorOutput.Error %}" },
                    "Next": "Recovered",
                }],
                "End": true,
            }),
            json!({ "Type": "Pass", "End": true }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            // The failure is one level IN: the iteration's task fails, the `Map` state
            // fails with it, and the Map's own catcher takes it over.
            "a Map whose iteration failed, caught on the Map state",
            json!({
                "Type": "Map",
                "Items": [1],
                "ItemProcessor": {
                    "StartAt": "Write",
                    "States": {
                        "Write": {
                            "Type": "Task",
                            "Resource": "arn:aws:states:::dynamodb:putItem",
                            "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                            "End": true,
                        }
                    }
                },
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "recovered": true, "error": "{% $states.errorOutput.Error %}" },
                    "Next": "Recovered",
                }],
                "End": true,
            }),
            json!({ "Type": "Pass", "End": true }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            "a Parallel whose branch failed, caught on the Parallel state",
            json!({
                "Type": "Parallel",
                "Branches": [{
                    "StartAt": "Write",
                    "States": {
                        "Write": {
                            "Type": "Task",
                            "Resource": "arn:aws:states:::dynamodb:putItem",
                            "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                            "End": true,
                        }
                    }
                }],
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": { "recovered": true, "error": "{% $states.errorOutput.Error %}" },
                    "Next": "Recovered",
                }],
                "End": true,
            }),
            json!({ "Type": "Pass", "End": true }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            // Both members of the error output, read the way the recorded definitions
            // of this family write them.
            "a catcher reading both members of the error output",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:dynamodb:getItem",
                "Arguments": { "TableName": "nowhere", "Key": { "pk": { "S": "a" } } },
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Output": {
                        "recovered": true,
                        "error": "{% $states.errorOutput.Error %}",
                        "cause": "{% $states.errorOutput.Cause %}",
                    },
                    "Next": "Recovered",
                }],
                "Next": "Recovered",
            }),
            json!({
                "Type": "Pass",
                "Output": { "recovered": true, "error": "{% $states.input.error %}" },
                "End": true,
            }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            // The catcher is one level IN, on the task inside a `Parallel` BRANCH: the
            // branch recovers, so the `Parallel` around it never fails.
            "a catcher on the task inside a Parallel branch",
            json!({
                "Type": "Parallel",
                "Branches": [{
                    "StartAt": "Write",
                    "States": {
                        "Write": {
                            "Type": "Task",
                            "Resource": "arn:aws:states:::dynamodb:putItem",
                            "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                            "Catch": [{
                                "ErrorEquals": ["States.ALL"],
                                "Output": { "error": "{% $states.errorOutput.Error %}" },
                                "Next": "Inner",
                            }],
                            "End": true,
                        },
                        "Inner": { "Type": "Pass", "End": true }
                    }
                }],
                "Output": { "recovered": true, "error": "{% $states.result[0].error %}" },
                "Next": "Recovered",
            }),
            json!({ "Type": "Pass", "End": true }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
        (
            // The same, on the task inside a `Map` ITERATION.
            "a catcher on the task inside a Map iteration",
            json!({
                "Type": "Map",
                "Items": [1],
                "ItemProcessor": {
                    "StartAt": "Write",
                    "States": {
                        "Write": {
                            "Type": "Task",
                            "Resource": "arn:aws:states:::dynamodb:putItem",
                            "Arguments": { "TableName": "nowhere", "Item": { "pk": { "S": "a" } } },
                            "Catch": [{
                                "ErrorEquals": ["States.ALL"],
                                "Output": { "error": "{% $states.errorOutput.Error %}" },
                                "Next": "Inner",
                            }],
                            "End": true,
                        },
                        "Inner": { "Type": "Pass", "End": true }
                    }
                },
                "Output": { "recovered": true, "error": "{% $states.result[0].error %}" },
                "Next": "Recovered",
            }),
            json!({ "Type": "Pass", "End": true }),
            json!({ "recovered": true, "error": REFUSED }),
        ),
    ];
    let emulator = WithTargets::new();
    for (index, (case, call, recovered, expected)) in rows.into_iter().enumerate() {
        let definition = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "Call",
            "States": { "Call": call, "Recovered": recovered }
        })
        .to_string();
        let created = emulator
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("catch-error-output-{index}"),
                    "definition": definition,
                    "roleArn": crate::state_machine_lifecycle::ROLE_ARN,
                    "type": "STANDARD",
                }),
            )
            .await;
        let machine = created["stateMachineArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no stateMachineArn in {created}"));
        let started = emulator
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
            )
            .await;
        let execution = started["executionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no executionArn in {started}"))
            .to_string();
        ::tokio::time::sleep(PAST_THE_EXECUTION_DELAY).await;
        let described = emulator
            .sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        assert_eq!(
            described["status"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: expected the catcher to recover the execution, got {described}"
        );
        let actual = output(&described);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected}, got {actual}"
        );
    }
}

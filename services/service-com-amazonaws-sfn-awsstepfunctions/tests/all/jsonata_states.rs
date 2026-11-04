// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the JSONata query language, driven over this service's own
//! wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test: a state is evaluated in the query language IT declares, else
//! the one its definition declares, and both of the languages `QueryLanguage` selects
//! between are evaluated. So a state carrying `"QueryLanguage": "JSONata"` is
//! evaluated with that language's semantics WHEREVER it appears — at the top level of
//! a JSONata definition, and inside a `Parallel` branch or a `Map` processor of a
//! JSONPath one — and a machine that mixes the two runs both.
//!
//! The members that carry those expressions are the ones the language declares:
//! `Output` (what the state puts out), `Assign` (the variables it writes, which the
//! states after it read under their own names), `Arguments` (what a `Task` or a
//! `Parallel` hands its work), `Items` (the array a `Map` iterates), a `Choice` rule's
//! `Condition`, and a `Wait`'s and a `Fail`'s own members. Each expression is
//! delimited with `{%` and `%}`, and a field carrying no such delimiter is the literal
//! it is written as.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`)
//! is the one `state_machine_lifecycle` builds and the history reader is
//! `execution_history`'s; `tests/all.rs` is this binary's crate root, so each is
//! reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// What one row of the table expects of its execution.
enum Expected {
    /// The run SUCCEEDED with this output document.
    Output(&'static str),
    /// The run FAILED with this ASL error name.
    Failure(&'static str),
}

/// One row: a definition written in (or mixing in) JSONata, the input it is run over,
/// and what it must produce.
struct Row {
    case: &'static str,
    definition: String,
    input: &'static str,
    expected: Expected,
}

/// Every row's execution is started, the clock is let run once, and each row's status
/// and output are asserted.
async fn check(emulator: &Emulator, prefix: &str, rows: Vec<Row>) {
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let machine =
            create_machine(emulator, &format!("{prefix}-{index}"), &row.definition).await;
        started.push(start_execution(emulator, &machine, "only", row.input).await);
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = describe_execution(emulator, &execution).await;
        let status = described.get("status").and_then(Value::as_str);
        match row.expected {
            Expected::Output(expected) => {
                assert_eq!(
                    status,
                    Some("SUCCEEDED"),
                    "case {case:?}: expected SUCCEEDED, got {described:?}"
                );
                let expected: Value =
                    serde_json::from_str(expected).expect("the expected output is JSON");
                let actual: Value = described
                    .get("output")
                    .and_then(Value::as_str)
                    .map(|text| serde_json::from_str(text).expect("the output is JSON"))
                    .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected output {expected}, got {actual}"
                );
            }
            Expected::Failure(error) => {
                assert_eq!(
                    status,
                    Some("FAILED"),
                    "case {case:?}: expected FAILED, got {described:?}"
                );
                assert_eq!(
                    described.get("error").and_then(Value::as_str),
                    Some(error),
                    "case {case:?}: expected error {error}, got {described:?}"
                );
            }
        }
    }
}

/// A state written in JSONata is evaluated with that language's semantics, over a
/// table that spans the members the language declares.
///
/// Rows vary along WHICH member carries the expression and which state type declares
/// it — the dimensions the rule generalises over — rather than repeating one state
/// with different values.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_jsonata_state_is_evaluated_in_jsonata() {
    let rows = vec![
        Row {
            case: "a Pass state's Output builds the document",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Build",
                "States": {
                    "Build": {
                        "Type": "Pass",
                        "Output": { "doubled": "{% $states.input.n * 2 %}", "kept": "literal" },
                        "End": true,
                    }
                }
            })
            .to_string(),
            input: r#"{"n":21}"#,
            expected: Expected::Output(r#"{"doubled":42,"kept":"literal"}"#),
        },
        Row {
            case: "a Pass state with no Output carries its input through",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Through",
                "States": { "Through": { "Type": "Pass", "End": true } }
            })
            .to_string(),
            input: r#"{"n":1}"#,
            expected: Expected::Output(r#"{"n":1}"#),
        },
        Row {
            case: "an Assign writes a variable the next state reads",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Remember",
                "States": {
                    "Remember": {
                        "Type": "Pass",
                        "Assign": { "total": "{% $states.input.a + $states.input.b %}" },
                        "Next": "Report",
                    },
                    "Report": {
                        "Type": "Pass",
                        "Output": { "total": "{% $total %}" },
                        "End": true,
                    }
                }
            })
            .to_string(),
            input: r#"{"a":2,"b":5}"#,
            expected: Expected::Output(r#"{"total":7}"#),
        },
        Row {
            case: "a Choice rule's Condition decides the transition",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Pick",
                "States": {
                    "Pick": {
                        "Type": "Choice",
                        "Choices": [{ "Condition": "{% $states.input.n > 10 %}", "Next": "Big" }],
                        "Default": "Small",
                    },
                    "Big": { "Type": "Pass", "Output": { "size": "big" }, "End": true },
                    "Small": { "Type": "Pass", "Output": { "size": "small" }, "End": true }
                }
            })
            .to_string(),
            input: r#"{"n":11}"#,
            expected: Expected::Output(r#"{"size":"big"}"#),
        },
        Row {
            case: "the same Choice falls through to its Default",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Pick",
                "States": {
                    "Pick": {
                        "Type": "Choice",
                        "Choices": [{ "Condition": "{% $states.input.n > 10 %}", "Next": "Big" }],
                        "Default": "Small",
                    },
                    "Big": { "Type": "Pass", "Output": { "size": "big" }, "End": true },
                    "Small": { "Type": "Pass", "Output": { "size": "small" }, "End": true }
                }
            })
            .to_string(),
            input: r#"{"n":3}"#,
            expected: Expected::Output(r#"{"size":"small"}"#),
        },
        Row {
            case: "a Wait state's Seconds is an expression",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Hold",
                "States": {
                    "Hold": {
                        "Type": "Wait",
                        "Seconds": "{% $states.input.seconds %}",
                        "End": true,
                    }
                }
            })
            .to_string(),
            input: r#"{"seconds":0}"#,
            expected: Expected::Output(r#"{"seconds":0}"#),
        },
        Row {
            case: "a Fail state's Error and Cause are expressions",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Boom",
                "States": {
                    "Boom": {
                        "Type": "Fail",
                        "Error": "{% $states.input.name %}",
                        "Cause": "{% 'the definition said ' & $states.input.name %}",
                    }
                }
            })
            .to_string(),
            input: r#"{"name":"Nope"}"#,
            expected: Expected::Failure("Nope"),
        },
        Row {
            case: "a Map state's Items is an expression and its result feeds Output",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Each",
                "States": {
                    "Each": {
                        "Type": "Map",
                        "Items": "{% $states.input.values %}",
                        "ItemProcessor": {
                            "StartAt": "Double",
                            "States": {
                                "Double": {
                                    "Type": "Pass",
                                    "Output": "{% $states.input * 2 %}",
                                    "End": true,
                                }
                            }
                        },
                        "Output": { "doubled": "{% $states.result %}" },
                        "End": true,
                    }
                }
            })
            .to_string(),
            input: r#"{"values":[1,2,3]}"#,
            expected: Expected::Output(r#"{"doubled":[2,4,6]}"#),
        },
        Row {
            case: "a Parallel state's Arguments is what each branch receives",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Fan",
                "States": {
                    "Fan": {
                        "Type": "Parallel",
                        "Arguments": { "shared": "{% $states.input.n %}" },
                        "Branches": [
                            {
                                "StartAt": "A",
                                "States": {
                                    "A": {
                                        "Type": "Pass",
                                        "Output": { "a": "{% $states.input.shared %}" },
                                        "End": true,
                                    }
                                }
                            },
                            {
                                "StartAt": "B",
                                "States": {
                                    "B": {
                                        "Type": "Pass",
                                        "Output": { "b": "{% $states.input.shared + 1 %}" },
                                        "End": true,
                                    }
                                }
                            },
                        ],
                        "End": true,
                    }
                }
            })
            .to_string(),
            input: r#"{"n":4}"#,
            expected: Expected::Output(r#"[{"a":4},{"b":5}]"#),
        },
        Row {
            case: "an expression that cannot be evaluated is the language's own error",
            definition: json!({
                "QueryLanguage": "JSONata",
                "StartAt": "Broken",
                "States": {
                    "Broken": {
                        "Type": "Pass",
                        "Output": { "oops": "{% $states.input.n + %}" },
                        "End": true,
                    }
                }
            })
            .to_string(),
            input: r#"{"n":1}"#,
            expected: Expected::Failure("States.QueryEvaluationError"),
        },
    ];

    let emulator = Emulator::new();
    check(&emulator, "jsonata", rows).await;
}

/// A JSONata state inside a JSONPATH definition is evaluated in JSONata, and the
/// JSONPath states around it in JSONPath: "mixing the two in one state machine is
/// legal", and a state's own `QueryLanguage` is what decides which one it is
/// evaluated in.
///
/// Rows vary along WHERE the JSONata state sits — at the top level, inside a
/// `Parallel` branch, inside a `Map` processor — which is the dimension the issue this
/// suite was written for turned on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_jsonata_state_inside_a_jsonpath_definition_is_evaluated() {
    let rows = vec![
        Row {
            case: "a JSONata state at the top level of a JSONPath definition",
            definition: json!({
                "StartAt": "Gate",
                "States": {
                    "Gate": {
                        "Type": "Pass",
                        "QueryLanguage": "JSONata",
                        "Output": { "gated": "{% $states.input.n > 0 %}" },
                        "Next": "Done",
                    },
                    "Done": { "Type": "Pass", "ResultPath": "$.seen", "End": true }
                }
            })
            .to_string(),
            input: r#"{"n":1}"#,
            expected: Expected::Output(r#"{"gated":true,"seen":{"gated":true}}"#),
        },
        Row {
            case: "a JSONata state inside a Parallel branch of a JSONPath definition",
            definition: json!({
                "StartAt": "Fan",
                "States": {
                    "Fan": {
                        "Type": "Parallel",
                        "Retry": [{ "ErrorEquals": ["States.ALL"], "MaxAttempts": 2 }],
                        "Branches": [
                            {
                                "StartAt": "Gate",
                                "States": {
                                    "Gate": {
                                        "Type": "Pass",
                                        "QueryLanguage": "JSONata",
                                        "Assign": { "seen": "{% $states.input.n %}" },
                                        "Output": { "gate": "{% $states.input.n * 10 %}" },
                                        "End": true,
                                    }
                                }
                            },
                            {
                                "StartAt": "Plain",
                                "States": {
                                    "Plain": {
                                        "Type": "Pass",
                                        "Parameters": { "plain.$": "$.n" },
                                        "End": true,
                                    }
                                }
                            },
                        ],
                        "End": true,
                    }
                }
            })
            .to_string(),
            input: r#"{"n":2}"#,
            expected: Expected::Output(r#"[{"gate":20},{"plain":2}]"#),
        },
        Row {
            case: "a JSONata state inside a Map processor of a JSONPath definition",
            definition: json!({
                "StartAt": "Each",
                "States": {
                    "Each": {
                        "Type": "Map",
                        "ItemsPath": "$.values",
                        "ItemProcessor": {
                            "StartAt": "Scale",
                            "States": {
                                "Scale": {
                                    "Type": "Pass",
                                    "QueryLanguage": "JSONata",
                                    "Output": "{% $states.input * 3 %}",
                                    "End": true,
                                }
                            }
                        },
                        "End": true,
                    }
                }
            })
            .to_string(),
            input: r#"{"values":[1,2]}"#,
            expected: Expected::Output("[3,6]"),
        },
    ];

    let emulator = Emulator::new();
    check(&emulator, "mixed", rows).await;
}

/// A runaway template is stopped by the ceilings one visit to a state runs under, and
/// the state fails with the ASL error name for the class it reached: a caller's
/// expressions can neither hold the thread the walk runs on nor build a document past
/// what a state may carry.
///
/// Rows vary along WHICH ceiling the template reaches — the time and recursion of one
/// expression, the size of the value one builds, how MANY expressions one visit
/// carries, and the size of what they build TOGETHER, which is the product of the
/// other two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runaway_template_is_stopped_by_the_ceilings_on_one_visit() {
    /// The ASL error name an expression that was not evaluated carries, and the one a
    /// document past what a state may carry is reported with.
    const NOT_EVALUATED: &str = "States.QueryEvaluationError";
    const TOO_LARGE: &str = "States.DataLimitExceeded";
    /// An expression that builds a 64 KiB string, so a handful of them together
    /// outgrow what one state's document may carry.
    const SIXTY_FOUR_KIB: &str =
        "{% $join($map([1..8192], function($i) { 'xxxxxxxx' }), '') %}";

    // (case, the `Output` the state declares, the error name the row must carry)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an expression that recurses without end",
            json!("{% ( $f := function($x) { $f($x + 1) }; $f(0) ) %}"),
            NOT_EVALUATED,
        ),
        (
            // The engine is asked for the whole string at once, so the arena's own
            // allocation limit declines it before the value exists — which is a
            // ceiling on the evaluation rather than on the document it would build.
            "one expression that builds a document far past the arena's own ceiling",
            json!("{% $join([1..200000].('xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'), '') %}"),
            NOT_EVALUATED,
        ),
        (
            // The value is ALIASED rather than built: it is small where the engine
            // holds it and large as a document, so what refuses it is the size
            // decided on the value itself.
            "one expression that aliases a value into a document past that ceiling",
            json!(
                "{% ( $x := [1..1000]; $y := [$x,$x,$x,$x,$x,$x,$x,$x,$x,$x]; \
                 [$y,$y,$y,$y,$y,$y,$y,$y,$y,$y] ) %}"
            ),
            TOO_LARGE,
        ),
        (
            "more expressions than one template evaluates",
            Value::Object(
                (0..100)
                    .map(|index| (format!("m{index}"), json!("{% 1 %}")))
                    .collect(),
            ),
            NOT_EVALUATED,
        ),
        (
            "members that together build more than a state's document may carry",
            Value::Object(
                (0..8)
                    .map(|index| (format!("m{index}"), json!(SIXTY_FOUR_KIB)))
                    .collect(),
            ),
            TOO_LARGE,
        ),
        (
            // An expression whose single step this emulator cannot bound is refused
            // before it is evaluated: a regular-expression literal, `$distinct` and
            // `$pad` are the three.
            "an expression carrying a regular-expression literal",
            json!("{% $match('xxx', /x+/) %}"),
            NOT_EVALUATED,
        ),
        (
            "an expression carrying $distinct",
            json!("{% $distinct([1,2,2,3]) %}"),
            NOT_EVALUATED,
        ),
        (
            "an expression carrying $pad",
            json!("{% $pad('x', 16) %}"),
            NOT_EVALUATED,
        ),
        (
            // The text itself is bounded before the parse, which is recursive over it.
            "an expression longer than the text this emulator evaluates",
            json!(format!("{{% {} %}}", "1 + ".repeat(400) + "1")),
            NOT_EVALUATED,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, output, expected)) in rows.into_iter().enumerate() {
        let definition = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "Runaway",
            "States": {
                "Runaway": { "Type": "Pass", "Output": output, "End": true }
            }
        })
        .to_string();
        let machine = create_machine(&emulator, &format!("runaway-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", "{}").await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);

        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the ceiling to fail the state, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {expected}, got {described:?}"
        );
        let cause = described
            .get("cause")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !cause.is_empty(),
            "case {case:?}: expected the failure to say what the ceiling was, got {described:?}"
        );
    }
}

/// A JSONata state's `Assign` is reported on the event the state is LEFT by: "map of
/// variable name and value as a serialized JSON representation", which is how a caller
/// reads back what a run assigned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_variables_a_state_assigned_are_reported_on_its_exit() {
    let emulator = Emulator::new();
    // The state AFTER the assigning one assigns nothing of its own, so its own exit
    // must report no variables: the member is what the state assigned, not the scope
    // the states after it can read.
    let definition = json!({
        "StartAt": "Remember",
        "States": {
            "Remember": {
                "Type": "Pass",
                "QueryLanguage": "JSONata",
                "Assign": {
                    "count": "{% $states.input.n %}",
                    "label": "{% 'n=' & $string($states.input.n) %}",
                },
                "Next": "After",
            },
            "After": { "Type": "Pass", "Result": { "done": true }, "End": true }
        }
    })
    .to_string();
    let machine = create_machine(&emulator, "assigned", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"n":3}"#).await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let case = "a Pass state that assigned two variables";
    let events = history(&emulator, &execution, json!({})).await;
    let exits: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("PassStateExited"))
        .collect();
    assert_eq!(
        exits.len(),
        2,
        "case {case:?}: expected both states to have been left, got {events:?}"
    );
    let details = exits[0]
        .get("stateExitedEventDetails")
        .unwrap_or_else(|| panic!("case {case:?}: expected stateExitedEventDetails in {:?}", exits[0]));
    assert_eq!(
        details.get("assignedVariables"),
        Some(&json!({ "count": "3", "label": "\"n=3\"" })),
        "case {case:?}: expected the variables the state assigned, got {details:?}"
    );
    assert_eq!(
        details
            .get("assignedVariablesDetails")
            .and_then(|details| details.get("truncated"))
            .and_then(Value::as_bool),
        Some(false),
        "case {case:?}: an API call never truncates them, got {details:?}"
    );

    let after = "the JSONPath state after it, which assigned nothing";
    let details = exits[1]
        .get("stateExitedEventDetails")
        .unwrap_or_else(|| panic!("case {after:?}: expected stateExitedEventDetails in {:?}", exits[1]));
    assert!(
        details.get("assignedVariables").is_none_or(Value::is_null),
        "case {after:?}: expected no variables on a state that assigned none, got {details:?}"
    );
    assert!(
        details
            .get("assignedVariablesDetails")
            .is_none_or(Value::is_null),
        "case {after:?}: expected no details either, got {details:?}"
    );
}

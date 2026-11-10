// SPDX-License-Identifier: CC0-1.0

//! What a JSON filter pattern's condition does with the TYPE of the value its `$.path`
//! selects, over every operation that runs a `filterPattern`, end to end over the wire.
//!
//! RULE: a condition of a JSON filter pattern relates the value a `$.path` selects to a
//! literal of the syntax, and the syntax's literals are NUMBERS and STRINGS — the state of a
//! boolean or null field is asked about with `IS TRUE`, `IS FALSE` and `IS NULL`, so a field
//! holding one of those states equals only the same state and never the text that renders
//! it, and a bare `true` in a condition is the string it spells. A field the event does not
//! carry, and one whose path lands on a container node rather than a value, has nothing to
//! compare: every comparison naming one is unsatisfied — `!=` as much as `=` — because what
//! asks about a field's PRESENCE is `EXISTS` / `NOT EXISTS`.
//!
//! The rows below differ along what the rule generalises over: which type the selected value
//! has (boolean, null, number, string, container, nothing at all), which predicate the
//! condition states (each `IS` state, equality, inequality, presence) and which form the
//! literal takes (bare word, quoted string, number).

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, Logs};

/// The one log group every row searches.
const GROUP: &str = "/aws/emulator/json-values";
/// Its one stream — which stream an event sits in is not what this rule is about.
const STREAM: &str = "documents";

/// The stored events, each a JSON document whose `flag` member carries a value of one type.
/// The leading `m` member names the document, so an assertion names the event it is about.
const DOCUMENTS: [&str; 7] = [
    r#"{"m":"boolean-true","flag":true}"#,
    r#"{"m":"boolean-false","flag":false}"#,
    r#"{"m":"string-true","flag":"true"}"#,
    r#"{"m":"null","flag":null}"#,
    r#"{"m":"number","flag":1}"#,
    r#"{"m":"container","flag":{"k":1}}"#,
    r#"{"m":"absent","other":1}"#,
];

/// The group, its stream and the documents above.
async fn given_stored_documents(logs: &Logs, at: i64) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": GROUP, "logStreamName": STREAM}),
    )
    .await;
    let events: Vec<(i64, &str)> = DOCUMENTS
        .iter()
        .enumerate()
        .map(|(index, message)| (at + i64::try_from(index).expect("a small offset"), *message))
        .collect();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&events),
        }),
    )
    .await;
}

/// The `m` member of one stored document, which is how a row names the events it expects.
fn named(message: &str) -> String {
    serde_json::from_str::<Value>(message)
        .ok()
        .and_then(|document| document["m"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("every stored document carries m, got {message}"))
}

/// One row: a condition, and the documents it selects by name.
struct Row {
    case: &'static str,
    pattern: &'static str,
    /// The `m` names of the documents the condition must select, in chronological order.
    selects: &'static [&'static str],
}

/// The table.
const ROWS: [Row; 11] = [
    Row {
        case: "IS TRUE, which asks about the boolean state",
        pattern: "{$.flag IS TRUE}",
        selects: &["boolean-true"],
    },
    Row {
        case: "IS FALSE, which asks about the other boolean state",
        pattern: "{$.flag IS FALSE}",
        selects: &["boolean-false"],
    },
    Row {
        case: "IS NULL, which asks about the null state",
        pattern: "{$.flag IS NULL}",
        selects: &["null"],
    },
    Row {
        case: "a bare 'true' in a comparison, which is the string it spells",
        pattern: "{$.flag = true}",
        selects: &["string-true"],
    },
    Row {
        case: "the same string, quoted",
        pattern: r#"{$.flag = "true"}"#,
        selects: &["string-true"],
    },
    Row {
        case: "a numeric literal, which the number carries",
        pattern: "{$.flag = 1}",
        selects: &["number"],
    },
    Row {
        case: "an equality a container cannot satisfy",
        pattern: "{$.flag = 1 || $.flag = \"k\"}",
        selects: &["number"],
    },
    Row {
        case: "an inequality, which the fields that carry no value cannot satisfy",
        pattern: r#"{$.flag != "true"}"#,
        selects: &["boolean-true", "boolean-false", "null", "number"],
    },
    Row {
        case: "an inequality against a number, over a container and a missing member",
        pattern: "{$.flag != 1}",
        selects: &["boolean-true", "boolean-false", "string-true", "null"],
    },
    Row {
        case: "NOT EXISTS, which is what asks about presence",
        pattern: "{$.flag NOT EXISTS}",
        selects: &["absent"],
    },
    Row {
        case: "a selector of a member of the container, which only it carries",
        pattern: "{$.flag.k = 1}",
        selects: &["container"],
    },
];

/// The `message` values a `FilterLogEvents` answer reports, in the order it reported them.
fn searched(case: &str, answer: &Value) -> Vec<String> {
    answer["events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a search answers events, got {answer}"))
        .iter()
        .map(|event| {
            event["message"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an event carries a message, got {event}"))
                .to_string()
        })
        .collect()
}

/// The `message` values a `TestMetricFilter` answer reports as matched.
fn tested(case: &str, answer: &Value) -> Vec<String> {
    answer["matches"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a test answers matches, got {answer}"))
        .iter()
        .map(|entry| {
            entry["eventMessage"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a match carries eventMessage, got {entry}")
                })
                .to_string()
        })
        .collect()
}

/// Every row selects exactly the documents whose value type and state its condition admits,
/// through `FilterLogEvents` over the stored events and through `TestMetricFilter` over the
/// same messages as a sample — one syntax, read the same way wherever it runs.
#[tokio::test]
async fn a_condition_relates_the_value_type_the_selector_lands_on() {
    let logs = Logs::new();
    given_stored_documents(&logs, base_ms()).await;

    for row in &ROWS {
        let case = row.case;
        let expected: Vec<String> = row.selects.iter().map(|name| (*name).to_string()).collect();

        let answer = logs
            .ok(
                "FilterLogEvents",
                &json!({"logGroupName": GROUP, "filterPattern": row.pattern}),
            )
            .await;
        let found: Vec<String> = searched(case, &answer).iter().map(|m| named(m)).collect();
        assert_eq!(
            found, expected,
            "case {case:?} with pattern {:?}: FilterLogEvents must select {expected:?}, got \
             {found:?}",
            row.pattern
        );

        let answer = logs
            .ok(
                "TestMetricFilter",
                &json!({"filterPattern": row.pattern, "logEventMessages": DOCUMENTS}),
            )
            .await;
        let matched: Vec<String> = tested(case, &answer).iter().map(|m| named(m)).collect();
        assert_eq!(
            matched, expected,
            "case {case:?} with pattern {:?}: TestMetricFilter must match the same documents \
             {expected:?}, got {matched:?}",
            row.pattern
        );
    }
}

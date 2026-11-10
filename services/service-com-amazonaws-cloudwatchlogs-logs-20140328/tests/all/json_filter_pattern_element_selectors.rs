// SPDX-License-Identifier: CC0-1.0

//! A JSON filter pattern that selects an ARRAY ELEMENT, over every operation that runs a
//! `filterPattern`, end to end over the wire.
//!
//! RULE: a `$.path` selector of a JSON filter pattern may index into an array member — a
//! decimal index names the element at that position and `[*]` names every element of the
//! array, so a condition on the latter holds for an event when ONE of the selected elements
//! satisfies it — and every operation that runs a filter pattern reads the selector the same
//! way. A pattern the service cannot read is refused 400 `InvalidParameterException` with
//! the fixed text for that class, with nothing of the pattern appended to it.
//!
//! The rows below differ along what the rule generalises over: which element selector the
//! path carries (a position, every element, several applied in a row), where in the path it
//! sits (at the end, before further member names), what the selected element holds (a
//! string, a number, an object, nothing at all), which comparison the condition states
//! (equality, ordering, presence) and whether the selector sits inside a `||` alternation.
//!
//! `FilterLogEvents` and `TestMetricFilter` are driven with the SAME patterns over the SAME
//! messages, because the syntax is the service's and not one operation's.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, Logs};

/// The one log group every row searches.
const GROUP: &str = "/aws/emulator/json-elements";
/// Its one stream — which stream an event sits in is not what this rule is about.
const STREAM: &str = "documents";

/// The stored events, each a JSON document, in the chronological order they are written in.
/// The leading `m` member names the document, so an assertion names the event it is about.
const DOCUMENTS: [&str; 7] = [
    r#"{"m":"strings-red-first","arr":["red","blue"],"n":1}"#,
    r#"{"m":"strings-blue-first","arr":["blue","red"],"n":4}"#,
    r#"{"m":"objects","arr":[{"c":"green"},{"c":"grey"}],"n":2}"#,
    r#"{"m":"empty-array","arr":[],"n":3}"#,
    r#"{"m":"no-array","n":5}"#,
    r#"{"m":"numbers","vals":[1,5,9]}"#,
    r#"{"m":"matrix","matrix":[["a","b"],["c"]]}"#,
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

/// One row: a pattern, and the documents it selects by name.
struct Row {
    case: &'static str,
    pattern: &'static str,
    /// The `m` names of the documents the pattern must select, in chronological order.
    selects: &'static [&'static str],
}

/// The selection table.
const ROWS: [Row; 13] = [
    Row {
        case: "the first element of an array, by position",
        pattern: r#"{$.arr[0] = "red"}"#,
        selects: &["strings-red-first"],
    },
    Row {
        case: "the second element of an array, by position",
        pattern: r#"{$.arr[1] = "red"}"#,
        selects: &["strings-blue-first"],
    },
    Row {
        case: "a position the array does not reach",
        pattern: r#"{$.arr[7] = "red"}"#,
        selects: &[],
    },
    Row {
        case: "every element of an array, which holds for either position",
        pattern: r#"{$.arr[*] = "red"}"#,
        selects: &["strings-red-first", "strings-blue-first"],
    },
    Row {
        case: "every element of an array, against a value no element carries",
        pattern: r#"{$.arr[*] = "green"}"#,
        selects: &[],
    },
    Row {
        case: "a member of the object at one position",
        pattern: r#"{$.arr[0].c = "green"}"#,
        selects: &["objects"],
    },
    Row {
        case: "a member of the object at every position",
        pattern: r#"{$.arr[*].c = "grey"}"#,
        selects: &["objects"],
    },
    Row {
        case: "two positions applied in a row, into a nested array",
        pattern: r#"{$.matrix[1][0] = "c"}"#,
        selects: &["matrix"],
    },
    Row {
        case: "every element of every element, into a nested array",
        pattern: r#"{$.matrix[*][*] = "b"}"#,
        selects: &["matrix"],
    },
    Row {
        case: "an ordering comparison against every element",
        pattern: "{$.vals[*] > 7}",
        selects: &["numbers"],
    },
    Row {
        case: "an equality comparison against one numeric element",
        pattern: "{$.vals[1] = 5}",
        selects: &["numbers"],
    },
    Row {
        case: "an element selector inside a '||' alternation, each branch selecting one",
        pattern: r#"{$.arr[0] = "red" || $.n = 4}"#,
        selects: &["strings-red-first", "strings-blue-first"],
    },
    Row {
        case: "no element selected at all, which is the absent field it is",
        pattern: "{$.arr[*] NOT EXISTS}",
        selects: &["empty-array", "no-array", "numbers", "matrix"],
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

/// Every row selects exactly the documents its element selector admits, through
/// `FilterLogEvents` over the stored events and through `TestMetricFilter` over the same
/// messages as a sample.
#[tokio::test]
async fn an_element_selector_selects_the_events_its_indexed_element_matches() {
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

/// The fixed text the service refuses a `filterPattern` it cannot read with, per the kind of
/// pattern the member carries. Source: the recorded refusals — a log-event filter pattern is
/// refused as `Invalid filter pattern`, and a metric filter's pattern (which is what
/// `TestMetricFilter` tests and `PutMetricFilter` configures) as `Invalid metric filter
/// pattern`; neither carries anything of the pattern.
const UNREADABLE_PATTERN: [(&str, &str); 3] = [
    ("FilterLogEvents", "Invalid filter pattern"),
    ("TestMetricFilter", "Invalid metric filter pattern"),
    ("PutMetricFilter", "Invalid metric filter pattern"),
];

/// Patterns the syntax does not describe, each a different defect: a bare existence
/// selector, a bracket group that is neither a position nor `[*]`, a bracket group that is
/// never closed, an element selector applied to no member, a condition that relates two
/// literals, and a condition the reader cannot get through.
const UNREADABLE: [&str; 6] = [
    "{$.flag EXISTS}",
    "{$.arr[x] = 1}",
    "{$.arr[0 = 1}",
    r#"{$.[0] = "red"}"#,
    "{1 = 1}",
    r#"{$.arr[*] = "red" ||}"#,
];

/// A pattern the service cannot read is refused with the fixed text for that class — with no
/// detail about what could not be read appended to it — by every operation that RUNS a
/// filter pattern and by the write that CONFIGURES one, and the refusal is the modelled
/// client error at HTTP 400.
#[tokio::test]
async fn an_unreadable_pattern_is_refused_with_the_fixed_text_wherever_a_pattern_is_read() {
    let logs = Logs::new();
    given_stored_documents(&logs, base_ms()).await;

    for pattern in UNREADABLE {
        for (operation, expected) in UNREADABLE_PATTERN {
            let body = match operation {
                "FilterLogEvents" => json!({"logGroupName": GROUP, "filterPattern": pattern}),
                "TestMetricFilter" => {
                    json!({"filterPattern": pattern, "logEventMessages": DOCUMENTS})
                }
                _ => json!({
                    "logGroupName": GROUP,
                    "filterName": "unreadable",
                    "filterPattern": pattern,
                    "metricTransformations": [{
                        "metricName": "Documents",
                        "metricNamespace": "Emulator/Logs",
                        "metricValue": "1",
                    }],
                }),
            };
            let (status, said) = logs.call(operation, &body).await;
            assert_eq!(
                status, 400,
                "case {pattern:?} on {operation}: expected HTTP 400, got {status} with {said}"
            );
            let (code, message) = logs.refusal(operation, &body).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {pattern:?} on {operation}: expected InvalidParameterException, got \
                 {code} with {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {pattern:?} on {operation}: expected the fixed text {expected:?}, got \
                 {message:?}"
            );
        }
    }

    // Nothing a refused configuration write named was stored.
    let described = logs
        .ok("DescribeMetricFilters", &json!({"logGroupName": GROUP}))
        .await;
    let configured = described["metricFilters"].as_array().map_or(0, Vec::len);
    assert_eq!(
        configured, 0,
        "case after every refused configuration write: the group holds no metric filter, got \
         {described}"
    );
}

/// A search is a REPORT: running an element selector over the group leaves every event a
/// later reader sees exactly as it was.
#[tokio::test]
async fn running_an_element_selector_changes_nothing_a_later_reader_sees() {
    let logs = Logs::new();
    given_stored_documents(&logs, base_ms()).await;

    let read_back = || async {
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupName": GROUP, "logStreamName": STREAM}),
            )
            .await;
        crate::harness::messages(&page)
    };
    let before = read_back().await;
    for row in &ROWS {
        logs.ok(
            "FilterLogEvents",
            &json!({"logGroupName": GROUP, "filterPattern": row.pattern}),
        )
        .await;
    }
    let after = read_back().await;
    assert_eq!(
        after, before,
        "case every pattern of the table: a search changes no event a later reader sees, got \
         {after:?}"
    );
}

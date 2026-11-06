// SPDX-License-Identifier: CC0-1.0

//! `FilterLogEvents` over one log group's stored events, end to end over the wire.
//!
//! RULE: `FilterLogEvents` answers the events of ONE log group that every selector of the
//! request admits — the `filterPattern` ("If not provided, all the events are matched"), the
//! time range (`startTime` "Events with a timestamp before this time are not returned",
//! `endTime` "Events with a timestamp later than this time are not returned") and the
//! log-stream selector ("Filters the results to only logs from the log streams in this list"
//! / "only events from log streams that have names starting with this prefix") — reported in
//! ONE order across the group's streams ("The returned log events are sorted by event
//! timestamp, the timestamp when the event was ingested by CloudWatch Logs, and the ID of
//! the PutLogEvents request", "The response from this operation always interleaves events
//! from multiple log streams within a log group"), each event naming the stream it belongs
//! to and carrying the id of the event, in pages no larger than `limit` that a `nextToken`
//! resumes until pagination is finished.
//!
//! The rows of the table below differ along what the rule generalises over: the FORM of the
//! filter pattern, which of the stored events it selects, how the time range is bounded,
//! which log-stream selector the request uses, and whether the selector names anything the
//! group holds.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, Logs};

/// The one log group every case here searches.
const GROUP: &str = "/aws/emulator/filtered";

/// The group's streams. Two share a prefix the third does not, so a prefix selects a proper
/// subset of them.
const APP_ONE: &str = "app-one";
const APP_TWO: &str = "app-two";
const SYS_ONE: &str = "sys-one";

/// The digits an event id carries, as the delivered `id` and the reported `eventId` both
/// spell it.
const EVENT_ID_DIGITS: usize = 56;

/// One stored event: the stream it is written to, its offset from the suite's base instant
/// and its message. The offsets are distinct, so the order the rule states is total.
struct Stored {
    stream: &'static str,
    offset: i64,
    message: &'static str,
}

/// The events every row searches over, in the chronological order they are stored in.
const STORED: [Stored; 5] = [
    Stored {
        stream: APP_ONE,
        offset: 0,
        message: "ERROR disk full",
    },
    Stored {
        stream: APP_TWO,
        offset: 10,
        message: "{\"level\":\"WARN\",\"code\":500}",
    },
    Stored {
        stream: APP_ONE,
        offset: 20,
        message: "INFO disk fine",
    },
    Stored {
        stream: SYS_ONE,
        offset: 30,
        message: "ERROR kernel panic",
    },
    Stored {
        stream: APP_TWO,
        offset: 40,
        message: "ERROR 404 while handling orders",
    },
];

/// The group, its three streams and the events above, written one batch per stream.
async fn given_stored_events(logs: &Logs, at: i64) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP})).await;
    for stream in [APP_ONE, APP_TWO, SYS_ONE] {
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": GROUP, "logStreamName": stream}),
        )
        .await;
        let events: Vec<(i64, &str)> = STORED
            .iter()
            .filter(|stored| stored.stream == stream)
            .map(|stored| (at + stored.offset, stored.message))
            .collect();
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": stream,
                "logEvents": batch(&events),
            }),
        )
        .await;
    }
}

/// The `(message, logStreamName)` pairs of one answer, in the order it reported them.
fn reported(case: &str, answer: &Value) -> Vec<(String, String)> {
    answer["events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: an answer carries events, got {answer}"))
        .iter()
        .map(|event| {
            let message = event["message"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: an event carries a message, got {event}"));
            let stream = event["logStreamName"].as_str().unwrap_or_else(|| {
                panic!("case {case}: an event names its log stream, got {event}")
            });
            (message.to_string(), stream.to_string())
        })
        .collect()
}

/// One row of the selection table.
struct Row {
    /// What the row varies, named in every assertion it makes.
    case: &'static str,
    /// The request members the row adds to the log group, as offsets from the base instant
    /// where the member is a timestamp.
    pattern: Option<&'static str>,
    start_offset: Option<i64>,
    end_offset: Option<i64>,
    stream_names: Option<&'static [&'static str]>,
    stream_prefix: Option<&'static str>,
    /// The offsets of the stored events the row must report, in order.
    selected: &'static [i64],
}

/// The selection table.
const ROWS: [Row; 9] = [
    Row {
        case: "no selector at all, so every stored event",
        pattern: None,
        start_offset: None,
        end_offset: None,
        stream_names: None,
        stream_prefix: None,
        selected: &[0, 10, 20, 30, 40],
    },
    Row {
        case: "an empty pattern, which matches every event",
        pattern: Some(""),
        start_offset: None,
        end_offset: None,
        stream_names: None,
        stream_prefix: None,
        selected: &[0, 10, 20, 30, 40],
    },
    Row {
        case: "an unstructured term carried by three of five",
        pattern: Some("ERROR"),
        start_offset: None,
        end_offset: None,
        stream_names: None,
        stream_prefix: None,
        selected: &[0, 30, 40],
    },
    Row {
        case: "a JSON condition selecting one",
        pattern: Some("{$.level = \"WARN\"}"),
        start_offset: None,
        end_offset: None,
        stream_names: None,
        stream_prefix: None,
        selected: &[10],
    },
    Row {
        case: "a space-delimited pattern on the first field",
        pattern: Some("[level = ERROR, ...]"),
        start_offset: None,
        end_offset: None,
        stream_names: None,
        stream_prefix: None,
        selected: &[0, 30, 40],
    },
    Row {
        case: "a term nothing carries",
        pattern: Some("NOTHING_MATCHES_THIS"),
        start_offset: None,
        end_offset: None,
        stream_names: None,
        stream_prefix: None,
        selected: &[],
    },
    Row {
        case: "a range bounded at both ends",
        pattern: None,
        start_offset: Some(10),
        end_offset: Some(30),
        stream_names: None,
        stream_prefix: None,
        selected: &[10, 20, 30],
    },
    Row {
        case: "the log streams named, which is one of the three",
        pattern: None,
        start_offset: None,
        end_offset: None,
        stream_names: Some(&[APP_ONE]),
        stream_prefix: None,
        selected: &[0, 20],
    },
    Row {
        case: "a prefix two of the three streams carry",
        pattern: None,
        start_offset: None,
        end_offset: None,
        stream_names: None,
        stream_prefix: Some("app-"),
        selected: &[0, 10, 20, 40],
    },
];

/// The request one row makes.
fn request(row: &Row, at: i64) -> Value {
    let mut body = json!({"logGroupName": GROUP});
    if let Some(pattern) = row.pattern {
        body["filterPattern"] = json!(pattern);
    }
    if let Some(offset) = row.start_offset {
        body["startTime"] = json!(at + offset);
    }
    if let Some(offset) = row.end_offset {
        body["endTime"] = json!(at + offset);
    }
    if let Some(names) = row.stream_names {
        body["logStreamNames"] = json!(names);
    }
    if let Some(prefix) = row.stream_prefix {
        body["logStreamNamePrefix"] = json!(prefix);
    }
    body
}

/// The `(message, stream)` pairs the stored events at `offsets` make up, in order.
fn expected(offsets: &[i64]) -> Vec<(String, String)> {
    offsets
        .iter()
        .map(|offset| {
            let stored = STORED
                .iter()
                .find(|stored| stored.offset == *offset)
                .unwrap_or_else(|| panic!("the table only names stored offsets, not {offset}"));
            (stored.message.to_string(), stored.stream.to_string())
        })
        .collect()
}

/// Every selector admits exactly the stored events it names, interleaved across the group's
/// streams in one order, and `searchedLogStreams` is the empty list the model documents.
#[tokio::test]
async fn every_selector_admits_exactly_the_events_it_names() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    for row in &ROWS {
        let case = row.case;
        let answer = logs.ok("FilterLogEvents", &request(row, at)).await;
        assert_eq!(
            reported(case, &answer),
            expected(row.selected),
            "case {case}: expected the selected events in event order, got {answer}"
        );
        assert_eq!(
            answer["searchedLogStreams"].as_array(),
            Some(&Vec::new()),
            "case {case}: searchedLogStreams is no longer supported and returns an empty list, \
             got {answer}"
        );
        let resumable = answer.get("nextToken").and_then(Value::as_str).is_some();
        assert_eq!(
            resumable,
            !row.selected.is_empty(),
            "case {case}: a page that carried events is resumable and one that carried none \
             finishes pagination, got {answer}"
        );
        for event in answer["events"].as_array().cloned().unwrap_or_default() {
            let id = event["eventId"].as_str().unwrap_or_else(|| {
                panic!("case {case}: an event carries an eventId, got {event}")
            });
            assert_eq!(
                id.len(),
                EVENT_ID_DIGITS,
                "case {case}: an event id carries {EVENT_ID_DIGITS} digits, got {id}"
            );
            assert!(
                event["ingestionTime"].as_i64().is_some_and(|at| at > 0),
                "case {case}: an event reports when it was ingested, got {event}"
            );
            assert!(
                event["timestamp"].as_i64().is_some(),
                "case {case}: an event reports its own timestamp, got {event}"
            );
        }
    }
}

/// A `nextToken` walks the whole scan in pages no larger than `limit`, covering every
/// matched event exactly once and in the same order one page would have reported them.
#[tokio::test]
async fn a_next_token_walks_the_whole_scan_in_limit_sized_pages() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    for limit in [1_i64, 2, 5, 10] {
        let case = format!("pages of at most {limit}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"logGroupName": GROUP, "limit": limit});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let answer = logs.ok("FilterLogEvents", &body).await;
            let page = reported(&case, &answer);
            assert!(
                page.len() <= usize::try_from(limit).unwrap_or(usize::MAX),
                "case {case}: a page is no larger than the limit, got {answer}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= STORED.len() + 1,
                "case {case}: the walk terminates, got {pages} pages"
            );
            match answer["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked,
            expected(&[0, 10, 20, 30, 40]),
            "case {case}: the pages cover every event exactly once, in event order"
        );
    }
}

/// `startFromHead` false reports the latest events first, which is the same set in the
/// reverse order.
#[tokio::test]
async fn a_descending_scan_reports_the_same_set_newest_first() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;
    let case = "newest first";

    let answer = logs
        .ok(
            "FilterLogEvents",
            &json!({"logGroupName": GROUP, "startFromHead": false, "startTime": at}),
        )
        .await;
    let mut newest_first = expected(&[0, 10, 20, 30, 40]);
    newest_first.reverse();
    assert_eq!(
        reported(case, &answer),
        newest_first,
        "case {case}: a descending scan reports the events newest first, got {answer}"
    );
}

/// An event's id is the event's own: stable across reads, and unique within the group.
#[tokio::test]
async fn an_event_id_is_stable_and_unique() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;
    let case = "the ids of one scan read twice";

    let ids = |answer: &Value| -> Vec<String> {
        answer["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| event["eventId"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let first = logs
        .ok("FilterLogEvents", &json!({"logGroupName": GROUP}))
        .await;
    let second = logs
        .ok("FilterLogEvents", &json!({"logGroupName": GROUP}))
        .await;
    let read = ids(&first);
    assert_eq!(
        read,
        ids(&second),
        "case {case}: an event's id is stable across reads, got {first} then {second}"
    );
    assert_eq!(
        read.iter().cloned().collect::<BTreeSet<String>>().len(),
        STORED.len(),
        "case {case}: every stored event has its own id, got {read:?}"
    );
}

/// One row of the refusal table: the request, the error code it must answer and, where the
/// service states a fixed sentence, that sentence.
struct Refusal {
    case: &'static str,
    body: fn() -> Value,
    code: &'static str,
}

/// The refusal table.
const REFUSALS: [Refusal; 6] = [
    Refusal {
        case: "both log-group selectors",
        body: || json!({"logGroupName": GROUP, "logGroupIdentifier": GROUP}),
        code: "ValidationException",
    },
    Refusal {
        case: "neither log-group selector",
        body: || json!({"filterPattern": "ERROR"}),
        code: "ValidationException",
    },
    Refusal {
        case: "both log-stream selectors",
        body: || {
            json!({
                "logGroupName": GROUP,
                "logStreamNames": [APP_ONE],
                "logStreamNamePrefix": "app-",
            })
        },
        code: "InvalidParameterException",
    },
    Refusal {
        case: "a log group that does not exist",
        body: || json!({"logGroupName": "/aws/emulator/absent"}),
        code: "ResourceNotFoundException",
    },
    Refusal {
        case: "a filter pattern that opens a bracketed form and never closes it",
        body: || json!({"logGroupName": GROUP, "filterPattern": "[level = ERROR"}),
        code: "InvalidParameterException",
    },
    Refusal {
        case: "a nextToken this operation did not mint",
        body: || json!({"logGroupName": GROUP, "nextToken": "not-a-token-of-this-operation"}),
        code: "InvalidParameterException",
    },
];

/// Each refused request is refused with the error shape the service answers for its class.
#[tokio::test]
async fn a_request_the_service_refuses_is_refused_with_its_own_error() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    for refusal in &REFUSALS {
        let case = refusal.case;
        let body = (refusal.body)();
        let code = logs.err("FilterLogEvents", &body).await;
        assert_eq!(
            code, refusal.code,
            "case {case}: expected {}, got {code}",
            refusal.code
        );
    }
}

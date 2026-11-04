// SPDX-License-Identifier: CC0-1.0

//! `StartLiveTail` sessions over the stored log events, end to end over the wire.
//!
//! RULE: a Live Tail session is served as HTTP 200 whose body is an AWS event stream —
//! "A single LiveTailSessionStart object is sent at the start of the session", then a
//! `LiveTailSessionUpdate` whose "array of log events" holds the events of the log
//! groups the request named that every selector of the request admits (`logStreamNames`
//! "only log events in the log streams that you specify here", `logStreamNamePrefixes`,
//! `logEventFilterPattern` "include only log events that match the pattern"), bounded by
//! the 500 events an update may carry and reporting `sampled` exactly when the match
//! exceeded that — and a session over a log group the account does not hold is refused,
//! like every modelled error of this operation, as a client error at HTTP 400.
//!
//! The rows of the tables below differ along what the rule generalises over: which
//! selector the request carries and in which FORM (a name or an ARN, named streams or
//! prefixed ones, a term pattern, a JSON pattern, a space-delimited pattern), how many
//! log groups the session spans, how many events the selection admits relative to the
//! update's ceiling, and which modelled constraint a refused request broke.

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;
use serde_json::{json, Value};

use crate::harness::{base_ms, batch, messages, Logs};

/// The two log groups the sessions below tail. Two groups rather than one, because a
/// session spans "up to 10 log groups" and interleaves their events.
const GROUP_A: &str = "/aws/emulator/live-tail-a";
const GROUP_B: &str = "/aws/emulator/live-tail-b";

/// `GROUP_A`'s streams: two share a prefix, so a prefix selects a proper subset.
const APP_ONE: &str = "app-one";
const APP_TWO: &str = "app-two";
/// `GROUP_B`'s one stream.
const SYS_ONE: &str = "sys-one";

/// The media type an event-stream body carries.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// One stored event: the group and stream it is written to, its offset from the suite's
/// base instant and its message. The offsets are distinct across BOTH groups, so the
/// chronological order a session delivers them in is total.
struct Stored {
    group: &'static str,
    stream: &'static str,
    offset: i64,
    message: &'static str,
}

/// The events every selection row tails over.
const STORED: [Stored; 5] = [
    Stored {
        group: GROUP_A,
        stream: APP_ONE,
        offset: 0,
        message: "ERROR disk full",
    },
    Stored {
        group: GROUP_A,
        stream: APP_TWO,
        offset: 10,
        message: "{\"level\":\"WARN\",\"code\":500}",
    },
    Stored {
        group: GROUP_A,
        stream: APP_ONE,
        offset: 20,
        message: "INFO disk fine",
    },
    Stored {
        group: GROUP_B,
        stream: SYS_ONE,
        offset: 30,
        message: "ERROR kernel panic",
    },
    Stored {
        group: GROUP_A,
        stream: APP_TWO,
        offset: 40,
        message: "ERROR 404 while handling orders",
    },
];

/// The two groups, their streams and the events above, written one batch per stream.
async fn given_stored_events(logs: &Logs, at: i64) {
    for group in GROUPS {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    for (group, stream) in [
        (GROUP_A, APP_ONE),
        (GROUP_A, APP_TWO),
        (GROUP_B, SYS_ONE),
    ] {
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": stream}),
        )
        .await;
        let events: Vec<(i64, &str)> = STORED
            .iter()
            .filter(|stored| stored.group == group && stored.stream == stream)
            .map(|stored| (at + stored.offset, stored.message))
            .collect();
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": stream,
                "logEvents": batch(&events),
            }),
        )
        .await;
    }
}

/// One decoded event-stream frame: the `:event-type` the union member is framed under,
/// and its payload.
struct Frame {
    event_type: String,
    payload: Value,
}

/// Decode an event-stream body, asserting the frame headers an SDK decoder requires.
fn decode_frames(case: &str, body: &[u8]) -> Vec<Frame> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(
            header(":message-type").as_deref(),
            Some("event"),
            "case {case:?}: a session frame must be an event, got {:?}",
            header(":message-type")
        );
        let event_type = header(":event-type")
            .unwrap_or_else(|| panic!("case {case:?}: a session frame named no :event-type"));
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!("case {case:?}: the {event_type} payload must be JSON: {error}")
        });
        frames.push(Frame {
            event_type,
            payload,
        });
    }
    frames
}

/// Start a session that must be served, returning its `sessionStart` and its one
/// `sessionUpdate` payload after asserting the whole wire shape around them.
async fn session(case: &str, logs: &Logs, request: &Value) -> (Value, Value) {
    let (status, headers, body) = logs.call_raw("StartLiveTail", request).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected HTTP 200 from StartLiveTail, got {status} with {:?}",
        String::from_utf8_lossy(&body)
    );
    let media_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    assert_eq!(
        media_type, EVENT_STREAM,
        "case {case:?}: expected an event-stream body, got {media_type:?}"
    );
    let frames = decode_frames(case, &body);
    let types: Vec<&str> = frames
        .iter()
        .map(|frame| frame.event_type.as_str())
        .collect();
    assert_eq!(
        types,
        vec!["sessionStart", "sessionUpdate"],
        "case {case:?}: a session sends one sessionStart then its update, got {types:?}"
    );
    let mut frames = frames.into_iter();
    let start = frames
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the session sent no sessionStart"))
        .payload;
    let update = frames
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the session sent no sessionUpdate"))
        .payload;
    (start, update)
}

/// The `(message, logStreamName, logGroupIdentifier)` triples one update delivered, in
/// the order it delivered them.
fn delivered(case: &str, update: &Value) -> Vec<(String, String, String)> {
    update["sessionResults"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: an update carries sessionResults, got {update}"))
        .iter()
        .map(|event| {
            let member = |key: &str| {
                event[key]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a delivered event carries {key}, got {event}")
                    })
                    .to_string()
            };
            (
                member("message"),
                member("logStreamName"),
                member("logGroupIdentifier"),
            )
        })
        .collect()
}

/// Whether one update reports itself sampled.
fn sampled(case: &str, update: &Value) -> bool {
    update["sessionMetadata"]["sampled"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: an update reports sampled, got {update}"))
}

/// One row of the selection table.
struct Row {
    /// What the row varies, named in every assertion it makes.
    case: &'static str,
    /// Which stored groups the session names, by index into [`GROUPS`].
    groups: &'static [usize],
    /// Whether the row names its groups by ARN rather than by name.
    by_arn: bool,
    stream_names: Option<&'static [&'static str]>,
    stream_prefixes: Option<&'static [&'static str]>,
    filter_pattern: Option<&'static str>,
    /// The offsets of the stored events the row must be delivered, in order.
    delivered: &'static [i64],
}

/// The groups a row's indexes address.
const GROUPS: [&str; 2] = [GROUP_A, GROUP_B];

/// The selection table.
const ROWS: [Row; 10] = [
    Row {
        case: "one group by name and no selector, so every event it holds",
        groups: &[0],
        by_arn: false,
        stream_names: None,
        stream_prefixes: None,
        filter_pattern: None,
        delivered: &[0, 10, 20, 40],
    },
    Row {
        case: "one group by ARN, which names the same group",
        groups: &[0],
        by_arn: true,
        stream_names: None,
        stream_prefixes: None,
        filter_pattern: None,
        delivered: &[0, 10, 20, 40],
    },
    Row {
        case: "two groups, whose events interleave in one chronological run",
        groups: &[0, 1],
        by_arn: false,
        stream_names: None,
        stream_prefixes: None,
        filter_pattern: None,
        delivered: &[0, 10, 20, 30, 40],
    },
    Row {
        case: "two groups by ARN",
        groups: &[0, 1],
        by_arn: true,
        stream_names: None,
        stream_prefixes: None,
        filter_pattern: None,
        delivered: &[0, 10, 20, 30, 40],
    },
    Row {
        case: "the log streams named, which is one of the group's two",
        groups: &[0],
        by_arn: false,
        stream_names: Some(&[APP_ONE]),
        stream_prefixes: None,
        filter_pattern: None,
        delivered: &[0, 20],
    },
    Row {
        case: "a log stream prefix both the group's streams carry",
        groups: &[0],
        by_arn: false,
        stream_names: None,
        stream_prefixes: Some(&["app-"]),
        filter_pattern: None,
        delivered: &[0, 10, 20, 40],
    },
    Row {
        case: "a log stream prefix no stream carries",
        groups: &[0],
        by_arn: false,
        stream_names: None,
        stream_prefixes: Some(&["nothing-"]),
        filter_pattern: None,
        delivered: &[],
    },
    Row {
        case: "an unstructured term carried by three of the five events",
        groups: &[0, 1],
        by_arn: false,
        stream_names: None,
        stream_prefixes: None,
        filter_pattern: Some("ERROR"),
        delivered: &[0, 30, 40],
    },
    Row {
        case: "a JSON condition selecting one event",
        groups: &[0, 1],
        by_arn: false,
        stream_names: None,
        stream_prefixes: None,
        filter_pattern: Some("{$.level = \"WARN\"}"),
        delivered: &[10],
    },
    Row {
        case: "a space-delimited pattern on the first field",
        groups: &[0, 1],
        by_arn: false,
        stream_names: None,
        stream_prefixes: None,
        filter_pattern: Some("[level = ERROR, ...]"),
        delivered: &[0, 30, 40],
    },
];

/// A session delivers exactly the stored events its selection admits, each naming the
/// stream it was ingested into and the identifier its group was named by, in one
/// chronological run — and its `sessionStart` echoes the selection it was started with.
#[tokio::test]
async fn a_session_delivers_the_events_its_selection_admits() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    // The ARN form of each group, read back off the group itself rather than built here,
    // so a row that names a group by ARN names the same group the name form does.
    let mut arns = Vec::new();
    for group in GROUPS {
        let (_wildcard, log_group_arn) = logs.group_arns(group).await;
        arns.push(log_group_arn);
    }

    for row in &ROWS {
        let identifiers: Vec<String> = row
            .groups
            .iter()
            .map(|index| {
                if row.by_arn {
                    arns[*index].clone()
                } else {
                    GROUPS[*index].to_string()
                }
            })
            .collect();
        let mut request = json!({"logGroupIdentifiers": identifiers});
        if let Some(names) = row.stream_names {
            request["logStreamNames"] = json!(names);
        }
        if let Some(prefixes) = row.stream_prefixes {
            request["logStreamNamePrefixes"] = json!(prefixes);
        }
        if let Some(pattern) = row.filter_pattern {
            request["logEventFilterPattern"] = json!(pattern);
        }

        let (start, update) = session(row.case, &logs, &request).await;

        // The session names itself and echoes the selection: "A single
        // LiveTailSessionStart object is sent at the start of the session."
        assert_eq!(
            start["logGroupIdentifiers"],
            json!(identifiers),
            "case {:?}: sessionStart echoes the log groups the session tails, got {start}",
            row.case
        );
        assert_eq!(
            start["logStreamNames"],
            row.stream_names.map_or(Value::Null, |names| json!(names)),
            "case {:?}: sessionStart echoes logStreamNames, got {start}",
            row.case
        );
        assert_eq!(
            start["logStreamNamePrefixes"],
            row.stream_prefixes
                .map_or(Value::Null, |prefixes| json!(prefixes)),
            "case {:?}: sessionStart echoes logStreamNamePrefixes, got {start}",
            row.case
        );
        assert_eq!(
            start["logEventFilterPattern"],
            row.filter_pattern
                .map_or(Value::Null, |pattern| json!(pattern)),
            "case {:?}: sessionStart echoes logEventFilterPattern, got {start}",
            row.case
        );
        for member in ["requestId", "sessionId"] {
            let value = start[member].as_str().unwrap_or_else(|| {
                panic!(
                    "case {:?}: sessionStart names the session's {member}, got {start}",
                    row.case
                )
            });
            assert!(
                !value.is_empty(),
                "case {:?}: the session's {member} is not empty, got {value:?}",
                row.case
            );
        }

        let expected: Vec<(String, String, String)> = row
            .delivered
            .iter()
            .map(|offset| {
                let stored = STORED
                    .iter()
                    .find(|stored| stored.offset == *offset)
                    .unwrap_or_else(|| panic!("case {:?}: {offset} is a stored event", row.case));
                let index = GROUPS
                    .iter()
                    .position(|group| *group == stored.group)
                    .unwrap_or_else(|| panic!("case {:?}: {} is a row group", row.case, stored.group));
                let identifier = if row.by_arn {
                    arns[index].clone()
                } else {
                    stored.group.to_string()
                };
                (
                    stored.message.to_string(),
                    stored.stream.to_string(),
                    identifier,
                )
            })
            .collect();
        let actual = delivered(row.case, &update);
        assert_eq!(
            actual, expected,
            "case {:?}: expected the update to deliver {expected:?}, got {actual:?}",
            row.case
        );
        assert!(
            !sampled(row.case, &update),
            "case {:?}: {} events is under the update's ceiling, so the update is not sampled",
            row.case,
            expected.len()
        );
    }
}

/// One sampling row: how large each stored message is, how many are stored, and how many
/// of them the update carries. The rows straddle BOTH ceilings — the documented 500
/// events (under it, at it, past it) and the bytes one update carries (past it, and the
/// single event larger than it that is carried whole).
struct Sampling {
    case: &'static str,
    message_bytes: usize,
    stored: usize,
    delivered: usize,
}

/// The documented event ceiling of one update, as `StartLiveTail` states it ("can
/// include as many as 500 log events").
const MAX_EVENTS_PER_UPDATE: usize = 500;

/// A message small enough that the byte ceiling is never the row's bound.
const SMALL_MESSAGE_BYTES: usize = 16;

/// A message large enough that three of them do not fit one update.
const LARGE_MESSAGE_BYTES: usize = 400_000;

/// A message large enough that two of them do not fit one update.
const HUGE_MESSAGE_BYTES: usize = 1_000_000;

/// The sampling table.
const SAMPLING_ROWS: [Sampling; 6] = [
    Sampling {
        case: "one event under the event ceiling",
        message_bytes: SMALL_MESSAGE_BYTES,
        stored: MAX_EVENTS_PER_UPDATE - 1,
        delivered: MAX_EVENTS_PER_UPDATE - 1,
    },
    Sampling {
        case: "exactly the event ceiling",
        message_bytes: SMALL_MESSAGE_BYTES,
        stored: MAX_EVENTS_PER_UPDATE,
        delivered: MAX_EVENTS_PER_UPDATE,
    },
    Sampling {
        case: "seven events past the event ceiling",
        message_bytes: SMALL_MESSAGE_BYTES,
        stored: MAX_EVENTS_PER_UPDATE + 7,
        delivered: MAX_EVENTS_PER_UPDATE,
    },
    Sampling {
        case: "two large events, both of which fit one update",
        message_bytes: LARGE_MESSAGE_BYTES,
        stored: 2,
        delivered: 2,
    },
    Sampling {
        case: "three large events, of which two fit one update",
        message_bytes: LARGE_MESSAGE_BYTES,
        stored: 3,
        delivered: 2,
    },
    Sampling {
        case: "two events of which only the newest fits one update",
        message_bytes: HUGE_MESSAGE_BYTES,
        stored: 2,
        delivered: 1,
    },
];

/// An update carries at most the 500 events the model allows it, and at most the bytes of
/// messages one update carries: past either ceiling it delivers the most recent events
/// that fit and reports itself sampled, and within both it delivers every matching event
/// and reports itself unsampled.
#[tokio::test]
async fn an_update_is_bounded_by_what_one_update_may_carry() {
    for row in &SAMPLING_ROWS {
        let case = row.case;
        let logs = Logs::new();
        let group = format!("/aws/emulator/live-tail-{}-{}", row.stored, row.message_bytes);
        let at = base_ms();
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": APP_ONE}),
        )
        .await;
        // Each message is distinct and of the row's size, so which events the update
        // carried is visible and the byte ceiling is reached by the row that means to.
        let written: Vec<String> = (0..row.stored)
            .map(|index| {
                let label = format!("event {index} ");
                let filler = "x".repeat(row.message_bytes.saturating_sub(label.len()));
                format!("{label}{filler}")
            })
            .collect();
        // One batch per event, as a client writes a long run of them.
        for (index, message) in written.iter().enumerate() {
            logs.ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": APP_ONE,
                    "logEvents": batch(&[(
                        at + i64::try_from(index).unwrap_or_default(),
                        message.as_str(),
                    )]),
                }),
            )
            .await;
        }

        let (_start, update) = session(
            case,
            &logs,
            &json!({"logGroupIdentifiers": [group.clone()]}),
        )
        .await;
        let actual = delivered(case, &update);
        assert_eq!(
            actual.len(),
            row.delivered,
            "case {case:?}: expected {} delivered events, got {}",
            row.delivered,
            actual.len()
        );
        // The sample is the most recent events, which is what a tail is watching.
        let newest: Vec<String> = written
            .iter()
            .skip(row.stored - row.delivered)
            .cloned()
            .collect();
        let delivered_messages: Vec<String> =
            actual.iter().map(|(message, _, _)| message.clone()).collect();
        assert_eq!(
            delivered_messages, newest,
            "case {case:?}: expected the most recent {} events, got {} of them",
            row.delivered,
            delivered_messages.len()
        );
        assert_eq!(
            sampled(case, &update),
            row.stored > row.delivered,
            "case {case:?}: an update reports itself sampled exactly when it left a \
             matching event out"
        );
    }
}

/// One refusal row: the request that is refused, and the error code and message it is
/// refused with.
struct Refusal {
    case: &'static str,
    /// The request body, built against the stored groups.
    request: fn(&str) -> Value,
    code: &'static str,
    message: &'static str,
}

/// The refusal table. Every modelled error of this operation is a client error at HTTP
/// 400, which [`Logs::refusal`] asserts for each row.
const REFUSALS: [Refusal; 10] = [
    Refusal {
        case: "a log group name the account does not hold",
        request: |_| json!({"logGroupIdentifiers": ["/aws/emulator/never-created"]}),
        code: "ResourceNotFoundException",
        message: "Provided LogGroup doesn't exist.",
    },
    Refusal {
        case: "a log group ARN the account does not hold",
        request: |_| json!({
            "logGroupIdentifiers": [
                "arn:aws:logs:eu-west-2:000000000000:log-group:/aws/emulator/never-created"
            ]
        }),
        code: "ResourceNotFoundException",
        message: "Provided LogGroup doesn't exist.",
    },
    Refusal {
        case: "one unknown log group among two the account holds",
        request: |deleted| {
            json!({"logGroupIdentifiers": [GROUP_A, deleted, GROUP_B]})
        },
        code: "ResourceNotFoundException",
        message: "Provided LogGroup doesn't exist.",
    },
    Refusal {
        case: "a log group that was deleted after it was created",
        request: |deleted| json!({"logGroupIdentifiers": [deleted]}),
        code: "ResourceNotFoundException",
        message: "Provided LogGroup doesn't exist.",
    },
    Refusal {
        case: "both log stream selectors at once",
        request: |_| json!({
            "logGroupIdentifiers": [GROUP_A],
            "logStreamNames": [APP_ONE],
            "logStreamNamePrefixes": ["app-"],
        }),
        code: "InvalidParameterException",
        message: "logStreamNames and logStreamNamePrefixes are mutually exclusive parameters.",
    },
    Refusal {
        case: "a log stream selector across two log groups",
        request: |_| json!({
            "logGroupIdentifiers": [GROUP_A, GROUP_B],
            "logStreamNames": [APP_ONE],
        }),
        code: "InvalidParameterException",
        message: "logStreamNames and logStreamNamePrefixes can be specified only with a single \
                  log group in logGroupIdentifiers.",
    },
    Refusal {
        case: "no logGroupIdentifiers at all, which the model requires",
        request: |_| json!({}),
        code: "InvalidParameterException",
        message: "1 validation error detected: Value at 'logGroupIdentifiers' failed to satisfy \
                  constraint: Member must not be null",
    },
    Refusal {
        case: "an empty logGroupIdentifiers, which is under the modelled minimum of 1",
        request: |_| json!({"logGroupIdentifiers": []}),
        code: "InvalidParameterException",
        message: "1 validation error detected: Value at 'logGroupIdentifiers' failed to satisfy \
                  constraint: Member must have length greater than or equal to 1",
    },
    Refusal {
        case: "an asterisk-suffixed log group ARN, which the identifier pattern excludes",
        request: |_| json!({
            "logGroupIdentifiers": [
                "arn:aws:logs:eu-west-2:000000000000:log-group:/aws/emulator/live-tail-a:*"
            ]
        }),
        code: "InvalidParameterException",
        message: "1 validation error detected: Value at 'logGroupIdentifiers.1.member' failed to \
                  satisfy constraint: Member must satisfy regular expression pattern: \
                  ^[\\w#+=/:,.@-]*$",
    },
    Refusal {
        case: "a filter pattern that opens a bracketed form it does not close",
        request: |_| json!({
            "logGroupIdentifiers": [GROUP_A],
            "logEventFilterPattern": "{$.level = \"WARN\"",
        }),
        code: "InvalidParameterException",
        message: "If a filter pattern starts with '{' it must end with '}'",
    },
];

/// The log group each refusal row that needs one names: created, then deleted, so a row
/// that must be refused for a group the account does not hold is refused for a group the
/// store once held rather than for a name no request ever created.
const DELETED_GROUP: &str = "/aws/emulator/live-tail-deleted";

/// Every refused session is refused with the modelled error the condition names, at the
/// HTTP 400 every modelled error of this operation carries — never with a 501, which an
/// SDK classifies as a retryable server fault.
#[tokio::test]
async fn a_refused_session_answers_the_modelled_client_error() {
    let logs = Logs::new();
    given_stored_events(&logs, base_ms()).await;
    logs.ok("CreateLogGroup", &json!({"logGroupName": DELETED_GROUP}))
        .await;
    logs.ok("DeleteLogGroup", &json!({"logGroupName": DELETED_GROUP}))
        .await;

    for row in &REFUSALS {
        let request = (row.request)(DELETED_GROUP);
        let (code, message) = logs.refusal("StartLiveTail", &request).await;
        assert_eq!(
            code, row.code,
            "case {:?}: expected {}, got {code} with {message:?}",
            row.case, row.code
        );
        assert_eq!(
            message, row.message,
            "case {:?}: expected the message {:?}, got {message:?}",
            row.case, row.message
        );
    }
}

/// A session REPORTS what the groups it tails hold: it stores nothing, so what a later
/// reader sees is exactly what it saw before — and a second session over the same
/// selection delivers the same events.
#[tokio::test]
async fn a_session_leaves_the_groups_it_tailed_unchanged() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    let read_back = || async {
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupName": GROUP_A, "logStreamName": APP_ONE}),
            )
            .await;
        messages(&page)
    };
    let before = read_back().await;
    let group_before = logs.describe_group(GROUP_A).await;

    let request = json!({"logGroupIdentifiers": [GROUP_A, GROUP_B]});
    let (_start, first) = session("the first session", &logs, &request).await;
    let (_start, second) = session("the second session", &logs, &request).await;
    assert_eq!(
        delivered("the second session", &second),
        delivered("the first session", &first),
        "case {:?}: a second session over one selection delivers the same events",
        "two sessions"
    );

    let after = read_back().await;
    assert_eq!(
        after, before,
        "case {:?}: a session changes no event a later reader sees, got {after:?}",
        "read back after tailing"
    );
    let group_after = logs.describe_group(GROUP_A).await;
    assert_eq!(
        group_after, group_before,
        "case {:?}: a session changes nothing the log group reports, got {group_after:?}",
        "describe after tailing"
    );
}

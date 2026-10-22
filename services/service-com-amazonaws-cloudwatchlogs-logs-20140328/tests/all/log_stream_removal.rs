// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for removing one log stream: `DeleteLogStream`, what
//! `DescribeLogStreams` and `GetLogEvents` report afterwards, and the `storedBytes` the
//! owning log group is left reporting.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the classes the rule generalises over: how many streams the
//! group holds, which of them is removed (first, middle, last, the only one), and how
//! many events each holds. Any single stream an issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::Logs;

/// Per-event storage overhead a log group's `storedBytes` counts. Source: the
/// `PutLogEvents` quota — a batch's size "is calculated as the sum of all event messages
/// in UTF-8, plus 26 bytes for each log event".
const EVENT_OVERHEAD_BYTES: i64 = 26;

/// The `logStreamName`s `DescribeLogStreams` reports for one log group, in the order
/// returned.
async fn stream_names(logs: &Logs, group: &str) -> Vec<String> {
    let page = logs
        .ok("DescribeLogStreams", &json!({"logGroupName": group}))
        .await;
    page.get("logStreams")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeLogStreams must return logStreams, got {page}"))
        .iter()
        .map(|stream| {
            stream
                .get("logStreamName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every stream carries a logStreamName, got {stream}"))
                .to_string()
        })
        .collect()
}

/// The `storedBytes` `DescribeLogGroups` reports for one log group.
async fn stored_bytes(logs: &Logs, group: &str) -> i64 {
    let page = logs
        .ok("DescribeLogGroups", &json!({"logGroupIdentifiers": [group]}))
        .await;
    page["logGroups"][0]
        .get("storedBytes")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("a log group carries storedBytes, got {page}"))
}

/// The `message` values `GetLogEvents` reads back from one stream, in the order returned.
async fn messages(logs: &Logs, group: &str, stream: &str) -> Vec<String> {
    let page = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": group, "logStreamName": stream}),
        )
        .await;
    page.get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetLogEvents must return events, got {page}"))
        .iter()
        .map(|event| {
            event
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every event carries a message, got {event}"))
                .to_string()
        })
        .collect()
}

/// The messages written into stream `index` of a case's group: `events` of them, each
/// distinguishable by stream and position, and of a different UTF-8 length per stream so
/// the byte accounting cannot pass by coincidence.
fn messages_for(index: usize, events: usize) -> Vec<String> {
    (0..events)
        .map(|position| format!("s{index}-e{position}{}", "x".repeat(index)))
        .collect()
}

/// What `messages_for` adds to the owning group's `storedBytes`.
fn bytes_of(messages: &[String]) -> i64 {
    messages
        .iter()
        .map(|message| i64::try_from(message.len()).expect("a message length fits") + EVENT_OVERHEAD_BYTES)
        .sum()
}

/// Create a group holding `streams` streams, stream `index` carrying `index + 1` events,
/// and answer each stream's messages.
async fn group_with_streams(logs: &Logs, group: &str, streams: usize) -> Vec<Vec<String>> {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let mut written = Vec::with_capacity(streams);
    for index in 0..streams {
        let name = format!("stream-{index}");
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": name}),
        )
        .await;
        // Stream 0 stays empty, so "a stream nothing was written to" is a row too.
        let messages = messages_for(index, index);
        if !messages.is_empty() {
            let events: Vec<Value> = messages
                .iter()
                .enumerate()
                .map(|(position, message)| {
                    json!({
                        "timestamp": 1_700_000_000_000i64 + i64::try_from(position).expect("fits"),
                        "message": message,
                    })
                })
                .collect();
            logs.ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": name,
                    "logEvents": events,
                }),
            )
            .await;
        }
        written.push(messages);
    }
    written
}

/// A delete removes exactly the named stream: it is no longer listed, its events are
/// gone, every sibling keeps its own events, and the group's `storedBytes` drops by the
/// removed stream's bytes alone. Crossed over the number of streams the group holds and
/// which of them is removed.
#[tokio::test]
async fn a_delete_removes_exactly_the_named_stream() {
    let logs = Logs::new();

    // (streams in the group, index removed): the only stream, the first, a middle one,
    // and the last.
    let cases: [(usize, usize); 6] = [(1, 0), (2, 0), (2, 1), (5, 0), (5, 2), (5, 4)];

    for (streams, removed) in cases {
        let group = format!("/aws/emulator/streams-{streams}-remove-{removed}");
        let case = format!("streams={streams} removed={removed}");
        let written = group_with_streams(&logs, &group, streams).await;
        let before = stored_bytes(&logs, &group).await;
        let total: i64 = written.iter().map(|m| bytes_of(m)).sum();
        assert_eq!(
            before, total,
            "case {case}: the group's storedBytes counts every stream's events"
        );

        let name = format!("stream-{removed}");
        let body = logs
            .ok(
                "DeleteLogStream",
                &json!({"logGroupName": group, "logStreamName": name}),
            )
            .await;
        let members = match &body {
            Value::Null => 0,
            Value::Object(map) => map.len(),
            other => panic!("case {case}: a Unit output is an empty body, got {other}"),
        };
        assert_eq!(
            members, 0,
            "case {case}: a Unit output carries no members, got {body}"
        );

        // The stream the service no longer holds is the one it no longer lists.
        let expected: Vec<String> = (0..streams)
            .filter(|index| *index != removed)
            .map(|index| format!("stream-{index}"))
            .collect();
        assert_eq!(
            stream_names(&logs, &group).await,
            expected,
            "case {case}: only the removed stream is gone from the group's stream set"
        );

        // "permanently deletes all the archived log events associated with the log
        // stream": the stream is unreachable, and the group's bytes drop by its events.
        let code = logs
            .err(
                "GetLogEvents",
                &json!({"logGroupName": group, "logStreamName": name}),
            )
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: the removed stream's events are no longer readable, got {code}"
        );
        assert_eq!(
            stored_bytes(&logs, &group).await,
            before - bytes_of(&written[removed]),
            "case {case}: the group's storedBytes drops by the removed stream's bytes alone"
        );

        // Every sibling still reads back exactly what was written to it.
        for index in (0..streams).filter(|index| *index != removed) {
            let sibling = format!("stream-{index}");
            assert_eq!(
                messages(&logs, &group, &sibling).await,
                written[index],
                "case {case}: sibling {sibling} keeps its own events"
            );
        }

        // The group itself survives the removal of every one of its streams.
        for index in (0..streams).filter(|index| *index != removed) {
            logs.ok(
                "DeleteLogStream",
                &json!({"logGroupName": group, "logStreamName": format!("stream-{index}")}),
            )
            .await;
        }
        assert_eq!(
            stream_names(&logs, &group).await,
            Vec::<String>::new(),
            "case {case}: the group is left holding no streams"
        );
        assert_eq!(
            stored_bytes(&logs, &group).await,
            0,
            "case {case}: a group holding no streams stores no bytes"
        );
    }
}

/// A delete naming a resource the service does not hold is refused with
/// `ResourceNotFoundException`, and removes nothing — including a second delete of a
/// stream already removed.
#[tokio::test]
async fn a_delete_of_an_absent_resource_is_refused() {
    let logs = Logs::new();
    let group = "present/group";
    let written = group_with_streams(&logs, group, 3).await;
    logs.ok(
        "DeleteLogStream",
        &json!({"logGroupName": group, "logStreamName": "stream-1"}),
    )
    .await;
    let after_one = stored_bytes(&logs, group).await;

    let refused = [
        (
            "an unknown log group",
            json!({"logGroupName": "no-such-group", "logStreamName": "stream-0"}),
        ),
        (
            "a group name differing only in case",
            json!({"logGroupName": "Present/Group", "logStreamName": "stream-0"}),
        ),
        (
            "a stream nobody created",
            json!({"logGroupName": group, "logStreamName": "no-such-stream"}),
        ),
        (
            "a stream name differing only in case",
            json!({"logGroupName": group, "logStreamName": "Stream-0"}),
        ),
        (
            "a stream already removed",
            json!({"logGroupName": group, "logStreamName": "stream-1"}),
        ),
    ];

    for (case, request) in refused {
        let code = logs.err("DeleteLogStream", &request).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case} ({request}): a delete names a resource that exists, got {code}"
        );
    }

    // Nothing the refusals named was removed.
    assert_eq!(
        stream_names(&logs, group).await,
        vec!["stream-0".to_string(), "stream-2".to_string()],
        "case after the refusals: a refused delete removes nothing"
    );
    assert_eq!(
        messages(&logs, group, "stream-2").await,
        written[2],
        "case after the refusals: the surviving stream keeps its events"
    );
    assert_eq!(
        stored_bytes(&logs, group).await,
        after_one,
        "case after the refusals: a refused delete changes no byte count"
    );
}

/// A removed stream name is free again: re-creating it succeeds and starts from an empty
/// stream, with none of the removed stream's event metadata.
#[tokio::test]
async fn a_removed_stream_can_be_created_again() {
    let logs = Logs::new();
    let group = "recreated.streams";
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;

    for round in 0..3 {
        let name = "reused-stream";
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": name}),
        )
        .await;
        let message = format!("round-{round}");
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": name,
                "logEvents": [{"timestamp": 1_700_000_000_000i64, "message": message}],
            }),
        )
        .await;
        assert_eq!(
            messages(&logs, group, name).await,
            vec![message.clone()],
            "case round {round}: the re-created stream holds what was written to it"
        );

        logs.ok(
            "DeleteLogStream",
            &json!({"logGroupName": group, "logStreamName": name}),
        )
        .await;
        assert_eq!(
            stream_names(&logs, group).await,
            Vec::<String>::new(),
            "case round {round}: the removed stream is no longer listed"
        );

        // A re-create of the name the delete freed is accepted, and the stream it makes
        // carries no event metadata from the one that was removed.
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": name}),
        )
        .await;
        let page = logs
            .ok("DescribeLogStreams", &json!({"logGroupName": group}))
            .await;
        let stream = &page["logStreams"][0];
        for member in [
            "firstEventTimestamp",
            "lastEventTimestamp",
            "lastIngestionTime",
            "uploadSequenceToken",
        ] {
            assert!(
                stream.get(member).is_none_or(Value::is_null),
                "case round {round}: a re-created stream reports no {member}, got {stream}"
            );
        }
        assert_eq!(
            messages(&logs, group, name).await,
            Vec::<String>::new(),
            "case round {round}: a re-created stream starts empty"
        );
        assert_eq!(
            stored_bytes(&logs, group).await,
            0,
            "case round {round}: the removed stream's bytes are no longer counted"
        );
        logs.ok(
            "DeleteLogStream",
            &json!({"logGroupName": group, "logStreamName": name}),
        )
        .await;
    }
}

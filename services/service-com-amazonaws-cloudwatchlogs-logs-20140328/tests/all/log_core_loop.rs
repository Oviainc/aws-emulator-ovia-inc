// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the log-group / log-stream / log-event core loop.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the classes the rules generalise over (name shape, log group
//! class, batch size and ordering, time window, page size, ordering key, identifier
//! form); the values any one issue happens to mention are single rows.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, group_with_stream, messages, Logs, REGION};

/// A log group class the model declares. Source: the `LogGroupClass` enum, surfaced in
/// the crate as `LOG_GROUP_CLASS_VALUES` ("DELIVERY", "INFREQUENT_ACCESS", "STANDARD").
const LOG_GROUP_CLASSES: &[&str] = &["STANDARD", "INFREQUENT_ACCESS", "DELIVERY"];

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// A create is readable back through `DescribeLogGroups`, with the ARNs, the class and
/// the counters the model declares — over the whole class of accepted names and classes.
#[tokio::test]
async fn created_log_groups_are_described_exactly_as_created() {
    let logs = Logs::new();

    // name shape × class × optional members: the rows differ along everything the
    // create rule generalises over.
    let cases: Vec<(&str, Option<&str>, bool, Option<&str>)> = vec![
        ("/aws/emulator/demo", None, false, None),
        ("plain", Some(LOG_GROUP_CLASSES[0]), false, None),
        ("with.dots-and_underscores", Some(LOG_GROUP_CLASSES[1]), true, None),
        ("Mixed/Case#Hash", Some(LOG_GROUP_CLASSES[2]), false, Some("arn:aws:kms:eu-west-2:000000000000:key/abc")),
        ("x", None, true, None),
    ];

    for (name, class, protected, kms) in &cases {
        let mut request = json!({"logGroupName": name});
        if let Some(class) = class {
            request["logGroupClass"] = json!(class);
        }
        if *protected {
            request["deletionProtectionEnabled"] = json!(true);
        }
        if let Some(kms) = kms {
            request["kmsKeyId"] = json!(kms);
        }
        request["tags"] = json!({"team": "emulator"});
        logs.ok("CreateLogGroup", &request).await;

        let described = logs
            .ok(
                "DescribeLogGroups",
                &json!({"logGroupIdentifiers": [name]}),
            )
            .await;
        let groups = described
            .get("logGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {name:?}: expected logGroups, got {described}"));
        assert_eq!(
            groups.len(),
            1,
            "case {name:?}: expected exactly the created group, got {described}"
        );
        let group = &groups[0];
        assert_eq!(
            group.get("logGroupName").and_then(Value::as_str),
            Some(*name),
            "case {name:?}: expected the created name, got {group}"
        );
        assert_eq!(
            group.get("arn").and_then(Value::as_str),
            Some(format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{name}:*").as_str()),
            "case {name:?}: expected the trailing-wildcard ARN, got {group}"
        );
        assert_eq!(
            group.get("logGroupArn").and_then(Value::as_str),
            Some(format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{name}").as_str()),
            "case {name:?}: expected the ARN without a trailing wildcard, got {group}"
        );
        assert_eq!(
            group.get("logGroupClass").and_then(Value::as_str),
            Some(class.unwrap_or("STANDARD")),
            "case {name:?}: expected the requested class (STANDARD by default), got {group}"
        );
        assert_eq!(
            group.get("storedBytes").and_then(Value::as_i64),
            Some(0),
            "case {name:?}: a group with no events stores no bytes, got {group}"
        );
        assert_eq!(
            group.get("metricFilterCount").and_then(Value::as_i64),
            Some(0),
            "case {name:?}: a new group has no metric filters, got {group}"
        );
        assert_eq!(
            group.get("deletionProtectionEnabled").and_then(Value::as_bool),
            Some(*protected),
            "case {name:?}: expected deletion protection {protected}, got {group}"
        );
        assert_eq!(
            group.get("kmsKeyId").and_then(Value::as_str),
            *kms,
            "case {name:?}: expected the requested KMS key, got {group}"
        );
        assert!(
            group.get("retentionInDays").is_none_or(Value::is_null),
            "case {name:?}: a new group has no retention policy, got {group}"
        );
        let creation = group.get("creationTime").and_then(Value::as_i64);
        assert!(
            creation.is_some_and(|time| time > 1_600_000_000_000),
            "case {name:?}: expected a wall-clock creationTime in ms, got {group}"
        );

        // Creating the same name twice is refused, whatever the rest of the request.
        let again = logs.err("CreateLogGroup", &json!({"logGroupName": name})).await;
        assert_eq!(
            again, "ResourceAlreadyExistsException",
            "case {name:?}: expected the duplicate name to be refused, got {again}"
        );
    }

    // Every created group is enumerated, and only those.
    let all = logs.ok("DescribeLogGroups", &json!({})).await;
    let listed: Vec<String> = all["logGroups"]
        .as_array()
        .expect("logGroups")
        .iter()
        .map(|group| group["logGroupName"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected: Vec<String> = cases.iter().map(|(name, ..)| (*name).to_string()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "DescribeLogGroups must enumerate exactly the created groups in name order, got {listed:?}"
    );
}

/// A log group name that breaks a documented naming guideline is refused rather than
/// stored, and a class outside the model's enum never reaches the handler at all.
#[tokio::test]
async fn log_group_creation_refuses_values_outside_the_documented_rules() {
    let logs = Logs::new();

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a name starting with aws/",
            json!({"logGroupName": "aws/reserved"}),
            "InvalidParameterException",
        ),
        (
            // `logGroupClass` is a modelled enum, so this is refused at the boundary —
            // and a boundary refusal carries the SERVED OPERATION's own refusal error,
            // which for `CreateLogGroup` is `InvalidParameterException` (the operation
            // does not declare `ValidationException` at all).
            "a class outside the enum",
            json!({"logGroupName": "classy", "logGroupClass": "GLACIER"}),
            "InvalidParameterException",
        ),
        (
            // `logGroupName` is `len 1..512, pattern ^[\.\-_/#A-Za-z0-9]+$`.
            "a name outside the modelled pattern",
            json!({"logGroupName": "has spaces"}),
            "InvalidParameterException",
        ),
        (
            "an empty name",
            json!({"logGroupName": ""}),
            "InvalidParameterException",
        ),
    ];
    for (case, request, expected) in &cases {
        let code = logs.err("CreateLogGroup", request).await;
        assert_eq!(
            code, *expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }

    // Nothing was stored by a refused create.
    let all = logs.ok("DescribeLogGroups", &json!({})).await;
    assert_eq!(
        all["logGroups"].as_array().map(Vec::len),
        Some(0),
        "a refused CreateLogGroup must store nothing, got {all}"
    );
}

/// What `PutLogEvents` writes is what `GetLogEvents` reads back: the same messages, the
/// same timestamps, ordered oldest-first, each stamped with an ingestion time — for
/// batches that differ in size, ordering and message content.
#[tokio::test]
async fn put_log_events_round_trips_through_get_log_events() {
    // Offsets from an in-window base, so the rows differ along batch size, ordering,
    // duplicate timestamps and message content without leaving the ingestion window.
    let cases: Vec<(&str, Vec<(i64, &str)>)> = vec![
        ("one event", vec![(0, "hello")]),
        ("two events", vec![(0, "hello"), (1_000, "world")]),
        (
            "duplicate timestamps keep ingestion order",
            vec![(42, "a"), (42, "b"), (42, "c")],
        ),
        (
            "unicode and punctuation",
            vec![(1, "héllo — ✓"), (2, "{\"json\":true}")],
        ),
        ("the base instant itself", vec![(0, "the beginning")]),
    ];

    for (case, offsets) in &cases {
        let base = base_ms();
        let events: Vec<(i64, &str)> = offsets
            .iter()
            .map(|(offset, message)| (base + offset, *message))
            .collect();
        let logs = Logs::new();
        group_with_stream(&logs, "/aws/emulator/demo", "stream-1", None).await;

        let put = logs
            .ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": "/aws/emulator/demo",
                    "logStreamName": "stream-1",
                    "logEvents": batch(&events),
                }),
            )
            .await;
        assert_eq!(
            put.get("rejectedLogEventsInfo"),
            None,
            "case {case:?}: an in-window batch is fully accepted, got {put}"
        );
        let token = put
            .get("nextSequenceToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a nextSequenceToken, got {put}"));
        assert!(
            !token.is_empty(),
            "case {case:?}: the sequence token must not be empty, got {put}"
        );

        let page = logs
            .ok(
                "GetLogEvents",
                &json!({
                    "logGroupName": "/aws/emulator/demo",
                    "logStreamName": "stream-1",
                    "startFromHead": true,
                }),
            )
            .await;

        let mut expected: Vec<(i64, &str)> = events.clone();
        expected.sort_by_key(|(timestamp, _)| *timestamp);
        let expected_messages: Vec<String> = expected
            .iter()
            .map(|(_, message)| (*message).to_string())
            .collect();
        assert_eq!(
            messages(&page),
            expected_messages,
            "case {case:?}: expected the written messages oldest-first, got {page}"
        );
        let returned = page["events"].as_array().expect("events");
        for (index, event) in returned.iter().enumerate() {
            assert_eq!(
                event.get("timestamp").and_then(Value::as_i64),
                Some(expected[index].0),
                "case {case:?} event {index}: expected the written timestamp, got {event}"
            );
            let ingestion = event.get("ingestionTime").and_then(Value::as_i64);
            assert!(
                ingestion.is_some_and(|time| time > 1_600_000_000_000),
                "case {case:?} event {index}: expected a wall-clock ingestionTime, got {event}"
            );
        }
        assert!(
            page.get("nextForwardToken")
                .and_then(Value::as_str)
                .is_some_and(|token| !token.is_empty()),
            "case {case:?}: the returned tokens are never null, got {page}"
        );
        assert!(
            page.get("nextBackwardToken")
                .and_then(Value::as_str)
                .is_some_and(|token| !token.is_empty()),
            "case {case:?}: the returned tokens are never null, got {page}"
        );

        // The stream summary reports the live values the write maintained.
        let described = logs
            .ok(
                "DescribeLogStreams",
                &json!({"logGroupName": "/aws/emulator/demo"}),
            )
            .await;
        let stream = &described["logStreams"][0];
        assert_eq!(
            stream.get("firstEventTimestamp").and_then(Value::as_i64),
            Some(expected.first().expect("a non-empty batch").0),
            "case {case:?}: expected the oldest event's timestamp, got {stream}"
        );
        assert_eq!(
            stream.get("lastEventTimestamp").and_then(Value::as_i64),
            Some(expected.last().expect("a non-empty batch").0),
            "case {case:?}: expected the newest event's timestamp, got {stream}"
        );
        assert_eq!(
            stream.get("uploadSequenceToken").and_then(Value::as_str),
            Some(token),
            "case {case:?}: the stream must carry the token PutLogEvents handed back, got {stream}"
        );
        assert!(
            stream
                .get("lastIngestionTime")
                .and_then(Value::as_i64)
                .is_some_and(|time| time > 1_600_000_000_000),
            "case {case:?}: expected a wall-clock lastIngestionTime, got {stream}"
        );
        assert_eq!(
            stream.get("arn").and_then(Value::as_str),
            Some(
                format!(
                    "arn:aws:logs:{REGION}:{ACCOUNT}:log-group:/aws/emulator/demo:log-stream:stream-1"
                )
                .as_str()
            ),
            "case {case:?}: expected the log stream ARN, got {stream}"
        );

        // The group's storedBytes grows by each message's UTF-8 size plus the
        // documented 26 bytes of per-event overhead.
        let expected_bytes: i64 = events
            .iter()
            .map(|(_, message)| i64::try_from(message.len()).expect("a small message") + 26)
            .sum();
        let groups = logs.ok("DescribeLogGroups", &json!({})).await;
        assert_eq!(
            groups["logGroups"][0].get("storedBytes").and_then(Value::as_i64),
            Some(expected_bytes),
            "case {case:?}: expected {expected_bytes} stored bytes, got {groups}"
        );

        // A second batch mints a different token, and both batches are readable.
        let second = logs
            .ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": "/aws/emulator/demo",
                    "logStreamName": "stream-1",
                    "logEvents": batch(&[(base + 7_200_000, "later")]),
                    "sequenceToken": "a token the service now ignores",
                }),
            )
            .await;
        let second_token = second["nextSequenceToken"].as_str().expect("a token");
        assert_ne!(
            second_token, token,
            "case {case:?}: each accepted batch mints a fresh sequence token, got {second}"
        );
        let after = logs
            .ok(
                "GetLogEvents",
                &json!({
                    "logGroupName": "/aws/emulator/demo",
                    "logStreamName": "stream-1",
                    "startFromHead": true,
                }),
            )
            .await;
        let mut all_expected = expected_messages.clone();
        all_expected.push("later".to_string());
        assert_eq!(
            messages(&after),
            all_expected,
            "case {case:?}: the second batch must append, got {after}"
        );
    }
}

/// A batch whose events predate what the stream already holds is merged into the
/// stream's timestamp order, not appended out of order.
#[tokio::test]
async fn a_late_batch_is_merged_into_the_stream_in_timestamp_order() {
    let logs = Logs::new();
    group_with_stream(&logs, "late", "stream", None).await;
    let base = base_ms();

    // Three batches: the middle one predates the first, the last straddles both.
    let batches: Vec<Vec<(i64, &str)>> = vec![
        vec![(base + 300, "c"), (base + 400, "d")],
        vec![(base + 100, "a")],
        vec![(base + 200, "b"), (base + 500, "e")],
    ];
    for (index, events) in batches.iter().enumerate() {
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": "late",
                "logStreamName": "stream",
                "logEvents": batch(events),
            }),
        )
        .await;
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({
                    "logGroupName": "late",
                    "logStreamName": "stream",
                    "startFromHead": true,
                }),
            )
            .await;
        let mut expected: Vec<(i64, &str)> = batches[..=index].iter().flatten().copied().collect();
        expected.sort_by_key(|(timestamp, _)| *timestamp);
        let expected_messages: Vec<String> = expected
            .iter()
            .map(|(_, message)| (*message).to_string())
            .collect();
        assert_eq!(
            messages(&page),
            expected_messages,
            "case after batch {index}: expected every event so far in timestamp order, got {page}"
        );
    }

    // The stream summary follows the merged order, not the arrival order.
    let streams = logs
        .ok("DescribeLogStreams", &json!({"logGroupName": "late"}))
        .await;
    let stream = &streams["logStreams"][0];
    assert_eq!(
        stream["firstEventTimestamp"].as_i64(),
        Some(base + 100),
        "expected the oldest merged event, got {stream}"
    );
    assert_eq!(
        stream["lastEventTimestamp"].as_i64(),
        Some(base + 500),
        "expected the newest merged event, got {stream}"
    );
}

/// A batch outside the constraints the operation documents is refused, and nothing it
/// carried is stored.
#[tokio::test]
async fn put_log_events_refuses_a_batch_outside_the_documented_constraints() {
    let logs = Logs::new();
    group_with_stream(&logs, "constraints", "stream", None).await;
    let base = base_ms();

    // "Each log event can be no larger than 1 MB" and "the maximum batch size is
    // 1,048,576 bytes … the sum of all event messages in UTF-8, plus 26 bytes for each
    // log event": one byte over each bound, and the largest batch that still fits.
    let over_event = "x".repeat(1_048_577);
    let half_batch = "y".repeat(524_288);
    let largest_fitting = "z".repeat(1_048_576 - 26);

    let refusals: Vec<(&str, Value)> = vec![
        (
            "events out of chronological order",
            json!({
                "logGroupName": "constraints",
                "logStreamName": "stream",
                "logEvents": batch(&[(base + 2, "second"), (base + 1, "first")]),
            }),
        ),
        (
            "an event one byte over the per-event ceiling",
            json!({
                "logGroupName": "constraints",
                "logStreamName": "stream",
                "logEvents": batch(&[(base + 1, over_event.as_str())]),
            }),
        ),
        (
            "a batch one event over the batch ceiling",
            json!({
                "logGroupName": "constraints",
                "logStreamName": "stream",
                "logEvents": batch(&[(base + 1, half_batch.as_str()), (base + 2, half_batch.as_str())]),
            }),
        ),
    ];
    for (case, request) in &refusals {
        let code = logs.err("PutLogEvents", request).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code}"
        );
    }

    // An unknown target is the more specific error, whatever else the batch breaks.
    let code = logs
        .err(
            "PutLogEvents",
            &json!({
                "logGroupName": "absent",
                "logStreamName": "stream",
                "logEvents": batch(&[(base + 1, over_event.as_str())]),
            }),
        )
        .await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case oversize batch into an unknown group: expected ResourceNotFoundException, got {code}"
    );

    let empty = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": "constraints", "logStreamName": "stream"}),
        )
        .await;
    assert_eq!(
        messages(&empty),
        Vec::<String>::new(),
        "a refused batch must store nothing, got {empty}"
    );

    // The batch that exactly fits is accepted.
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": "constraints",
            "logStreamName": "stream",
            "logEvents": batch(&[(base + 1, largest_fitting.as_str())]),
        }),
    )
    .await;
    let groups = logs.ok("DescribeLogGroups", &json!({})).await;
    assert_eq!(
        groups["logGroups"][0]["storedBytes"].as_i64(),
        Some(1_048_576),
        "the accepted batch's bytes must be counted, got {groups}"
    );
}

/// The emulator's per-stream storage ceiling holds: repeated writes keep the newest
/// events and discard the oldest, and the log group's `storedBytes` follows what is
/// actually stored.
#[tokio::test]
async fn a_stream_keeps_only_what_fits_in_the_storage_ceiling() {
    // The ceiling this test drives, from `crate::state::MAX_STREAM_EVENTS`.
    const MAX_STREAM_EVENTS: usize = 20_000;
    const BATCH: usize = 10_000;

    let logs = Logs::new();
    group_with_stream(&logs, "ceiling", "stream", None).await;
    let base = base_ms();

    // Three full batches: 30,000 events written, 20,000 of them keepable.
    for round in 0..3_usize {
        let events: Vec<Value> = (0..BATCH)
            .map(|index| {
                let ordinal = round * BATCH + index;
                let offset = i64::try_from(ordinal).expect("a small ordinal");
                json!({"timestamp": base + offset, "message": format!("e{ordinal}")})
            })
            .collect();
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": "ceiling",
                "logStreamName": "stream",
                "logEvents": Value::Array(events),
            }),
        )
        .await;
    }

    // The newest events survive; the oldest were discarded to stay inside the ceiling.
    let newest = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": "ceiling", "logStreamName": "stream", "limit": 1}),
        )
        .await;
    assert_eq!(
        messages(&newest),
        vec![format!("e{}", 3 * BATCH - 1)],
        "expected the most recent event to be kept, got {newest}"
    );
    let oldest = logs
        .ok(
            "GetLogEvents",
            &json!({
                "logGroupName": "ceiling",
                "logStreamName": "stream",
                "startFromHead": true,
                "limit": 1,
            }),
        )
        .await;
    assert_eq!(
        messages(&oldest),
        vec![format!("e{}", 3 * BATCH - MAX_STREAM_EVENTS)],
        "expected the oldest kept event to sit at the ceiling, got {oldest}"
    );

    // Exactly the ceiling is held, and `storedBytes` counts what is held, not what was
    // ever written.
    let mut held = 0_usize;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({
            "logGroupName": "ceiling",
            "logStreamName": "stream",
            "startFromHead": true,
            "limit": 10_000,
        });
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = logs.ok("GetLogEvents", &request).await;
        let count = page["events"].as_array().map_or(0, Vec::len);
        if count == 0 {
            break;
        }
        held += count;
        token = Some(page["nextForwardToken"].as_str().expect("a token").to_string());
    }
    assert_eq!(
        held, MAX_STREAM_EVENTS,
        "expected the stream to hold exactly the ceiling, got {held}"
    );

    let groups = logs.ok("DescribeLogGroups", &json!({})).await;
    let stored = groups["logGroups"][0]["storedBytes"].as_i64();
    let expected: i64 = (BATCH..3 * BATCH)
        .map(|ordinal| i64::try_from(format!("e{ordinal}").len()).expect("a small message") + 26)
        .sum();
    assert_eq!(
        stored,
        Some(expected),
        "storedBytes must count the events still held, got {groups}"
    );
}

/// `DescribeLogGroups` searches an account: this emulator's own by default, and only
/// the accounts a cross-account request names.
#[tokio::test]
async fn describe_log_groups_searches_only_the_accounts_the_request_names() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "local"})).await;

    let cases: Vec<(&str, Value, usize)> = vec![
        ("no cross-account members", json!({}), 1),
        (
            "linked accounts off, other account named",
            json!({"includeLinkedAccounts": false, "accountIdentifiers": ["111122223333"]}),
            1,
        ),
        (
            "linked accounts on, this account named",
            json!({"includeLinkedAccounts": true, "accountIdentifiers": [ACCOUNT]}),
            1,
        ),
        (
            "linked accounts on, no account named",
            json!({"includeLinkedAccounts": true}),
            1,
        ),
        (
            "linked accounts on, only another account named",
            json!({"includeLinkedAccounts": true, "accountIdentifiers": ["111122223333"]}),
            0,
        ),
    ];
    for (case, request, expected) in &cases {
        let page = logs.ok("DescribeLogGroups", request).await;
        assert_eq!(
            page["logGroups"].as_array().map(Vec::len),
            Some(*expected),
            "case {case:?}: expected {expected} log group(s), got {page}"
        );
    }
}

/// Paging `GetLogEvents` forward from the head, and backward from the tail, visits every
/// event exactly once in timestamp order — for every event count and page size in a
/// bounded grid (an exhaustive check of the invariant, not a sample of it).
#[tokio::test]
async fn get_log_events_pagination_visits_every_event_exactly_once() {
    for count in 0_usize..=5 {
        for limit in 1_usize..=3 {
            let logs = Logs::new();
            group_with_stream(&logs, "pagination", "stream", None).await;
            let base = base_ms();
            let events: Vec<(i64, String)> = (0..count)
                .map(|index| {
                    (
                        base + i64::try_from(index).expect("a small index") * 1_000,
                        format!("event-{index}"),
                    )
                })
                .collect();
            if count > 0 {
                let wire: Vec<(i64, &str)> = events
                    .iter()
                    .map(|(timestamp, message)| (*timestamp, message.as_str()))
                    .collect();
                logs.ok(
                    "PutLogEvents",
                    &json!({
                        "logGroupName": "pagination",
                        "logStreamName": "stream",
                        "logEvents": batch(&wire),
                    }),
                )
                .await;
            }
            let expected: Vec<String> = events.iter().map(|(_, message)| message.clone()).collect();
            let case = format!("{count} events, limit {limit}");

            // Forward from the head.
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for _ in 0..(count + 3) {
                let mut request = json!({
                    "logGroupName": "pagination",
                    "logStreamName": "stream",
                    "startFromHead": true,
                    "limit": limit,
                });
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let page = logs.ok("GetLogEvents", &request).await;
                let page_messages = messages(&page);
                let next = page["nextForwardToken"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a forward token, got {page}"))
                    .to_string();
                assert!(
                    page_messages.len() <= limit,
                    "case {case:?}: a page must not exceed the limit, got {page}"
                );
                if page_messages.is_empty() {
                    assert_eq!(
                        token.as_deref(),
                        token.is_some().then_some(next.as_str()),
                        "case {case:?}: an exhausted forward scan repeats its token, got {page}"
                    );
                    break;
                }
                collected.extend(page_messages);
                token = Some(next);
            }
            assert_eq!(
                collected, expected,
                "case {case:?}: forward paging must visit every event once, in order"
            );

            // Backward from the tail.
            let mut backward: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for _ in 0..(count + 3) {
                let mut request = json!({
                    "logGroupName": "pagination",
                    "logStreamName": "stream",
                    "limit": limit,
                });
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let page = logs.ok("GetLogEvents", &request).await;
                let page_messages = messages(&page);
                let next = page["nextBackwardToken"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a backward token, got {page}")
                    })
                    .to_string();
                if page_messages.is_empty() {
                    break;
                }
                let mut head = page_messages;
                head.extend(backward);
                backward = head;
                token = Some(next);
            }
            assert_eq!(
                backward, expected,
                "case {case:?}: backward paging must visit every event once, in order"
            );
        }
    }
}

/// `startTime` is inclusive, `endTime` exclusive, and the page is taken from the head or
/// the tail according to `startFromHead`.
#[tokio::test]
async fn get_log_events_honours_the_time_window_and_which_end_it_reads_from() {
    let logs = Logs::new();
    group_with_stream(&logs, "window", "stream", None).await;
    let base = base_ms();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": "window",
            "logStreamName": "stream",
            "logEvents": batch(&[
                (base + 10, "a"),
                (base + 20, "b"),
                (base + 30, "c"),
                (base + 40, "d"),
            ]),
        }),
    )
    .await;

    // (startTime, endTime) offsets from the base -> the events in the window
    let windows: Vec<(Option<i64>, Option<i64>, Vec<&str>)> = vec![
        (None, None, vec!["a", "b", "c", "d"]),
        (Some(20), None, vec!["b", "c", "d"]),
        (None, Some(30), vec!["a", "b"]),
        (Some(20), Some(40), vec!["b", "c"]),
        (Some(20), Some(20), vec![]),
        (Some(41), None, vec![]),
        (Some(0), Some(11), vec!["a"]),
    ];
    for (start, end, expected) in &windows {
        let mut request = json!({
            "logGroupName": "window",
            "logStreamName": "stream",
            "startFromHead": true,
        });
        if let Some(start) = start {
            request["startTime"] = json!(base + start);
        }
        if let Some(end) = end {
            request["endTime"] = json!(base + end);
        }
        let page = logs.ok("GetLogEvents", &request).await;
        assert_eq!(
            messages(&page),
            *expected,
            "case {start:?}..{end:?}: expected exactly the events in the window, got {page}"
        );
    }

    // Which end the first page is taken from, at a limit smaller than the stream.
    let from_head = logs
        .ok(
            "GetLogEvents",
            &json!({
                "logGroupName": "window",
                "logStreamName": "stream",
                "startFromHead": true,
                "limit": 2,
            }),
        )
        .await;
    assert_eq!(
        messages(&from_head),
        vec!["a", "b"],
        "case startFromHead=true: expected the earliest events, got {from_head}"
    );
    let from_tail = logs
        .ok(
            "GetLogEvents",
            &json!({
                "logGroupName": "window",
                "logStreamName": "stream",
                "limit": 2,
            }),
        )
        .await;
    assert_eq!(
        messages(&from_tail),
        vec!["c", "d"],
        "case startFromHead absent: expected the latest events, got {from_tail}"
    );

    // A continuation keeps the scan's own window: omitting the time members is fine,
    // contradicting them is refused rather than answered over a different window.
    let first = logs
        .ok(
            "GetLogEvents",
            &json!({
                "logGroupName": "window",
                "logStreamName": "stream",
                "startFromHead": true,
                "startTime": base + 20,
                "endTime": base + 40,
                "limit": 1,
            }),
        )
        .await;
    assert_eq!(
        messages(&first),
        vec!["b"],
        "case windowed first page: expected the first event in the window, got {first}"
    );
    let token = first["nextForwardToken"].as_str().expect("a forward token");
    let second = logs
        .ok(
            "GetLogEvents",
            &json!({
                "logGroupName": "window",
                "logStreamName": "stream",
                "startFromHead": true,
                "nextToken": token,
                "limit": 1,
            }),
        )
        .await;
    assert_eq!(
        messages(&second),
        vec!["c"],
        "case windowed continuation: expected the next event in the same window, got {second}"
    );
    let code = logs
        .err(
            "GetLogEvents",
            &json!({
                "logGroupName": "window",
                "logStreamName": "stream",
                "startFromHead": true,
                "nextToken": token,
                "startTime": base,
            }),
        )
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case continuation with a different window: expected InvalidParameterException, got {code}"
    );
}

/// A log group is named by `logGroupName`, or by `logGroupIdentifier` in any of the
/// forms the model documents; naming neither or both is refused.
#[tokio::test]
async fn a_log_group_is_reachable_by_name_and_by_either_arn_form() {
    let logs = Logs::new();
    group_with_stream(&logs, "identifiers", "stream", None).await;
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": "identifiers",
            "logStreamName": "stream",
            "logEvents": batch(&[(base_ms() + 7, "only")]),
        }),
    )
    .await;

    let identifiers = vec![
        ("the bare name", "identifiers".to_string()),
        (
            // The `logGroupArn` version, "the ARN [that] doesn't include a trailing :*".
            // The wildcard version is not a legal `logGroupIdentifier`: the member's
            // modelled pattern `^[\w#+=/:,.@-]*$` admits no `*`.
            "the ARN without a wildcard",
            format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:identifiers"),
        ),
    ];
    for (case, identifier) in &identifiers {
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupIdentifier": identifier, "logStreamName": "stream"}),
            )
            .await;
        assert_eq!(
            messages(&page),
            vec!["only"],
            "case {case:?}: expected the stream's events, got {page}"
        );
        let streams = logs
            .ok("DescribeLogStreams", &json!({"logGroupIdentifier": identifier}))
            .await;
        assert_eq!(
            streams["logStreams"][0]["logStreamName"].as_str(),
            Some("stream"),
            "case {case:?}: expected the group's streams, got {streams}"
        );
    }

    let refusals: Vec<(&str, Value)> = vec![
        (
            "neither member",
            json!({"logStreamName": "stream"}),
        ),
        (
            "both members",
            json!({
                "logGroupName": "identifiers",
                "logGroupIdentifier": "identifiers",
                "logStreamName": "stream",
            }),
        ),
    ];
    for (case, request) in &refusals {
        // The log-group selector pair is checked ACROSS two members, which no single
        // member's constraint can express, and the service answers that check with
        // `ValidationException` (400) rather than with this operation's
        // `InvalidParameterException` — see `crate::state::LOG_GROUP_SELECTORS_EXCLUSIVE`.
        let code = logs.err("GetLogEvents", request).await;
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code}"
        );
    }
}

/// `DescribeLogStreams` lists the group's streams in the requested order, filtered by
/// prefix, and pages through all of them.
#[tokio::test]
async fn describe_log_streams_orders_filters_and_pages() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "streams"})).await;

    // Created in an order that matches neither sort, with event times that invert the
    // name order, so each ordering is distinguishable.
    let base = base_ms();
    let streams: Vec<(&str, Option<i64>)> = vec![
        ("beta", Some(base + 300)),
        ("alpha", Some(base + 400)),
        ("delta", None),
        ("charlie", Some(base + 100)),
    ];
    for (name, last_event) in &streams {
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": "streams", "logStreamName": name}),
        )
        .await;
        if let Some(timestamp) = last_event {
            logs.ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": "streams",
                    "logStreamName": name,
                    "logEvents": batch(&[(*timestamp, "event")]),
                }),
            )
            .await;
        }
    }

    // (orderBy, descending) -> the names in the order they must come back
    let orders: Vec<(Option<&str>, Option<bool>, Vec<&str>)> = vec![
        (None, None, vec!["alpha", "beta", "charlie", "delta"]),
        (
            Some("LogStreamName"),
            Some(false),
            vec!["alpha", "beta", "charlie", "delta"],
        ),
        (
            Some("LogStreamName"),
            Some(true),
            vec!["delta", "charlie", "beta", "alpha"],
        ),
        (
            Some("LastEventTime"),
            Some(false),
            vec!["delta", "charlie", "beta", "alpha"],
        ),
        (
            Some("LastEventTime"),
            Some(true),
            vec!["alpha", "beta", "charlie", "delta"],
        ),
    ];
    for (order_by, descending, expected) in &orders {
        let case = format!("orderBy {order_by:?}, descending {descending:?}");
        let mut request = json!({"logGroupName": "streams"});
        if let Some(order_by) = order_by {
            request["orderBy"] = json!(order_by);
        }
        if let Some(descending) = descending {
            request["descending"] = json!(descending);
        }
        let page = logs.ok("DescribeLogStreams", &request).await;
        let names: Vec<String> = page["logStreams"]
            .as_array()
            .expect("logStreams")
            .iter()
            .map(|stream| stream["logStreamName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, *expected,
            "case {case:?}: expected that order, got {names:?}"
        );

        // The same order, one page at a time, covers exactly the same list.
        let mut paged: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = request.clone();
            request["limit"] = json!(1);
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = logs.ok("DescribeLogStreams", &request).await;
            let names = page["logStreams"].as_array().expect("logStreams");
            assert!(
                names.len() <= 1,
                "case {case:?}: a page must not exceed the limit, got {page}"
            );
            paged.extend(
                names
                    .iter()
                    .map(|stream| stream["logStreamName"].as_str().unwrap_or_default().to_string()),
            );
            match page["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            paged, *expected,
            "case {case:?}: paging must cover the same list in the same order"
        );
    }

    // The prefix filter, and the documented refusal of a prefix ordered by event time.
    let prefixes: Vec<(&str, Vec<&str>)> = vec![
        ("a", vec!["alpha"]),
        ("c", vec!["charlie"]),
        ("delta", vec!["delta"]),
        ("z", vec![]),
    ];
    for (prefix, expected) in &prefixes {
        let page = logs
            .ok(
                "DescribeLogStreams",
                &json!({"logGroupName": "streams", "logStreamNamePrefix": prefix}),
            )
            .await;
        let names: Vec<String> = page["logStreams"]
            .as_array()
            .expect("logStreams")
            .iter()
            .map(|stream| stream["logStreamName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, *expected,
            "case prefix {prefix:?}: expected only the matching streams, got {names:?}"
        );
    }
    let code = logs
        .err(
            "DescribeLogStreams",
            &json!({
                "logGroupName": "streams",
                "logStreamNamePrefix": "a",
                "orderBy": "LastEventTime",
            }),
        )
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case prefix with orderBy=LastEventTime: expected InvalidParameterException, got {code}"
    );

    // A fresh stream reports its creation time and no event times at all.
    let page = logs
        .ok(
            "DescribeLogStreams",
            &json!({"logGroupName": "streams", "logStreamNamePrefix": "delta"}),
        )
        .await;
    let stream = &page["logStreams"][0];
    assert!(
        stream["creationTime"].as_i64().is_some_and(|time| time > 1_600_000_000_000),
        "case delta: expected a wall-clock creationTime, got {stream}"
    );
    for absent in ["firstEventTimestamp", "lastEventTimestamp", "uploadSequenceToken"] {
        assert!(
            stream.get(absent).is_none_or(Value::is_null),
            "case delta: a stream with no events reports no {absent}, got {stream}"
        );
    }
    assert_eq!(
        stream["storedBytes"].as_i64(),
        Some(0),
        "case delta: a log stream's storedBytes is always reported as zero, got {stream}"
    );
}

/// `DescribeLogGroups` filters by prefix, by case-sensitive substring pattern, by class
/// and by identifier, and pages through the result.
#[tokio::test]
async fn describe_log_groups_filters_and_pages() {
    let logs = Logs::new();
    let groups: Vec<(&str, &str)> = vec![
        ("DataLogs", "STANDARD"),
        ("aws/DataLogs", "INFREQUENT_ACCESS"),
        ("GroupDataLogs", "STANDARD"),
        ("datalogs", "DELIVERY"),
        ("other", "STANDARD"),
    ];
    for (name, class) in &groups {
        // `aws/DataLogs` is refused as a name, so it is created under a legal prefix
        // that still contains the pattern.
        let name = if *name == "aws/DataLogs" { "x/aws/DataLogs" } else { name };
        logs.ok(
            "CreateLogGroup",
            &json!({"logGroupName": name, "logGroupClass": class}),
        )
        .await;
    }

    // (filter, expected names)
    let filters: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "prefix",
            json!({"logGroupNamePrefix": "Data"}),
            vec!["DataLogs"],
        ),
        (
            "prefix matching none",
            json!({"logGroupNamePrefix": "zzz"}),
            vec![],
        ),
        (
            // "if you specify DataLogs, log groups named DataLogs, aws/DataLogs and
            // GroupDataLogs would match, but datalogs would not".
            "case-sensitive substring pattern",
            json!({"logGroupNamePattern": "DataLogs"}),
            vec!["DataLogs", "GroupDataLogs", "x/aws/DataLogs"],
        ),
        (
            "pattern with the other case",
            json!({"logGroupNamePattern": "datalogs"}),
            vec!["datalogs"],
        ),
        (
            "class",
            json!({"logGroupClass": "STANDARD"}),
            vec!["DataLogs", "GroupDataLogs", "other"],
        ),
        (
            "class with no members",
            json!({"logGroupClass": "INFREQUENT_ACCESS"}),
            vec!["x/aws/DataLogs"],
        ),
        (
            "identifiers by name and by ARN",
            json!({"logGroupIdentifiers": [
                "other",
                format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:datalogs"),
            ]}),
            vec!["datalogs", "other"],
        ),
    ];
    for (case, filter, expected) in &filters {
        let page = logs.ok("DescribeLogGroups", filter).await;
        let names: Vec<String> = page["logGroups"]
            .as_array()
            .expect("logGroups")
            .iter()
            .map(|group| group["logGroupName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, *expected,
            "case {case:?}: expected exactly those groups in name order, got {names:?}"
        );
    }

    // A pattern search returns the reduced projection: identity and classification only.
    let page = logs
        .ok("DescribeLogGroups", &json!({"logGroupNamePattern": "other"}))
        .await;
    let group = &page["logGroups"][0];
    for present in [
        "arn",
        "creationTime",
        "logGroupArn",
        "logGroupClass",
        "logGroupName",
        "metricFilterCount",
    ] {
        assert!(
            group.get(present).is_some_and(|value| !value.is_null()),
            "case pattern search: expected {present}, got {group}"
        );
    }
    for absent in ["storedBytes", "retentionInDays", "kmsKeyId"] {
        assert!(
            group.get(absent).is_none_or(Value::is_null),
            "case pattern search: expected no {absent}, got {group}"
        );
    }

    // The two name filters are mutually exclusive.
    let code = logs
        .err(
            "DescribeLogGroups",
            &json!({"logGroupNamePrefix": "Data", "logGroupNamePattern": "Data"}),
        )
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case prefix and pattern together: expected InvalidParameterException, got {code}"
    );

    // Paging at every page size covers the whole enumeration exactly once.
    let all: Vec<String> = logs.ok("DescribeLogGroups", &json!({})).await["logGroups"]
        .as_array()
        .expect("logGroups")
        .iter()
        .map(|group| group["logGroupName"].as_str().unwrap_or_default().to_string())
        .collect();
    for limit in 1..=6 {
        let mut paged: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"limit": limit});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = logs.ok("DescribeLogGroups", &request).await;
            let names = page["logGroups"].as_array().expect("logGroups");
            assert!(
                names.len() <= limit,
                "case limit {limit}: a page must not exceed the limit, got {page}"
            );
            paged.extend(
                names
                    .iter()
                    .map(|group| group["logGroupName"].as_str().unwrap_or_default().to_string()),
            );
            match page["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            paged, all,
            "case limit {limit}: paging must cover the whole enumeration once, in order"
        );
    }

    // A token from one scan is not a position in another.
    let first = logs.ok("DescribeLogGroups", &json!({"limit": 1})).await;
    let token = first["nextToken"].as_str().expect("a nextToken").to_string();
    let code = logs
        .err(
            "DescribeLogGroups",
            &json!({"limit": 1, "nextToken": token, "logGroupNamePrefix": "Data"}),
        )
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case token reused under a different filter: expected InvalidParameterException, got {code}"
    );
    let code = logs
        .err("DescribeLogGroups", &json!({"nextToken": "not-a-token"}))
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case malformed token: expected InvalidParameterException, got {code}"
    );
}

/// Deleting a log group removes it, its streams and its events; deleting one that is not
/// there, or one that is protected, is refused.
#[tokio::test]
async fn deleting_a_log_group_removes_everything_stored_under_it() {
    let logs = Logs::new();
    group_with_stream(&logs, "/aws/emulator/demo", "stream-1", None).await;
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": "/aws/emulator/demo",
            "logStreamName": "stream-1",
            "logEvents": batch(&[(base_ms(), "hello")]),
        }),
    )
    .await;
    logs.ok("CreateLogGroup", &json!({"logGroupName": "kept"})).await;

    logs.ok(
        "DeleteLogGroup",
        &json!({"logGroupName": "/aws/emulator/demo"}),
    )
    .await;

    let remaining: Vec<String> = logs.ok("DescribeLogGroups", &json!({})).await["logGroups"]
        .as_array()
        .expect("logGroups")
        .iter()
        .map(|group| group["logGroupName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        remaining,
        vec!["kept".to_string()],
        "a deleted group must not be listed, got {remaining:?}"
    );

    // Its streams and events went with it.
    let gone: Vec<(&str, &str, Value)> = vec![
        (
            "GetLogEvents",
            "GetLogEvents",
            json!({"logGroupName": "/aws/emulator/demo", "logStreamName": "stream-1"}),
        ),
        (
            "CreateLogStream",
            "CreateLogStream",
            json!({"logGroupName": "/aws/emulator/demo", "logStreamName": "stream-2"}),
        ),
        (
            "DescribeLogStreams",
            "DescribeLogStreams",
            json!({"logGroupName": "/aws/emulator/demo"}),
        ),
        (
            "PutLogEvents",
            "PutLogEvents",
            json!({
                "logGroupName": "/aws/emulator/demo",
                "logStreamName": "stream-1",
                "logEvents": batch(&[(1, "x")]),
            }),
        ),
        (
            "DeleteLogGroup again",
            "DeleteLogGroup",
            json!({"logGroupName": "/aws/emulator/demo"}),
        ),
    ];
    for (case, operation, request) in &gone {
        let code = logs.err(operation, request).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code}"
        );
    }

    // Re-creating the deleted name starts from an empty group.
    logs.ok(
        "CreateLogGroup",
        &json!({"logGroupName": "/aws/emulator/demo"}),
    )
    .await;
    let streams = logs
        .ok(
            "DescribeLogStreams",
            &json!({"logGroupName": "/aws/emulator/demo"}),
        )
        .await;
    assert_eq!(
        streams["logStreams"].as_array().map(Vec::len),
        Some(0),
        "a re-created group starts with no streams, got {streams}"
    );

    // Deletion protection blocks the delete until it is disabled.
    logs.ok(
        "CreateLogGroup",
        &json!({"logGroupName": "protected", "deletionProtectionEnabled": true}),
    )
    .await;
    let code = logs
        .err("DeleteLogGroup", &json!({"logGroupName": "protected"}))
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case deletion protection: expected the delete to be blocked, got {code}"
    );
    let still_there = logs
        .ok("DescribeLogGroups", &json!({"logGroupNamePrefix": "protected"}))
        .await;
    assert_eq!(
        still_there["logGroups"].as_array().map(Vec::len),
        Some(1),
        "a blocked delete must leave the group in place, got {still_there}"
    );
}

/// Every operation of the loop refuses a resource that was never created, and a stream
/// that collides with one that was.
#[tokio::test]
async fn unknown_and_duplicate_resources_are_refused() {
    let logs = Logs::new();
    group_with_stream(&logs, "present", "stream", None).await;

    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "CreateLogStream in an unknown group",
            "CreateLogStream",
            json!({"logGroupName": "absent", "logStreamName": "stream"}),
            "ResourceNotFoundException",
        ),
        (
            "CreateLogStream that already exists",
            "CreateLogStream",
            json!({"logGroupName": "present", "logStreamName": "stream"}),
            "ResourceAlreadyExistsException",
        ),
        (
            "PutLogEvents into an unknown group",
            "PutLogEvents",
            json!({
                "logGroupName": "absent",
                "logStreamName": "stream",
                "logEvents": batch(&[(1, "x")]),
            }),
            "ResourceNotFoundException",
        ),
        (
            "PutLogEvents into an unknown stream",
            "PutLogEvents",
            json!({
                "logGroupName": "present",
                "logStreamName": "absent",
                "logEvents": batch(&[(1, "x")]),
            }),
            "ResourceNotFoundException",
        ),
        (
            "GetLogEvents from an unknown group",
            "GetLogEvents",
            json!({"logGroupName": "absent", "logStreamName": "stream"}),
            "ResourceNotFoundException",
        ),
        (
            "GetLogEvents from an unknown stream",
            "GetLogEvents",
            json!({"logGroupName": "present", "logStreamName": "absent"}),
            "ResourceNotFoundException",
        ),
        (
            "DescribeLogStreams of an unknown group",
            "DescribeLogStreams",
            json!({"logGroupName": "absent"}),
            "ResourceNotFoundException",
        ),
        (
            "DeleteLogGroup of an unknown group",
            "DeleteLogGroup",
            json!({"logGroupName": "absent"}),
            "ResourceNotFoundException",
        ),
        (
            "GetLogEvents with a token from another stream",
            "GetLogEvents",
            json!({
                "logGroupName": "present",
                "logStreamName": "stream",
                "nextToken": "not-a-token",
            }),
            "InvalidParameterException",
        ),
    ];

    for (case, operation, request, expected) in &cases {
        let code = logs.err(operation, request).await;
        assert_eq!(
            code, *expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }
}

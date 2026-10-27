// SPDX-License-Identifier: CC0-1.0

//! The envelope `GetLogEvents` cursors travel in, and what the service answers when a
//! client hands one back.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: a `GetLogEvents` cursor is a direction-prefixed envelope —
//! `f/<body>` for the cursor that scans towards newer events, `b/<body>` for the one
//! that scans towards older events, with a trailing `/s` once that scan has reached the
//! end of the stream — the two cursors of one page name their own ends of it, and a
//! cursor in that envelope which the store cannot place is an exhausted scan, answered
//! with an empty page rather than refused.
//!
//! The tables below span what the rule generalises over (event count, page size, which
//! end the scan starts from, direction, sentinel, body) rather than any one cursor value:
//! a cursor body is opaque, so no case asserts one.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, group_with_stream, Logs};

/// The `message` values of a `GetLogEvents` page, in the order returned.
fn messages(page: &Value) -> Vec<String> {
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

/// One cursor of a page, as the envelope decomposes it: the direction prefix, the opaque
/// body, and whether the end-of-stream sentinel is present.
fn envelope(case: &str, token: &str) -> (String, String, bool) {
    let (direction, rest) = token
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: a cursor carries a direction prefix, got {token:?}"));
    let (body, end_of_stream) = match rest.strip_suffix("/s") {
        Some(body) => (body, true),
        None => (rest, false),
    };
    assert!(
        !body.is_empty() && !body.contains('/'),
        "case {case:?}: a cursor carries one opaque body, got {token:?}"
    );
    (direction.to_string(), body.to_string(), end_of_stream)
}

/// Both of a page's cursors, each decomposed by [`envelope`], with their direction
/// prefixes already asserted.
fn cursors(case: &str, page: &Value) -> ((String, bool), (String, bool)) {
    let read = |member: &str, expected_direction: &str| {
        let token = page
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: {member} is never null, got {page}"));
        let (direction, body, end_of_stream) = envelope(case, token);
        assert_eq!(
            direction, expected_direction,
            "case {case:?}: {member} carries the {expected_direction:?} prefix, got {token:?}"
        );
        (body, end_of_stream)
    };
    (
        read("nextForwardToken", "f"),
        read("nextBackwardToken", "b"),
    )
}

/// A group with one stream holding `count` events one second apart, named `event-<i>`,
/// returned with the base timestamp the first of them was stamped at.
///
/// A window a case means to place relative to those events is derived from THIS base: a
/// second clock reading is a different instant, so a bound taken from one would sit a few
/// milliseconds away from where the case means it, and `GetLogEvents` selects on the
/// stored timestamps.
async fn stream_of(logs: &Logs, group: &str, count: usize) -> (i64, Vec<String>) {
    group_with_stream(logs, group, "stream", None).await;
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
                "logGroupName": group,
                "logStreamName": "stream",
                "logEvents": batch(&wire),
            }),
        )
        .await;
    }
    (
        base,
        events.into_iter().map(|(_, message)| message).collect(),
    )
}

/// Both cursors of every page carry their own direction prefix, the sentinel appears
/// exactly on the side the window holds nothing further on, and the forward cursor still
/// resumes the scan where the page stopped — over a grid of event counts, page sizes and
/// starting ends.
#[tokio::test]
async fn get_log_events_cursors_carry_their_direction_and_end_of_stream_envelope() {
    for count in 0_usize..=4 {
        for limit in 1_usize..=3 {
            for from_head in [true, false] {
                let logs = Logs::new();
                let (_, expected) = stream_of(&logs, "envelope", count).await;
                let case = format!("{count} events, limit {limit}, startFromHead {from_head}");

                // Where the first page starts: the head, or the last `limit` events.
                let mut page_start = if from_head {
                    0
                } else {
                    count.saturating_sub(limit)
                };
                let first_start = page_start;
                let mut token: Option<String> = None;
                for round in 0..(count + 3) {
                    let mut request = json!({
                        "logGroupName": "envelope",
                        "logStreamName": "stream",
                        "startFromHead": from_head,
                        "limit": limit,
                    });
                    if let Some(token) = &token {
                        request["nextToken"] = json!(token);
                    }
                    let page = logs.ok("GetLogEvents", &request).await;
                    let ((forward_body, forward_end), (backward_body, backward_end)) =
                        cursors(&case, &page);
                    let page_messages = messages(&page);
                    let page_end = page_start + page_messages.len();
                    assert_eq!(
                        page_messages,
                        expected[page_start..page_end],
                        "case {case:?} round {round}: the page is the next slice of the window"
                    );

                    // The two cursors name the two ends of the page, so a page that
                    // spans more than one position distinguishes them.
                    if page_messages.len() > 1 {
                        assert_ne!(
                            forward_body, backward_body,
                            "case {case:?} round {round}: a page's cursors name its two ends, got {page}"
                        );
                    }
                    // The sentinel says "nothing further this way".
                    assert_eq!(
                        forward_end,
                        page_end == count,
                        "case {case:?} round {round}: the forward sentinel marks the end of the stream, got {page}"
                    );
                    assert_eq!(
                        backward_end,
                        page_start == 0,
                        "case {case:?} round {round}: the backward sentinel marks the head of the stream, got {page}"
                    );

                    let next = page["nextForwardToken"]
                        .as_str()
                        .expect("a forward cursor")
                        .to_string();
                    if page_messages.is_empty() {
                        // An exhausted scan hands back the cursor it was called with.
                        if let Some(token) = &token {
                            assert_eq!(
                                &next, token,
                                "case {case:?} round {round}: an exhausted scan repeats its cursor, got {page}"
                            );
                        }
                        break;
                    }
                    page_start = page_end;
                    token = Some(next);
                }
                assert_eq!(
                    page_start, count,
                    "case {case:?}: forward paging from {first_start} must reach the end of the stream"
                );
            }
        }
    }
}

/// The backward cursor walks towards older events: following it from the tail visits
/// every event exactly once, in order, and the head page carries the sentinel.
#[tokio::test]
async fn the_backward_cursor_walks_towards_older_events() {
    for count in 1_usize..=4 {
        for limit in 1_usize..=3 {
            let logs = Logs::new();
            let (_, expected) = stream_of(&logs, "backward", count).await;
            let case = format!("{count} events, limit {limit}");

            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for _ in 0..(count + 3) {
                let mut request = json!({
                    "logGroupName": "backward",
                    "logStreamName": "stream",
                    "limit": limit,
                });
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let page = logs.ok("GetLogEvents", &request).await;
                let (_, (_, backward_end)) = cursors(&case, &page);
                let page_messages = messages(&page);
                if page_messages.is_empty() {
                    break;
                }
                let mut head = page_messages;
                head.extend(walked);
                walked = head;
                token = Some(
                    page["nextBackwardToken"]
                        .as_str()
                        .expect("a backward cursor")
                        .to_string(),
                );
                if backward_end {
                    break;
                }
            }
            assert_eq!(
                walked, expected,
                "case {case:?}: backward paging visits every event once, in order"
            );
        }
    }
}

/// A cursor in the service's envelope whose body this store cannot place — one minted
/// elsewhere, or the zero position an end-of-stream page reports — is an exhausted scan:
/// 200 with an empty page, echoing that cursor back in both directions.
#[tokio::test]
async fn a_cursor_the_store_cannot_place_is_an_empty_page() {
    let logs = Logs::new();
    stream_of(&logs, "exhausted", 3).await;

    // direction prefix × sentinel × body shape: the zero-position body an end-of-stream
    // page reports is one row, not the rule.
    let bodies: Vec<(&str, &str)> = vec![
        (
            "the zero position, forward",
            "f/00000000000000000000000000000000000000000000000000000000/s",
        ),
        (
            "the zero position, backward",
            "b/00000000000000000000000000000000000000000000000000000000/s",
        ),
        ("a foreign forward cursor at the end", "f/374182417234885632/s"),
        ("a foreign forward cursor mid-stream", "f/374182417234885632"),
        ("a foreign backward cursor", "b/9"),
        ("a foreign backward cursor at the end", "b/deadBEEF0123/s"),
    ];
    for (case, token) in &bodies {
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({
                    "logGroupName": "exhausted",
                    "logStreamName": "stream",
                    "nextToken": token,
                }),
            )
            .await;
        assert_eq!(
            messages(&page),
            Vec::<String>::new(),
            "case {case:?}: an exhausted cursor is an empty page, got {page}"
        );
        let (_, body, _) = envelope(case, token);
        assert_eq!(
            page["nextForwardToken"].as_str(),
            Some(format!("f/{body}/s").as_str()),
            "case {case:?}: the forward cursor is handed back at the end of the stream, got {page}"
        );
        assert_eq!(
            page["nextBackwardToken"].as_str(),
            Some(format!("b/{body}/s").as_str()),
            "case {case:?}: the backward cursor is handed back at the end of the stream, got {page}"
        );
    }
}

/// A `nextToken` that is not in the envelope at all is not a cursor: the service refuses
/// it with the error its model declares for a parameter specified incorrectly.
#[tokio::test]
async fn a_token_outside_the_envelope_is_refused() {
    let logs = Logs::new();
    stream_of(&logs, "refusals", 2).await;

    let tokens: Vec<(&str, &str)> = vec![
        ("no envelope at all", "not-a-token"),
        ("a bare body", "7b226b696e64223a22476574"),
        ("an unknown direction prefix", "x/abcdef"),
        ("a direction prefix with no body", "f/"),
        ("a sentinel with no body", "b//s"),
        ("a body carrying a separator", "f/abc/def"),
        ("a direction prefix only", "f"),
        ("an upper-case direction prefix", "F/abcdef"),
    ];
    for (case, token) in &tokens {
        let code = logs
            .err(
                "GetLogEvents",
                &json!({
                    "logGroupName": "refusals",
                    "logStreamName": "stream",
                    "nextToken": token,
                }),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code}"
        );
    }
}

/// A scan over a window holding no events has no position to resume from, so both its
/// cursors are the zero position, already at the end of the stream — and handing either
/// back is the same empty page again.
#[tokio::test]
async fn a_window_with_no_events_reports_the_zero_position() {
    let logs = Logs::new();
    let (base, expected) = stream_of(&logs, "empty", 2).await;
    group_with_stream(&logs, "empty-stream", "stream", None).await;

    // An empty stream, and a window that excludes every event this stream holds.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a stream with no events",
            json!({"logGroupName": "empty-stream", "logStreamName": "stream"}),
        ),
        (
            "a window before every event",
            json!({
                "logGroupName": "empty",
                "logStreamName": "stream",
                "startTime": base - 10_000,
                "endTime": base - 1,
            }),
        ),
        (
            "a window after every event",
            json!({
                "logGroupName": "empty",
                "logStreamName": "stream",
                "startTime": base + 60_000,
            }),
        ),
    ];
    for (case, request) in &cases {
        let page = logs.ok("GetLogEvents", request).await;
        assert_eq!(
            messages(&page),
            Vec::<String>::new(),
            "case {case:?}: expected no events, got {page}"
        );
        let ((forward_body, forward_end), (backward_body, backward_end)) = cursors(case, &page);
        assert!(
            forward_body.chars().all(|digit| digit == '0') && !forward_body.is_empty(),
            "case {case:?}: an empty window reports the zero position, got {page}"
        );
        assert_eq!(
            backward_body, forward_body,
            "case {case:?}: both cursors report the same zero position, got {page}"
        );
        assert!(
            forward_end && backward_end,
            "case {case:?}: the zero position is already at the end of the stream, got {page}"
        );

        // Handing the zero cursor back is the same empty page, not a refusal.
        let mut again_request = request.clone();
        again_request["nextToken"] = page["nextForwardToken"].clone();
        let again = logs.ok("GetLogEvents", &again_request).await;
        assert_eq!(
            messages(&again),
            Vec::<String>::new(),
            "case {case:?}: the zero cursor pages to an empty page, got {again}"
        );
    }

    // The stream that does hold events still serves them outside those windows, so the
    // empty pages above come from the window and not from a lost stream.
    let whole = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": "empty", "logStreamName": "stream", "startFromHead": true}),
        )
        .await;
    assert_eq!(
        messages(&whole),
        expected,
        "case \"the whole stream\": expected every event, got {whole}"
    );
}

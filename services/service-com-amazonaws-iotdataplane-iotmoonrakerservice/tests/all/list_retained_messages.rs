// SPDX-License-Identifier: CC0-1.0

//! `ListRetainedMessages`, end to end over HTTP through the router this service mounts
//! in production.
//!
//! The rules these cases assert:
//!
//! * The listing reports exactly one SUMMARY per topic that currently holds a retained
//!   message, computed from the stored message: its topic, the payload's size in bytes,
//!   the QoS it was published at, and the same epoch-millisecond instant
//!   `GetRetainedMessage` reports for it. A summary never carries a payload — that is
//!   what `GetRetainedMessage` is for — and a topic with no retained message (never
//!   published, published without `retain`, or cleared with an empty retained payload)
//!   has no entry.
//! * The listing walks the store in one stable order, `maxResults` bounds a page,
//!   and the `nextToken` of a page is what continues it: following the tokens visits
//!   every retained topic exactly once and the final page carries no token.
//! * Listing is a REPORT: two listings with nothing published in between are identical,
//!   and a listing leaves every retained message readable.
//! * A `maxResults` outside the modelled range is refused by the model's own bound, and
//!   a `nextToken` that is not one this operation issued is an `InvalidRequestException`
//!   naming the token rather than a silently empty page.
//!
//! Each table row varies what the rules generalise over — payload size, payload bytes,
//! QoS, topic depth, topic count, page size, token position — and no row is privileged.

use serde_json::Value;

use crate::shadow_and_retained_message::{Answer, Harness};

/// The top of the modelled `maxResults` range, as
/// `smithy_query(query="operation", operation="ListRetainedMessages")` states it
/// (`range {min: 1, max: 200}`) and as the crate's
/// `state::MAX_RETAINED_MESSAGE_PAGE_SIZE` declares it.
const MAX_RESULTS_CEILING: i64 = 200;

/// The bottom of that same modelled range.
const MAX_RESULTS_FLOOR: i64 = 1;

/// One retained message to publish, and what its summary must then report.
struct RetainedCase {
    name: &'static str,
    /// The topic as it appears in a request path (`/` percent-encoded).
    uri_topic: &'static str,
    /// The same topic once the path label is decoded — what the summary reports.
    topic: &'static str,
    qos: i32,
    payload: Vec<u8>,
}

/// Rows differing in topic depth, QoS, payload size and payload bytes. The sizes span a
/// single byte, a short ASCII payload, a payload with a multi-byte UTF-8 character (so
/// `payloadSize` is BYTES, not characters) and a 4 KiB binary payload.
fn retained_cases() -> Vec<RetainedCase> {
    vec![
        RetainedCase {
            name: "single-segment topic, one byte, QoS 0",
            uri_topic: "beacon",
            topic: "beacon",
            qos: 0,
            payload: b"x".to_vec(),
        },
        RetainedCase {
            name: "two-level topic, short ASCII, QoS 1",
            uri_topic: "sensors%2Ftemperature",
            topic: "sensors/temperature",
            qos: 1,
            payload: b"21.5".to_vec(),
        },
        RetainedCase {
            name: "four-level topic, multi-byte UTF-8, QoS 0",
            uri_topic: "site%2Fbuilding-3%2Ffloor-2%2Fstatus",
            topic: "site/building-3/floor-2/status",
            qos: 0,
            // 6 characters, 10 bytes: "stäts-ok" style payloads prove payloadSize
            // counts bytes.
            payload: "héllo".as_bytes().to_vec(),
        },
        RetainedCase {
            name: "two-level topic, 4 KiB of binary, QoS 1",
            uri_topic: "devices%2Fbulk",
            topic: "devices/bulk",
            qos: 1,
            payload: (0..4096u32).map(|index| (index % 251) as u8).collect(),
        },
        RetainedCase {
            name: "topic sorting last, 1 KiB, QoS 0",
            uri_topic: "zone-9%2Fsummary",
            topic: "zone-9/summary",
            qos: 0,
            payload: vec![b'z'; 1024],
        },
    ]
}

/// The retained-message listing, with whatever query string the case uses.
async fn list(harness: &Harness, query: &str) -> Answer {
    harness
        .send("GET", &format!("/retainedMessage{query}"), &[], Vec::new())
        .await
}

/// The `retainedTopics` member of a listing, as a vector of summaries. An absent member
/// reads as no summaries, so a page with nothing on it is not a parse failure.
fn summaries(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200, got {} with body {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("retainedTopics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `nextToken` of a listing, if it carries one.
fn next_token(answer: &Answer) -> Option<String> {
    answer
        .json()
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn topic_of(case: &str, summary: &Value) -> String {
    summary
        .get("topic")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a topic on summary {summary}"))
        .to_string()
}

/// Publish every row of the table as a retained message.
async fn publish_all(harness: &Harness) {
    for case in retained_cases() {
        let published = harness
            .publish(case.uri_topic, case.qos, true, None, case.payload.clone())
            .await;
        assert_eq!(
            published.status,
            200,
            "case {:?}: expected the retained publish to be accepted, got {} with body {}",
            case.name,
            published.status,
            String::from_utf8_lossy(&published.body)
        );
    }
}

#[tokio::test]
async fn every_retained_topic_is_summarised_from_the_stored_message() {
    let harness = Harness::new();
    publish_all(&harness).await;

    let listed = list(&harness, "").await;
    let page = summaries("all retained topics", &listed);
    let cases = retained_cases();
    assert_eq!(
        page.len(),
        cases.len(),
        "expected one summary per retained topic ({}), got {} in body {}",
        cases.len(),
        page.len(),
        String::from_utf8_lossy(&listed.body)
    );
    assert_eq!(
        next_token(&listed),
        None,
        "expected no continuation token when one page holds every topic, got body {}",
        String::from_utf8_lossy(&listed.body)
    );

    for case in &cases {
        let summary = page
            .iter()
            .find(|summary| topic_of(case.name, summary) == case.topic)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected topic {:?} among the summaries, got {}",
                    case.name,
                    case.topic,
                    String::from_utf8_lossy(&listed.body)
                )
            });

        assert_eq!(
            summary.get("payloadSize").and_then(Value::as_i64),
            Some(case.payload.len() as i64),
            "case {:?}: expected payloadSize {} (the stored payload's bytes), got {summary}",
            case.name,
            case.payload.len()
        );
        assert_eq!(
            summary.get("qos").and_then(Value::as_i64),
            Some(i64::from(case.qos)),
            "case {:?}: expected the published qos {}, got {summary}",
            case.name,
            case.qos
        );
        // A summary reports the payload's SIZE and nothing of its bytes.
        assert_eq!(
            summary.get("payload"),
            None,
            "case {:?}: a summary must carry no payload, got {summary}",
            case.name
        );

        // The instant is the one the retained-message reader reports for the same
        // topic, in epoch MILLISECONDS.
        let read = harness.get_retained_message(case.uri_topic).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected the retained message to be readable, got {} with body {}",
            case.name,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let stored_instant = read.json().get("lastModifiedTime").and_then(Value::as_i64);
        assert_eq!(
            summary.get("lastModifiedTime").and_then(Value::as_i64),
            stored_instant,
            "case {:?}: expected the summary's lastModifiedTime to be the stored one \
             ({stored_instant:?}), got {summary}",
            case.name
        );
        let instant = stored_instant.unwrap_or_default();
        // Epoch milliseconds: 1_600_000_000_000 ms is 2020-09-13, so a value in
        // SECONDS could never reach it.
        assert!(
            instant > 1_600_000_000_000,
            "case {:?}: expected lastModifiedTime in epoch milliseconds, got {instant}",
            case.name
        );
    }

    // One stable order: the order is the store's, and it is the same on every call.
    let topics: Vec<String> = page
        .iter()
        .map(|summary| topic_of("ordering", summary))
        .collect();
    let mut sorted = topics.clone();
    sorted.sort();
    assert_eq!(
        topics, sorted,
        "expected the summaries in a stable topic order, got {topics:?}"
    );
}

#[tokio::test]
async fn following_the_next_token_visits_every_retained_topic_exactly_once() {
    let harness = Harness::new();
    publish_all(&harness).await;

    let mut expected: Vec<String> = retained_cases()
        .iter()
        .map(|case| case.topic.to_string())
        .collect();
    expected.sort();

    // Page sizes spanning both published bounds of the modelled range and the sizes
    // between: 1 and 200 are the bounds, 2 splits the table unevenly, and 5 is exactly
    // the number of retained topics.
    for page_size in [MAX_RESULTS_FLOOR, 2, 5, MAX_RESULTS_CEILING] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let query = match &token {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let answer = list(&harness, &query).await;
            let page = summaries(&case, &answer);
            assert!(
                page.len() as i64 <= page_size,
                "case {case:?}: expected at most {page_size} summaries on a page, got {}",
                page.len()
            );
            for summary in &page {
                seen.push(topic_of(&case, summary));
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: expected pagination to terminate, got {pages} pages"
            );
            match next_token(&answer) {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case {case:?}: expected a continued page to carry summaries, got none"
                    );
                    token = Some(next);
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every retained topic exactly once in store order, got {seen:?}"
        );
        let mut deduplicated = seen.clone();
        deduplicated.dedup();
        assert_eq!(
            deduplicated, seen,
            "case {case:?}: expected no topic twice across pages, got {seen:?}"
        );
    }

    // A token that points past the end is an empty final page, not an error.
    let past_the_end = format!("?nextToken={}", expected.len());
    let answer = list(&harness, &past_the_end).await;
    assert!(
        summaries("token past the end", &answer).is_empty(),
        "expected no summaries past the end of the store, got body {}",
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        next_token(&answer),
        None,
        "expected no continuation past the end of the store, got body {}",
        String::from_utf8_lossy(&answer.body)
    );
}

#[tokio::test]
async fn only_topics_holding_a_retained_message_are_listed() {
    let harness = Harness::new();

    // Nothing retained yet: the listing is empty rather than a 404.
    let empty = list(&harness, "").await;
    assert!(
        summaries("nothing retained", &empty).is_empty(),
        "expected an empty listing before anything is retained, got body {}",
        String::from_utf8_lossy(&empty.body)
    );

    // Rows: retained and listed; published without `retain` and therefore not listed.
    for (case, uri_topic, retain) in [
        ("retained", "kept-one", true),
        ("not retained", "transient-one", false),
        ("retained, deeper topic", "kept%2Ftwo", true),
        ("not retained, deeper topic", "transient%2Ftwo", false),
    ] {
        let published = harness
            .publish(uri_topic, 0, retain, None, b"body".to_vec())
            .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: expected the publish to be accepted, got {}",
            published.status
        );
    }

    let listed = list(&harness, "").await;
    let topics: Vec<String> = summaries("retain flag", &listed)
        .iter()
        .map(|summary| topic_of("retain flag", summary))
        .collect();
    assert_eq!(
        topics,
        vec!["kept-one".to_string(), "kept/two".to_string()],
        "expected only the retained publishes to be listed, got {topics:?}"
    );

    // An overwrite keeps ONE entry per topic, carrying the latest publish's values.
    harness
        .publish("kept-one", 1, true, None, b"a much longer body".to_vec())
        .await;
    let overwritten = list(&harness, "").await;
    let page = summaries("overwrite", &overwritten);
    let kept: Vec<&Value> = page
        .iter()
        .filter(|summary| topic_of("overwrite", summary) == "kept-one")
        .collect();
    assert_eq!(
        kept.len(),
        1,
        "expected an overwrite to keep one entry for the topic, got {} in body {}",
        kept.len(),
        String::from_utf8_lossy(&overwritten.body)
    );
    assert_eq!(
        kept[0].get("payloadSize").and_then(Value::as_i64),
        Some("a much longer body".len() as i64),
        "expected the latest payload's size, got {}",
        kept[0]
    );
    assert_eq!(
        kept[0].get("qos").and_then(Value::as_i64),
        Some(1),
        "expected the latest publish's qos, got {}",
        kept[0]
    );

    // Listing is a REPORT: a second listing with nothing published in between answers
    // the same summaries, and every retained message is still readable afterwards.
    let again = list(&harness, "").await;
    assert_eq!(
        summaries("repeat listing", &again),
        page,
        "expected a second listing to answer the same summaries, got body {}",
        String::from_utf8_lossy(&again.body)
    );
    for uri_topic in ["kept-one", "kept%2Ftwo"] {
        let read = harness.get_retained_message(uri_topic).await;
        assert_eq!(
            read.status, 200,
            "expected {uri_topic} to still hold its retained message after listing, got {}",
            read.status
        );
    }

    // Clearing a retained message (an empty retained payload) removes its entry.
    harness.publish("kept-one", 0, true, None, Vec::new()).await;
    let cleared = list(&harness, "").await;
    let topics: Vec<String> = summaries("cleared", &cleared)
        .iter()
        .map(|summary| topic_of("cleared", summary))
        .collect();
    assert_eq!(
        topics,
        vec!["kept/two".to_string()],
        "expected a cleared topic to drop out of the listing, got {topics:?}"
    );
}

#[tokio::test]
async fn a_request_outside_the_modelled_bounds_or_with_an_unissued_token_is_refused() {
    let harness = Harness::new();
    publish_all(&harness).await;

    // The modelled `maxResults` range: both published bounds are accepted, and one
    // step past either bound is refused by the model before the handler runs.
    for (case, max_results, expected_status) in [
        ("at the floor", MAX_RESULTS_FLOOR, 200),
        ("at the ceiling", MAX_RESULTS_CEILING, 200),
        ("one below the floor", MAX_RESULTS_FLOOR - 1, 400),
        ("one above the ceiling", MAX_RESULTS_CEILING + 1, 400),
    ] {
        let answer = list(&harness, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?} (maxResults={max_results}): expected {expected_status}, got {} with \
             body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A `nextToken` this operation never issued is an invalid request naming the token,
    // not an empty page.
    for (case, token) in [
        ("not a number", "not-a-token"),
        ("negative", "-1"),
        ("fractional", "1.5"),
        ("empty", ""),
    ] {
        let answer = list(&harness, &format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for an unissued nextToken, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got body {}",
            String::from_utf8_lossy(&answer.body)
        );
        let message = answer
            .json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(token),
            "case {case:?}: expected the refusal to name the token {token:?}, got {message:?}"
        );
    }

    // The refused requests changed nothing: every retained topic is still listed.
    let listed = list(&harness, "").await;
    assert_eq!(
        summaries("after refusals", &listed).len(),
        retained_cases().len(),
        "expected the refused requests to change nothing, got body {}",
        String::from_utf8_lossy(&listed.body)
    );
}

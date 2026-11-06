// SPDX-License-Identifier: CC0-1.0

//! The two ceilings the message broker puts on a `Publish`, end to end over HTTP through
//! the router this service mounts in production.
//!
//! The rules these cases assert:
//!
//! * A message the broker will not take because its PAYLOAD is larger than the published
//!   quota is refused as an entity too large — `RequestEntityTooLargeException` (HTTP
//!   413), which tells a publisher to split or shrink the message — and not as a
//!   malformed request. The quota is measured inclusively: a payload of exactly the
//!   quota is taken, one byte more is refused, and a refused publish stores nothing. The
//!   refusal names the CEILING and nothing of the request that crossed it.
//! * A topic name is weighed against two INDEPENDENT ceilings, each refused with
//!   `InvalidRequestException` (HTTP 400) carrying its own message naming its own limit:
//!   how many `/`-separated parts the name carries, and how many BYTES the whole name is
//!   (measured on the UTF-8 name, so a multi-byte character costs its bytes and not
//!   one). A name of legal depth is still weighed against the byte ceiling, and the
//!   same verdict is reached on the READ path: a name the broker will not publish to is
//!   malformed when a retained-message read names it too, not merely absent.
//!
//! Every limit is named by the crate (`state::MAX_PUBLISH_PAYLOAD_BYTES`,
//! `state::MAX_TOPIC_PARTS`, `state::MAX_TOPIC_BYTES`) and every message by the crate's
//! own renderer, so a case cannot drift from the rule the handler enforces. The tables
//! span what each rule generalises over — payload size, name length, character width,
//! part count — with a row at each published bound and one past it; the recorded case
//! that filed the issue is one row among them.

use crate::shadow_and_retained_message::{Answer, Harness};
use serde_json::Value;
use service_com_amazonaws_iotdataplane_iotmoonrakerservice::state::{
    payload_too_large, too_many_topic_bytes, too_many_topic_parts, INVALID_TOPIC_FILTER,
    MAX_PUBLISH_PAYLOAD_BYTES, MAX_TOPIC_BYTES, MAX_TOPIC_PARTS,
};

/// Percent-encode `value` for one URI component: a `{topic}` path label carries no `/`
/// and no byte outside the unreserved ASCII set, so everything else travels
/// percent-encoded — which is how an SDK sends a topic name holding slashes or UTF-8.
pub(crate) fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~') {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The `message` of a response body.
fn message(answer: &Answer) -> String {
    answer
        .json()
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One topic name, and what the broker does with it.
struct TopicCase {
    name: String,
    /// The topic as the handler sees it, before URI encoding.
    topic: String,
    /// `None` when the broker takes the name; otherwise the message it refuses it with.
    refusal: Option<String>,
}

/// A topic name of exactly `bytes` bytes carrying exactly [`MAX_TOPIC_PARTS`] parts, so
/// the byte rule can be exercised at legal depth.
fn parts_of_total_length(bytes: usize) -> String {
    let separators = MAX_TOPIC_PARTS - 1;
    let payload = bytes - separators;
    let per_part = payload / MAX_TOPIC_PARTS;
    let mut parts: Vec<String> = (1..MAX_TOPIC_PARTS).map(|_| "q".repeat(per_part)).collect();
    parts.push("r".repeat(payload - per_part * separators));
    let name = parts.join("/");
    assert_eq!(
        name.len(),
        bytes,
        "the case builder must produce a {bytes}-byte name, got {}",
        name.len()
    );
    name
}

/// A topic name of `parts` short `/`-separated parts.
fn name_of_parts(parts: usize) -> String {
    (0..parts)
        .map(|index| format!("p{index}"))
        .collect::<Vec<String>>()
        .join("/")
}

fn topic_cases() -> Vec<TopicCase> {
    let too_long = too_many_topic_bytes(MAX_TOPIC_BYTES);
    let too_deep = too_many_topic_parts(MAX_TOPIC_PARTS);
    let mut cases = vec![
        // The byte ceiling, at each published bound and past it.
        TopicCase {
            name: "a one-byte name".to_string(),
            topic: "a".to_string(),
            refusal: None,
        },
        TopicCase {
            name: "a name well inside the byte ceiling".to_string(),
            topic: "b".repeat(64),
            refusal: None,
        },
        TopicCase {
            name: "one byte below the ceiling".to_string(),
            topic: "c".repeat(MAX_TOPIC_BYTES - 1),
            refusal: None,
        },
        TopicCase {
            name: "exactly at the ceiling".to_string(),
            topic: "d".repeat(MAX_TOPIC_BYTES),
            refusal: None,
        },
        TopicCase {
            name: "one byte past the ceiling".to_string(),
            topic: "e".repeat(MAX_TOPIC_BYTES + 1),
            refusal: Some(too_long.clone()),
        },
        TopicCase {
            name: "far past the ceiling".to_string(),
            topic: "f".repeat(4 * MAX_TOPIC_BYTES),
            refusal: Some(too_long.clone()),
        },
        // The ceiling counts BYTES, not characters: a two-byte character costs two.
        TopicCase {
            name: "multi-byte characters exactly at the ceiling".to_string(),
            topic: "é".repeat(MAX_TOPIC_BYTES / 2),
            refusal: None,
        },
        TopicCase {
            name: "multi-byte characters past the ceiling in bytes, not in characters"
                .to_string(),
            topic: "é".repeat(MAX_TOPIC_BYTES / 2 + 1),
            refusal: Some(too_long.clone()),
        },
        // Depth and length are independent: a name at legal depth is still weighed
        // against the byte ceiling, and a short name against the part ceiling.
        TopicCase {
            name: "the maximum depth exactly at the byte ceiling".to_string(),
            topic: parts_of_total_length(MAX_TOPIC_BYTES),
            refusal: None,
        },
        TopicCase {
            name: "the maximum depth one byte past the byte ceiling".to_string(),
            topic: parts_of_total_length(MAX_TOPIC_BYTES + 1),
            refusal: Some(too_long),
        },
        TopicCase {
            name: "one part past the depth ceiling, well inside the byte ceiling"
                .to_string(),
            topic: name_of_parts(MAX_TOPIC_PARTS + 1),
            refusal: Some(too_deep.clone()),
        },
        TopicCase {
            name: "far past the depth ceiling".to_string(),
            topic: name_of_parts(3 * MAX_TOPIC_PARTS),
            refusal: Some(too_deep.clone()),
        },
        TopicCase {
            name: "past both ceilings, refused on the depth it crossed first".to_string(),
            topic: name_of_parts(MAX_TOPIC_PARTS + 1) + &"z".repeat(MAX_TOPIC_BYTES),
            refusal: Some(too_deep.clone()),
        },
        // A name carrying a wildcard is a FILTER and not a name at all.
        TopicCase {
            name: "a single-level wildcard".to_string(),
            topic: "sensors/+/temperature".to_string(),
            refusal: Some(INVALID_TOPIC_FILTER.to_string()),
        },
    ];
    // Every depth up to the ceiling is taken; the ceiling itself is inclusive.
    for parts in 1..=MAX_TOPIC_PARTS {
        cases.push(TopicCase {
            name: format!("{parts} part(s), inside the depth ceiling"),
            topic: name_of_parts(parts),
            refusal: None,
        });
    }
    cases
}

#[tokio::test]
async fn a_publish_topic_is_weighed_against_the_part_and_the_byte_ceiling() {
    let harness = Harness::new();

    for case in topic_cases() {
        let encoded = encode_component(&case.topic);
        let payload = format!("payload for {}", case.name).into_bytes();
        let answer = harness
            .publish(&encoded, 0, true, None, payload.clone())
            .await;
        match &case.refusal {
            None => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {:?}: expected the broker to take a {}-byte, {}-part name, got {} \
                     with body {}",
                    case.name,
                    case.topic.len(),
                    case.topic.split('/').count(),
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                // Taken means STORED: the retained-message reader answers from the store.
                let read = harness.get_retained_message(&encoded).await;
                assert_eq!(
                    read.status,
                    200,
                    "case {:?}: expected the retained message to be readable, got {} with \
                     body {}",
                    case.name,
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                assert_eq!(
                    read.json().get("topic").and_then(Value::as_str),
                    Some(case.topic.as_str()),
                    "case {:?}: expected the stored topic back, got {}",
                    case.name,
                    String::from_utf8_lossy(&read.body)
                );
            }
            Some(expected) => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {:?}: expected 400 for a {}-byte, {}-part name, got {} with body {}",
                    case.name,
                    case.topic.len(),
                    case.topic.split('/').count(),
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidRequestException",
                    "case {:?}: expected InvalidRequestException, got {}",
                    case.name,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    message(&answer),
                    *expected,
                    "case {:?}: expected the refusal naming its own limit, got {:?}",
                    case.name,
                    message(&answer)
                );
                // A refused topic name is refused on the READ path with the same verdict:
                // the name identifies no single message on either side of the broker.
                let read = harness.get_retained_message(&encoded).await;
                assert_eq!(
                    read.status,
                    400,
                    "case {:?}: expected the read of a malformed topic name to be refused \
                     400, got {} with body {}",
                    case.name,
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                assert_eq!(
                    read.error_code(),
                    "InvalidRequestException",
                    "case {:?}: expected InvalidRequestException on the read, got {}",
                    case.name,
                    String::from_utf8_lossy(&read.body)
                );
                assert_eq!(
                    message(&read),
                    *expected,
                    "case {:?}: expected the read to carry the same refusal as the \
                     publish, got {:?}",
                    case.name,
                    message(&read)
                );
            }
        }
    }
}

#[tokio::test]
async fn an_oversize_publish_payload_is_refused_as_an_entity_too_large() {
    let harness = Harness::new();

    // One row per published bound of the payload quota and one past it, plus the small
    // sizes the rule has to leave alone.
    for (case, size) in [
        ("an empty payload", 0usize),
        ("a one-byte payload", 1),
        ("a kilobyte", 1024),
        ("one byte below the quota", MAX_PUBLISH_PAYLOAD_BYTES - 1),
        ("exactly at the quota", MAX_PUBLISH_PAYLOAD_BYTES),
        ("one byte past the quota", MAX_PUBLISH_PAYLOAD_BYTES + 1),
        ("far past the quota", 2 * MAX_PUBLISH_PAYLOAD_BYTES),
    ] {
        let topic = format!("sized-{size}");
        let payload = vec![0x5a_u8; size];
        let answer = harness.publish(&topic, 1, true, None, payload).await;
        let over_quota = size > MAX_PUBLISH_PAYLOAD_BYTES;
        if over_quota {
            assert_eq!(
                answer.status,
                413,
                "case {case:?}: expected 413 for {size} bytes, got {} with body {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "RequestEntityTooLargeException",
                "case {case:?}: expected RequestEntityTooLargeException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                message(&answer),
                payload_too_large(MAX_PUBLISH_PAYLOAD_BYTES),
                "case {case:?}: expected the refusal naming the ceiling, got {:?}",
                message(&answer)
            );
            // The ceiling is the only number in it: the size the request carried is not
            // part of the sentence the broker answers with.
            assert!(
                !message(&answer).contains(&size.to_string()),
                "case {case:?}: expected the refusal to carry nothing of the request's \
                 {size} bytes, got {:?}",
                message(&answer)
            );
            // Nothing was stored for the refused publish.
            let read = harness.get_retained_message(&topic).await;
            assert_eq!(
                read.status,
                404,
                "case {case:?}: a refused publish must retain nothing, got {} with body {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            continue;
        }
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the broker to take {size} bytes, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let read = harness.get_retained_message(&topic).await;
        if size == 0 {
            // An empty retained payload CLEARS a topic's retained message, so there is
            // nothing to read back — that is MQTT's delete, not a refusal.
            assert_eq!(
                read.status,
                404,
                "case {case:?}: an empty retained payload clears the topic, got {} with body {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            continue;
        }
        let stored = read
            .json()
            .get("payload")
            .and_then(Value::as_str)
            .map(|encoded| aws_smithy_types::base64::decode(encoded).expect("base64"))
            .unwrap_or_default();
        assert_eq!(
            stored.len(),
            size,
            "case {case:?}: expected the stored payload to be {size} bytes, got {}",
            stored.len()
        );
    }
}

#[tokio::test]
async fn a_refused_oversize_publish_leaves_the_retained_message_it_would_have_replaced() {
    let harness = Harness::new();

    let accepted = vec![1u8; MAX_PUBLISH_PAYLOAD_BYTES];
    let first = harness
        .publish("replace-topic", 0, true, None, accepted.clone())
        .await;
    assert_eq!(
        first.status,
        200,
        "expected a payload at the quota to be taken, got {} with body {}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );

    let refused = harness
        .publish(
            "replace-topic",
            0,
            true,
            None,
            vec![2u8; MAX_PUBLISH_PAYLOAD_BYTES + 1],
        )
        .await;
    assert_eq!(
        refused.status,
        413,
        "expected the over-quota publish to be refused 413, got {} with body {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );

    let stored = harness
        .get_retained_message("replace-topic")
        .await
        .json()
        .get("payload")
        .and_then(Value::as_str)
        .map(|encoded| aws_smithy_types::base64::decode(encoded).expect("base64"))
        .unwrap_or_default();
    assert_eq!(
        stored, accepted,
        "expected the refused publish to have changed nothing, got {} bytes",
        stored.len()
    );
}

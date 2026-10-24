// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two message-plane rules the lifecycle suite does not pin, both
//! driven over the wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`)
//! through the shared harness in [`super::support`]:
//!
//! 1. `SendMessage` accepts only the Unicode characters its own documentation lists in
//!    a message body and answers the modelled `InvalidMessageContents` for anything
//!    else.
//! 2. `ReceiveMessage` reports the message system attributes it genuinely records, in
//!    the documented units, and `MessageSystemAttributeNames` — or the discontinued
//!    `AttributeNames` spelling — selects which of them come back.
//!
//! Both tables span the class the rule generalises over rather than one example: the
//! character rows walk every boundary of every allowed range and the gaps between them,
//! and the selection rows walk every form a request can ask in (both member spellings,
//! `All`, a subset, nothing at all).

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use serde_json::{json, Value};

use super::support::{as_map, call, create_ok, emulator, error_code, messages, ok};

/// One message's system `Attributes` map.
fn attributes(message: &Value) -> BTreeMap<String, String> {
    message.get("Attributes").map(as_map).unwrap_or_default()
}

/// A `SystemTime` as whole milliseconds since the Unix epoch.
fn epoch_millis(at: SystemTime) -> u128 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default()
}

/// Only the characters `SendMessage` documents may appear in a message body: "#x9 | #xA
/// | #xD | #x20 to #xD7FF | #xE000 to #xFFFD | #x10000 to #x10FFFF". Anything else is
/// refused with the modelled `InvalidMessageContents`, and nothing is stored.
#[tokio::test]
async fn a_body_character_outside_the_allowed_set_is_refused() {
    let router = emulator();
    let url = create_ok(&router, "characters", &json!({})).await;

    // (case, body, whether the allowed set admits it) — the rows walk each range's
    // first and last character and every gap between the ranges.
    let rows: &[(&str, String, bool)] = &[
        ("tab, #x9", "a\tb".to_string(), true),
        ("line feed, #xA", "a\nb".to_string(), true),
        ("carriage return, #xD", "a\rb".to_string(), true),
        (
            "space, the start of the third range",
            "a b".to_string(),
            true,
        ),
        (
            "the end of the third range, #xD7FF",
            "a\u{D7FF}b".to_string(),
            true,
        ),
        (
            "the start of the fourth range, #xE000",
            "a\u{E000}b".to_string(),
            true,
        ),
        (
            "the end of the fourth range, #xFFFD",
            "a\u{FFFD}b".to_string(),
            true,
        ),
        (
            "the start of the fifth range, #x10000",
            "a\u{10000}b".to_string(),
            true,
        ),
        (
            "the end of the fifth range, #x10FFFF",
            "a\u{10FFFF}b".to_string(),
            true,
        ),
        ("a NUL, below every range", "a\u{0}b".to_string(), false),
        ("#x1, below every range", "a\u{1}b".to_string(), false),
        ("#x8, just below the tab", "a\u{8}b".to_string(), false),
        ("#xB, just above the line feed", "a\u{B}b".to_string(), false),
        (
            "#xC, just below the carriage return",
            "a\u{C}b".to_string(),
            false,
        ),
        ("#x1F, just below the space", "a\u{1F}b".to_string(), false),
        (
            "#xFFFE, just above the fourth range",
            "a\u{FFFE}b".to_string(),
            false,
        ),
    ];

    let mut accepted = 0usize;
    for (case, body, allowed) in rows {
        let (status, parsed) = call(
            &router,
            "SendMessage",
            &json!({"QueueUrl": url, "MessageBody": body}),
        )
        .await;
        if *allowed {
            accepted += 1;
            assert_eq!(
                status, 200,
                "case {case}: the character is in the allowed set, so the send must succeed, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidMessageContents"),
                "case {case}: expected a 400 InvalidMessageContents, got {status} with {parsed}"
            );
        }
    }

    // Exactly the accepted bodies are in state: a refused send stores nothing.
    let received = messages(
        &ok(
            &router,
            "ReceiveMessage",
            json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
        )
        .await,
    );
    assert_eq!(
        received.len(),
        accepted,
        "case the queue after the table: expected the {accepted} accepted bodies and nothing else, got {}",
        received.len()
    );
}

/// One row of the system-attribute selection table: which member carries the names,
/// which names it carries, and which attributes must come back.
struct SelectionRow {
    case: &'static str,
    /// `MessageSystemAttributeNames`, or its discontinued spelling `AttributeNames`.
    member: &'static str,
    /// The names supplied, or `None` to omit the member entirely.
    requested: Option<&'static [&'static str]>,
    expected: &'static [&'static str],
}

/// The message system attributes a receive reports are computed from the message's own
/// history, in the documented units, and either name member selects them.
#[tokio::test]
async fn message_system_attributes_are_computed_and_selected_by_name() {
    let router = emulator();
    let url = create_ok(&router, "system-attributes", &json!({})).await;
    let before = epoch_millis(SystemTime::now());
    ok(
        &router,
        "SendMessage",
        json!({"QueueUrl": url, "MessageBody": "one"}),
    )
    .await;

    // The emulator records these three; the rest of the `MessageSystemAttributeName`
    // set belongs to behaviour it does not implement, so no row expects one. An empty
    // `expected` means the `Attributes` member must be absent entirely.
    const EVERY: &[&str] = &[
        "ApproximateFirstReceiveTimestamp",
        "ApproximateReceiveCount",
        "SentTimestamp",
    ];
    let rows: &[SelectionRow] = &[
        SelectionRow {
            case: "every system attribute",
            member: "MessageSystemAttributeNames",
            requested: Some(&["All"]),
            expected: EVERY,
        },
        SelectionRow {
            case: "the discontinued AttributeNames spelling",
            member: "AttributeNames",
            requested: Some(&["All"]),
            expected: EVERY,
        },
        SelectionRow {
            case: "one name",
            member: "MessageSystemAttributeNames",
            requested: Some(&["SentTimestamp"]),
            expected: &["SentTimestamp"],
        },
        SelectionRow {
            case: "two names",
            member: "MessageSystemAttributeNames",
            requested: Some(&["SentTimestamp", "ApproximateReceiveCount"]),
            expected: &["ApproximateReceiveCount", "SentTimestamp"],
        },
        SelectionRow {
            case: "a name this emulator records nothing for",
            member: "MessageSystemAttributeNames",
            requested: Some(&["SequenceNumber"]),
            expected: &[],
        },
        SelectionRow {
            case: "no names at all",
            member: "MessageSystemAttributeNames",
            requested: None,
            expected: &[],
        },
    ];

    for SelectionRow {
        case,
        member,
        requested,
        expected,
    } in rows
    {
        // Visibility timeout 0, so every row sees the same message.
        let mut request = json!({"QueueUrl": url, "VisibilityTimeout": 0});
        if let Some(names) = requested {
            request[*member] = json!(names);
        }
        let received = messages(&ok(&router, "ReceiveMessage", request).await);
        assert_eq!(
            received.len(),
            1,
            "case {case}: expected the one sent message, got {received:?}"
        );
        let reported = attributes(&received[0]);
        let actual: BTreeSet<&str> = reported.keys().map(String::as_str).collect();
        let wanted: BTreeSet<&str> = expected.iter().copied().collect();
        assert_eq!(
            actual, wanted,
            "case {case}: expected the system attributes {wanted:?}, got {actual:?}"
        );
        if let Some(sent_at) = reported.get("SentTimestamp") {
            let parsed: u128 = sent_at
                .parse()
                .unwrap_or_else(|_| panic!("case {case}: SentTimestamp {sent_at} is not a number"));
            // "returned as an integer representing the epoch time in milliseconds"
            // (`Message$Attributes` docs), so it lies between the two clock readings
            // this test takes around the send.
            let now = epoch_millis(SystemTime::now());
            assert!(
                (before..=now).contains(&parsed),
                "case {case}: SentTimestamp {parsed} is not epoch milliseconds between {before} and {now}"
            );
        }
    }
}

/// Across repeated receives of the same message, `ApproximateReceiveCount` counts the
/// receive being answered and `ApproximateFirstReceiveTimestamp` stays at the FIRST
/// one — "the time the message was first received from the queue".
#[tokio::test]
async fn the_first_receive_timestamp_stays_fixed_while_the_receive_count_rises() {
    let router = emulator();
    let url = create_ok(&router, "receive-history", &json!({})).await;
    ok(
        &router,
        "SendMessage",
        json!({"QueueUrl": url, "MessageBody": "one"}),
    )
    .await;

    let mut first_received: Option<String> = None;
    for receipt in 1..=3u32 {
        let case = format!("receive {receipt}");
        let received = messages(
            &ok(
                &router,
                "ReceiveMessage",
                // Visibility timeout 0: the message is available again at once, so the
                // next pass receives the SAME message.
                json!({
                    "QueueUrl": url,
                    "VisibilityTimeout": 0,
                    "MessageSystemAttributeNames": ["All"],
                }),
            )
            .await,
        );
        assert_eq!(
            received.len(),
            1,
            "case {case}: a message hidden for zero seconds must be available again, got {received:?}"
        );
        let reported = attributes(&received[0]);
        assert_eq!(
            reported.get("ApproximateReceiveCount").cloned(),
            Some(receipt.to_string()),
            "case {case}: expected receive count {receipt}, got {reported:?}"
        );
        let first = reported
            .get("ApproximateFirstReceiveTimestamp")
            .cloned()
            .unwrap_or_default();
        assert!(
            first.parse::<u128>().is_ok(),
            "case {case}: ApproximateFirstReceiveTimestamp {first} is not epoch milliseconds"
        );
        if let Some(expected) = &first_received {
            assert_eq!(
                &first, expected,
                "case {case}: ApproximateFirstReceiveTimestamp must stay at the first receive"
            );
        }
        first_received = Some(first);
    }
}

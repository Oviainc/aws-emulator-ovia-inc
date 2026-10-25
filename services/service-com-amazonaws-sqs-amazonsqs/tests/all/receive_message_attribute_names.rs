// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the value set `ReceiveMessage`'s two system-attribute name
//! members accept, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rule pinned here: `MessageSystemAttributeNames` carries the message system
//! attribute names, `AttributeNames` is the discontinued alias of that member, and an
//! alias accepts the value set of the member that replaced it — so each name member
//! serves every message system attribute name, refuses a name outside that set with
//! `InvalidAttributeName`, and the two spellings answer the same request identically.
//!
//! The table walks the WHOLE published value set on BOTH members rather than the one
//! name a defect was reported for, and the refusal rows walk the kinds of name that are
//! outside it — including the queue attribute names the alias's modelled target lists.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use serde_json::{json, Value};

use super::support::{as_map, call, create_ok, emulator, error_code, messages, ok};

/// The two members a request may carry message system attribute names in: the one in
/// use, and the discontinued alias of it.
const NAME_MEMBERS: [&str; 2] = ["MessageSystemAttributeNames", "AttributeNames"];

/// Every value `MessageSystemAttributeName` publishes, and therefore every name both
/// members accept. Source: `smithy_query(query="shape",
/// shape="MessageSystemAttributeName")` on this crate's model.
const EVERY_SYSTEM_ATTRIBUTE_NAME: &[&str] = &[
    "All",
    "SenderId",
    "SentTimestamp",
    "ApproximateReceiveCount",
    "ApproximateFirstReceiveTimestamp",
    "SequenceNumber",
    "MessageDeduplicationId",
    "MessageGroupId",
    "AWSTraceHeader",
    "DeadLetterQueueSourceArn",
];

/// The system attributes a standard queue's message has a value for in this emulator,
/// and therefore the ones `All` reports for it. A name the emulator records nothing for
/// is still accepted; it simply selects no value.
const RECORDED_ON_A_STANDARD_QUEUE: &[&str] = &[
    "ApproximateFirstReceiveTimestamp",
    "ApproximateReceiveCount",
    "SentTimestamp",
];

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

/// Every message system attribute name is served on either member — the one in use and
/// its discontinued alias — and both report exactly the same attributes for it.
#[tokio::test]
async fn every_system_attribute_name_is_served_on_both_name_members() {
    let router = emulator();
    let url = create_ok(&router, "alias-names", &json!({})).await;
    let before = epoch_millis(SystemTime::now());
    ok(
        &router,
        "SendMessage",
        json!({"QueueUrl": url, "MessageBody": "payload"}),
    )
    .await;

    for name in EVERY_SYSTEM_ATTRIBUTE_NAME {
        // What this name selects: `All` every attribute the message has a value for,
        // any other name itself when the message has one, and nothing otherwise.
        let wanted: BTreeSet<&str> = if *name == "All" {
            RECORDED_ON_A_STANDARD_QUEUE.iter().copied().collect()
        } else {
            RECORDED_ON_A_STANDARD_QUEUE
                .iter()
                .copied()
                .filter(|recorded| recorded == name)
                .collect()
        };
        let mut per_member: Vec<(&str, BTreeSet<String>)> = Vec::new();

        for member in NAME_MEMBERS {
            let case = format!("{member} = [{name}]");
            // Visibility timeout 0, so every row sees the same message.
            let request = json!({
                "QueueUrl": url,
                "VisibilityTimeout": 0,
                member: [name],
            });
            let (status, answer) = call(&router, "ReceiveMessage", &request).await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {answer}"
            );
            let received = messages(&answer);
            assert_eq!(
                received.len(),
                1,
                "case {case}: expected the one sent message, got {received:?}"
            );
            let reported = attributes(&received[0]);
            let actual: BTreeSet<&str> = reported.keys().map(String::as_str).collect();
            assert_eq!(
                actual, wanted,
                "case {case}: expected the system attributes {wanted:?}, got {actual:?}"
            );
            if let Some(sent_at) = reported.get("SentTimestamp") {
                let parsed: u128 = sent_at.parse().unwrap_or_else(|_| {
                    panic!("case {case}: SentTimestamp {sent_at} is not a number")
                });
                // "the time the message was sent to the queue (epoch time in
                // milliseconds)", so it lies between the two clock readings this test
                // takes around the send.
                let now = epoch_millis(SystemTime::now());
                assert!(
                    (before..=now).contains(&parsed),
                    "case {case}: SentTimestamp {parsed} is not epoch milliseconds between \
                     {before} and {now}"
                );
            }
            per_member.push((member, reported.into_keys().collect()));
        }

        // The alias answers the same request as the member that replaced it. Only the
        // names are compared: a value such as `ApproximateReceiveCount` counts the
        // receive being answered, so it differs between two receives by design.
        let (first_member, first_keys) = &per_member[0];
        let (second_member, second_keys) = &per_member[1];
        assert_eq!(
            first_keys, second_keys,
            "case [{name}]: {first_member} reported {first_keys:?} but {second_member} \
             reported {second_keys:?}; the alias must answer identically"
        );
    }
}

/// A name outside the message system attribute set names no attribute, on either
/// member: the request is refused with `InvalidAttributeName` and nothing is handed
/// over. The rows walk the kinds of name that are outside the set.
#[tokio::test]
async fn a_name_outside_the_system_attribute_set_is_refused_on_both_members() {
    let router = emulator();
    let url = create_ok(&router, "alias-refusals", &json!({})).await;
    ok(
        &router,
        "SendMessage",
        json!({"QueueUrl": url, "MessageBody": "payload"}),
    )
    .await;

    // (case, the names the request supplies, the name the refusal must report)
    let rows: &[(&str, &[&str], &str)] = &[
        (
            "a queue attribute name, which is not a message attribute",
            &["VisibilityTimeout"],
            "VisibilityTimeout",
        ),
        (
            "another queue attribute name",
            &["ApproximateNumberOfMessages"],
            "ApproximateNumberOfMessages",
        ),
        ("a misspelling", &["SentTimestamps"], "SentTimestamps"),
        (
            "the wrong case: these names are case-sensitive",
            &["senttimestamp"],
            "senttimestamp",
        ),
        (
            "a selector only MessageAttributeNames accepts",
            &[".*"],
            ".*",
        ),
        ("a name of nothing", &["NotAnAttribute"], "NotAnAttribute"),
        (
            "one unknown name beside an accepted one",
            &["SentTimestamp", "NotAnAttribute"],
            "NotAnAttribute",
        ),
    ];

    for (case, requested, reported) in rows {
        for member in NAME_MEMBERS {
            let case = format!("{member} {case}");
            let request = json!({
                "QueueUrl": url,
                "VisibilityTimeout": 0,
                member: requested,
            });
            let (status, answer) = call(&router, "ReceiveMessage", &request).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "InvalidAttributeName"),
                "case {case}: expected a 400 InvalidAttributeName, got {status} with {answer}"
            );
            assert!(
                answer["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(reported)),
                "case {case}: the refusal must name {reported}, got {answer}"
            );
            assert!(
                messages(&answer).is_empty(),
                "case {case}: a refused receive hands nothing over, got {answer}"
            );
        }
    }
}

/// The alias serves the three names a FIFO send records, which the queue attribute enum
/// its model targets does not list at all: each comes back with the value that send
/// wrote into state.
#[tokio::test]
async fn the_alias_reports_the_system_attributes_a_fifo_send_records() {
    let router = emulator();
    let url = create_ok(&router, "alias.fifo", &json!({"FifoQueue": "true"})).await;
    let sent = ok(
        &router,
        "SendMessage",
        json!({
            "QueueUrl": url,
            "MessageBody": "payload",
            "MessageGroupId": "group-1",
            "MessageDeduplicationId": "token-1",
        }),
    )
    .await;
    let sequence_number = sent["SequenceNumber"]
        .as_str()
        .unwrap_or_else(|| panic!("case the FIFO send: no SequenceNumber in {sent}"))
        .to_string();

    let received = messages(
        &ok(
            &router,
            "ReceiveMessage",
            json!({"QueueUrl": url, "AttributeNames": ["All"]}),
        )
        .await,
    );
    assert_eq!(
        received.len(),
        1,
        "case the FIFO receive: expected the one sent message, got {received:?}"
    );
    let reported = attributes(&received[0]);
    for (name, expected) in [
        ("MessageGroupId", "group-1"),
        ("MessageDeduplicationId", "token-1"),
        ("SequenceNumber", sequence_number.as_str()),
    ] {
        let actual = reported.get(name).map(String::as_str).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case the FIFO receive: {name} must report what the send recorded, got {actual:?} \
             in {reported:?}"
        );
    }
}

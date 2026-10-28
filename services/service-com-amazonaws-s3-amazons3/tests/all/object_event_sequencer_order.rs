// SPDX-License-Identifier: CC0-1.0

//! The `sequencer` of the records one KEY's changes publish.
//!
//! The rule under test, from the Amazon S3 User Guide's *Event message structure*: the
//! `sequencer` is "a string representation of a hexadecimal value used to determine event
//! sequence, only used with PUTs and DELETEs", and "you can compare the hexadecimal
//! sequencer strings … to determine the correct sequence of events for a given object
//! key" — the comparison is lexicographic over strings of the same length, and it is the
//! ONLY order a consumer may rely on, because the deliveries themselves can arrive in any
//! order.
//!
//! So, for the changes of one key: every delivered record carries a sequencer, no two of
//! them share one, they are all the same width, and sorting the records by their sequencer
//! reproduces the order the calls were MADE in — which is what this suite asserts, by
//! identifying each record by the change it came of rather than by the order it arrived.
//!
//! The rows span what the rule generalises over: how many changes one key takes, which
//! calls make them (writes of different sizes, a tag set, a delete), and whether the key
//! is re-created after a removal.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::object_event_notifications::{queue_arn, Destination, Emulator};

/// How long a wait for a whole fan-out lasts before giving up. Each delivery is made on a
/// task of its own as soon as the call that produced it is answered, so this is many
/// deliveries' worth of slack and not a span the behaviour depends on.
const DELIVERY_BUDGET: Duration = Duration::from_secs(10);

/// The events every configuration below is notified about: the three classes the rows'
/// changes produce.
const CONFIGURED_EVENTS: &[&str] = &[
    "s3:ObjectCreated:*",
    "s3:ObjectRemoved:*",
    "s3:ObjectTagging:*",
];

/// The one key every change of a row is made on — the scope the sequencer orders.
const KEY: &str = "ledger/entry.bin";

/// One change a row makes to that key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// `PutObject` of a body of this many bytes, which is also what tells two writes of
    /// one key apart in the records they publish.
    Write(usize),
    /// `PutObjectTagging`, on a key holding this many bytes — a tagging leaves the object
    /// in place, so its record reports that stored size.
    Tag(usize),
    /// `DeleteObject`.
    Delete,
}

impl Change {
    /// The `eventName` and `s3.object.size` the record of this change carries — what
    /// identifies that record among a key's records, whatever order they arrived in.
    fn mark(self) -> (&'static str, Option<i64>) {
        match self {
            Self::Write(bytes) => ("ObjectCreated:Put", Some(bytes as i64)),
            Self::Tag(bytes) => ("ObjectTagging:Put", Some(bytes as i64)),
            Self::Delete => ("ObjectRemoved:Delete", None),
        }
    }
}

/// `bytes` bytes of body, so a write's record reports a size of exactly that.
fn payload(bytes: usize) -> Vec<u8> {
    vec![b'x'; bytes]
}

/// The `(eventName, size)` a delivered record names itself by, beside its sequencer.
fn reported(message: &Value) -> ((String, Option<i64>), String) {
    let record = &message["Records"][0];
    let name = record["eventName"].as_str().unwrap_or_default().to_string();
    let size = record["s3"]["object"]["size"].as_i64();
    let sequencer = record["s3"]["object"]["sequencer"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    ((name, size), sequencer)
}

/// Take messages off `url` until `wanted` of them have arrived or the budget runs out.
///
/// One receive takes a whole batch, so a fan-out that arrived together is never half
/// dropped — which is what makes "exactly this many records" assertable.
async fn collected(emulator: &Emulator, url: &str, wanted: usize) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut messages = Vec::new();
    loop {
        let received = emulator
            .sqs(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
            )
            .await;
        for message in received["Messages"].as_array().cloned().unwrap_or_default() {
            let body = message["Body"].as_str().unwrap_or_default();
            messages.push(serde_json::from_str(body).unwrap_or(Value::Null));
        }
        if messages.len() >= wanted || Instant::now() >= deadline {
            return messages;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn the_sequencer_orders_one_keys_changes_whatever_order_they_arrive_in() {
    // (case, the changes made to the key, in the order they are made)
    let rows: &[(&str, &[Change])] = &[
        (
            "three writes of one key",
            &[Change::Write(11), Change::Write(22), Change::Write(33)],
        ),
        (
            "a write, a tag set and a delete",
            &[Change::Write(10), Change::Tag(10), Change::Delete],
        ),
        (
            "a write, a delete and a write again",
            &[Change::Write(7), Change::Delete, Change::Write(9)],
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, changes)) in rows.iter().enumerate() {
        let bucket = format!("sequencer-{index}");
        let queue = format!("sequencer-queue-{index}");
        emulator.create_bucket(&bucket).await;
        let url = emulator.create_queue(&queue).await;
        emulator
            .put_notification(
                &bucket,
                &Destination::queue(&queue_arn(&queue)),
                CONFIGURED_EVENTS,
                &[],
                false,
            )
            .await;

        // The changes, in the order the rule orders them by.
        for change in *changes {
            match change {
                Change::Write(bytes) => {
                    emulator
                        .put_object(&bucket, KEY, &payload(*bytes))
                        .await;
                }
                Change::Tag(_) => {
                    emulator.put_object_tagging(&bucket, KEY).await;
                }
                Change::Delete => {
                    emulator.delete_object(&bucket, KEY).await;
                }
            }
        }

        let messages = collected(&emulator, &url, changes.len()).await;
        assert_eq!(
            messages.len(),
            changes.len(),
            "case {case:?}: expected one record per change, got {messages:?}"
        );
        let mut delivered: Vec<((String, Option<i64>), String)> =
            messages.iter().map(reported).collect();

        // Every record carries a sequencer, and the sequencers of one key are distinct and
        // all of one width — the two things that make a lexicographic comparison of them a
        // total order at all.
        let widths: Vec<usize> = delivered
            .iter()
            .map(|(_, sequencer)| sequencer.len())
            .collect();
        assert!(
            widths.iter().all(|width| *width == widths[0] && *width > 0),
            "case {case:?}: a key's sequencers must all be one non-empty width, got \
             {delivered:?}"
        );
        assert!(
            delivered
                .iter()
                .all(|(_, sequencer)| sequencer.chars().all(|digit| digit.is_ascii_hexdigit())),
            "case {case:?}: a sequencer is a hexadecimal value, got {delivered:?}"
        );
        let mut sequencers: Vec<&str> = delivered
            .iter()
            .map(|(_, sequencer)| sequencer.as_str())
            .collect();
        sequencers.sort_unstable();
        let distinct = sequencers.len();
        sequencers.dedup();
        assert_eq!(
            sequencers.len(),
            distinct,
            "case {case:?}: two changes of one key must not share a sequencer, got \
             {delivered:?}"
        );

        // THE RULE: sorting the records by their sequencer reproduces the order the calls
        // were made in, whichever order the deliveries themselves arrived in.
        delivered.sort_by(|left, right| left.1.cmp(&right.1));
        let ordered: Vec<(String, Option<i64>)> =
            delivered.iter().map(|(mark, _)| mark.clone()).collect();
        let expected: Vec<(String, Option<i64>)> = changes
            .iter()
            .map(|change| {
                let (name, size) = change.mark();
                (name.to_string(), size)
            })
            .collect();
        assert_eq!(
            ordered, expected,
            "case {case:?}: the sequencer order must be the order the changes were made \
             in, got {delivered:?}"
        );

        // And nothing else was published for the key: the records above are the whole
        // fan-out, so the order asserted is over all of it.
        let extra = emulator.nothing_delivered(&url).await;
        assert!(
            extra.is_empty(),
            "case {case:?}: the key's changes owe one record each and no more, got \
             {extra:?}"
        );
    }
}

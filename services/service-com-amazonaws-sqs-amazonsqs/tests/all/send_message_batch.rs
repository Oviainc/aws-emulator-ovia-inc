// SPDX-License-Identifier: CC0-1.0

//! `SendMessageBatch` end to end over the wire (`POST /` with `X-Amz-Target:
//! AmazonSQS.SendMessageBatch`) through the same `MultiServiceRouter` the emulator's
//! front doors use.
//!
//! The RULES asserted here, each over a TABLE of inputs rather than one example:
//!
//! * every entry a batch carries is enqueued in the queue it addresses, in request
//!   order, and reported in `Successful` under its own `Id` with the message's id and the
//!   MD5 of its body — read back through `ReceiveMessage`, not from the send's answer;
//! * a batch that is not one AWS would act on is refused WHOLE and enqueues nothing: no
//!   entries, more than ten entries, an entry `Id` outside the specification, two entries
//!   sharing an `Id`, a `QueueUrl` no queue answers to, and entries whose bytes together
//!   pass the queue's `MaximumMessageSize` (asserted on that bound and one byte past it);
//! * past those, each entry is judged on its own against the queue by the rules
//!   `SendMessage` applies to one message, so an entry the queue will not take is reported
//!   in `Failed` with the sender's fault while the rest of the batch is still enqueued;
//! * on a FIFO queue a batch is enqueued in the order it was sent, each entry is
//!   sequenced, and an entry repeating a deduplication token already accepted is reported
//!   successful for the message that token admitted without delivering a second copy.

use std::collections::BTreeSet;

use md5::{Digest, Md5};
use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{MAX_BATCH_ENTRIES, MAX_BATCH_ENTRY_ID_LEN};
use smithy_server_runtime::MultiServiceRouter;

use super::support;

/// The MD5 an answer must report for `body`, derived here from the member's own rule —
/// "an MD5 digest of the non-URL-encoded message body string" — rather than from the
/// emulator's helper.
fn md5_of(body: &str) -> String {
    hex::encode(Md5::digest(body.as_bytes()))
}

/// The `Successful` entries of a batch answer, in the order they were reported.
fn successful(answer: &Value) -> Vec<Value> {
    answer["Successful"].as_array().cloned().unwrap_or_default()
}

/// The `Failed` entries of a batch answer, in the order they were reported.
fn failed(answer: &Value) -> Vec<Value> {
    answer["Failed"].as_array().cloned().unwrap_or_default()
}

/// The `Id` of each entry in `entries`.
fn ids(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Receive up to `max` messages from the queue at `url`.
async fn receive(router: &MultiServiceRouter, url: &str, max: usize) -> Vec<Value> {
    let answer = support::ok(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": max }),
    )
    .await;
    support::messages(&answer)
}

/// How many messages the queue at `url` holds, visible and in flight.
async fn held(router: &MultiServiceRouter, url: &str) -> (i64, i64) {
    let (available, _delayed, in_flight) = support::message_counts(router, url).await;
    (available, in_flight)
}

/// Every entry of an acceptable batch is enqueued and reported under its own id, with the
/// message's id and the digest of its body; the messages are then handed over by a
/// receive. Rows differ along the batch's SIZE, up to the documented maximum.
#[tokio::test]
async fn every_entry_of_a_batch_is_enqueued_and_reported_under_its_own_id() {
    for size in [1usize, 2, 5, MAX_BATCH_ENTRIES] {
        let router = support::emulator();
        let url = support::create_ok(&router, &format!("send-batch-{size}"), &json!({})).await;

        let bodies: Vec<String> = (0..size).map(|index| format!("body-{index}")).collect();
        let entries: Vec<Value> = bodies
            .iter()
            .enumerate()
            .map(|(index, body)| json!({ "Id": format!("e{index}"), "MessageBody": body }))
            .collect();
        let answer = support::ok(
            &router,
            "SendMessageBatch",
            json!({ "QueueUrl": url, "Entries": entries }),
        )
        .await;

        let reported = successful(&answer);
        assert_eq!(
            ids(&reported),
            (0..size).map(|index| format!("e{index}")).collect::<Vec<_>>(),
            "case size {size}: expected every entry reported successful in request order, \
             got {answer}"
        );
        assert_eq!(
            failed(&answer).len(),
            0,
            "case size {size}: expected no failed entry, got {answer}"
        );

        let mut minted: BTreeSet<String> = BTreeSet::new();
        for (index, entry) in reported.iter().enumerate() {
            let message_id = entry["MessageId"].as_str().unwrap_or_default().to_string();
            assert!(
                !message_id.is_empty(),
                "case size {size} entry {index}: expected a MessageId, got {entry}"
            );
            assert!(
                minted.insert(message_id),
                "case size {size} entry {index}: every entry must mint its own MessageId, \
                 got {answer}"
            );
            assert_eq!(
                entry["MD5OfMessageBody"],
                json!(md5_of(&bodies[index])),
                "case size {size} entry {index}: expected the digest of its own body, got {entry}"
            );
            assert!(
                entry["SequenceNumber"].is_null(),
                "case size {size} entry {index}: a standard queue sequences nothing, got {entry}"
            );
        }

        assert_eq!(
            held(&router, &url).await,
            (i64::try_from(size).unwrap_or_default(), 0),
            "case size {size}: the queue must hold every message the batch enqueued"
        );

        // Read the messages back through the queue rather than trusting the answer.
        let received = receive(&router, &url, size).await;
        let mut handed: Vec<String> = received
            .iter()
            .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
            .collect();
        handed.sort();
        let mut expected = bodies.clone();
        expected.sort();
        assert_eq!(
            handed, expected,
            "case size {size}: expected the batch's bodies back, got {received:?}"
        );
        let handed_ids: BTreeSet<String> = received
            .iter()
            .map(|message| message["MessageId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            handed_ids, minted,
            "case size {size}: the ids handed over must be the ids the batch reported"
        );
    }
}

/// An entry carrying message attributes reports their digest as well as the body's, and
/// the attributes come back on the receive. Rows differ along how many attributes an
/// entry carries, including none.
#[tokio::test]
async fn an_entry_reports_the_digest_of_the_attributes_it_carries() {
    let rows: [(&str, Value, usize); 3] = [
        ("no attributes at all", json!(null), 0),
        (
            "one attribute",
            json!({ "colour": { "DataType": "String", "StringValue": "green" } }),
            1,
        ),
        (
            "two attributes",
            json!({
                "colour": { "DataType": "String", "StringValue": "green" },
                "size": { "DataType": "Number", "StringValue": "7" },
            }),
            2,
        ),
    ];

    for (case, attributes, count) in rows {
        let router = support::emulator();
        let url = support::create_ok(&router, "send-batch-attributes", &json!({})).await;
        let mut entry = json!({ "Id": "e0", "MessageBody": "payload" });
        if count > 0 {
            entry["MessageAttributes"] = attributes.clone();
        }
        let answer = support::ok(
            &router,
            "SendMessageBatch",
            json!({ "QueueUrl": url, "Entries": [entry] }),
        )
        .await;
        let reported = successful(&answer);
        assert_eq!(
            reported.len(),
            1,
            "case {case}: expected the entry accepted, got {answer}"
        );
        let digest = reported[0].get("MD5OfMessageAttributes").cloned();
        if count == 0 {
            assert!(
                digest.is_none(),
                "case {case}: an entry with no attributes reports no digest, got {answer}"
            );
        } else {
            assert!(
                digest
                    .as_ref()
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.len() == 32
                        && value.chars().all(|c| c.is_ascii_hexdigit())),
                "case {case}: expected an MD5 of the attributes, got {answer}"
            );
        }

        let received = support::ok(
            &router,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MessageAttributeNames": ["All"] }),
        )
        .await;
        let message = support::messages(&received)
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected the message back, got {received}"));
        assert_eq!(
            message["MessageAttributes"]
                .as_object()
                .map(|map| map.len())
                .unwrap_or(0),
            count,
            "case {case}: expected {count} attribute(s) back, got {message}"
        );
        assert_eq!(
            message["MD5OfMessageAttributes"],
            reported[0]["MD5OfMessageAttributes"],
            "case {case}: the receive must report the digest the send did, got {message}"
        );
    }
}

/// A batch AWS would not act on is refused WHOLE, with the error for WHY, and enqueues
/// nothing. Rows differ along what makes the batch unacceptable, including the payload
/// bound itself — at the bound, and one byte past it.
#[tokio::test]
async fn a_batch_that_is_not_acceptable_is_refused_whole() {
    /// The smallest `MaximumMessageSize` the attribute's documentation admits — "an
    /// integer from 1,024 bytes (1 KiB) to 1,048,576 bytes (1 MiB)" — so a row can reach
    /// the payload bound without a megabyte of body.
    const SMALL_MAXIMUM: usize = 1_024;

    struct Case {
        label: &'static str,
        attributes: Value,
        entries: Value,
        real_queue: bool,
        status: u16,
        code: &'static str,
    }
    let long_id = "i".repeat(MAX_BATCH_ENTRY_ID_LEN + 1);
    let cases = vec![
        Case {
            label: "no entries at all",
            attributes: json!({}),
            entries: json!([]),
            real_queue: true,
            status: 400,
            code: "EmptyBatchRequest",
        },
        Case {
            label: "one entry more than the documented maximum",
            attributes: json!({}),
            entries: Value::Array(
                (0..=MAX_BATCH_ENTRIES)
                    .map(|index| json!({ "Id": format!("e{index}"), "MessageBody": "payload" }))
                    .collect(),
            ),
            real_queue: true,
            status: 400,
            code: "TooManyEntriesInBatchRequest",
        },
        Case {
            label: "an entry id carrying a character the specification excludes",
            attributes: json!({}),
            entries: json!([{ "Id": "not ok!", "MessageBody": "payload" }]),
            real_queue: true,
            status: 400,
            code: "InvalidBatchEntryId",
        },
        Case {
            label: "an entry id past the documented length",
            attributes: json!({}),
            entries: json!([{ "Id": long_id, "MessageBody": "payload" }]),
            real_queue: true,
            status: 400,
            code: "InvalidBatchEntryId",
        },
        Case {
            label: "an empty entry id",
            attributes: json!({}),
            entries: json!([{ "Id": "", "MessageBody": "payload" }]),
            real_queue: true,
            status: 400,
            code: "InvalidBatchEntryId",
        },
        Case {
            label: "two entries sharing an id",
            attributes: json!({}),
            entries: json!([
                { "Id": "same", "MessageBody": "one" },
                { "Id": "same", "MessageBody": "two" },
            ]),
            real_queue: true,
            status: 400,
            code: "BatchEntryIdsNotDistinct",
        },
        Case {
            label: "a queue url no queue answers to",
            attributes: json!({}),
            entries: json!([{ "Id": "e0", "MessageBody": "payload" }]),
            real_queue: false,
            status: 400,
            code: "QueueDoesNotExist",
        },
        Case {
            label: "one entry one byte past the queue's maximum message size",
            attributes: json!({ "MaximumMessageSize": SMALL_MAXIMUM.to_string() }),
            entries: json!([{ "Id": "e0", "MessageBody": "x".repeat(SMALL_MAXIMUM + 1) }]),
            real_queue: true,
            status: 400,
            code: "BatchRequestTooLong",
        },
        Case {
            label: "entries whose bodies together pass the queue's maximum message size",
            attributes: json!({ "MaximumMessageSize": SMALL_MAXIMUM.to_string() }),
            entries: json!([
                { "Id": "e0", "MessageBody": "x".repeat(SMALL_MAXIMUM / 2) },
                { "Id": "e1", "MessageBody": "y".repeat(SMALL_MAXIMUM / 2 + 1) },
            ]),
            real_queue: true,
            status: 400,
            code: "BatchRequestTooLong",
        },
    ];

    for case in cases {
        let router = support::emulator();
        let url = support::create_ok(&router, "refused-send-batch", &case.attributes).await;
        let queue_url = if case.real_queue {
            url.clone()
        } else {
            support::expected_url("no-such-queue")
        };
        let (status, body) = support::call(
            &router,
            "SendMessageBatch",
            &json!({ "QueueUrl": queue_url, "Entries": case.entries }),
        )
        .await;
        assert_eq!(
            status, case.status,
            "case {}: expected HTTP {}, got {status} with {body}",
            case.label, case.status
        );
        assert_eq!(
            support::error_code(&body),
            case.code,
            "case {}: expected the {} code, got {body}",
            case.label,
            case.code
        );
        assert_eq!(
            held(&router, &url).await,
            (0, 0),
            "case {}: a refused batch must enqueue nothing",
            case.label
        );
    }

    // The published bound is a BOUNDARY: entries filling it exactly are accepted, while
    // the rows above show one byte more is not.
    let router = support::emulator();
    let url = support::create_ok(
        &router,
        "payload-bound",
        &json!({ "MaximumMessageSize": SMALL_MAXIMUM.to_string() }),
    )
    .await;
    let answer = support::ok(
        &router,
        "SendMessageBatch",
        json!({
            "QueueUrl": url,
            "Entries": [
                { "Id": "e0", "MessageBody": "x".repeat(SMALL_MAXIMUM / 2) },
                { "Id": "e1", "MessageBody": "y".repeat(SMALL_MAXIMUM / 2) },
            ],
        }),
    )
    .await;
    assert_eq!(
        successful(&answer).len(),
        2,
        "case exactly at the payload bound: expected both entries accepted, got {answer}"
    );
    assert_eq!(
        held(&router, &url).await,
        (2, 0),
        "case exactly at the payload bound: both messages must be held"
    );
}

/// An entry the queue will not take fails ALONE: it is reported in `Failed` with the
/// sender's fault and the code for its condition, and the other entries of the same batch
/// are still enqueued. Rows differ along WHY the entry is not acceptable, and along where
/// it sits in the batch.
#[tokio::test]
async fn an_entry_the_queue_will_not_take_fails_on_its_own() {
    // `(label, the queue's attributes, the entries, the id -> expected code per failure)`
    struct Case {
        label: &'static str,
        attributes: Value,
        entries: Value,
        failures: &'static [(&'static str, &'static str)],
        enqueued: &'static [&'static str],
    }
    let cases = vec![
        Case {
            label: "a body holding a character outside the allowed set",
            attributes: json!({}),
            entries: json!([
                { "Id": "bad", "MessageBody": "payload\u{0}" },
                { "Id": "good", "MessageBody": "payload" },
            ]),
            failures: &[("bad", "InvalidMessageContents")],
            enqueued: &["good"],
        },
        Case {
            label: "a FIFO member on a standard queue",
            attributes: json!({}),
            entries: json!([
                { "Id": "good", "MessageBody": "one" },
                { "Id": "bad", "MessageBody": "two", "MessageDeduplicationId": "token" },
            ]),
            failures: &[("bad", "InvalidParameterValue")],
            enqueued: &["good"],
        },
        Case {
            label: "a DelaySeconds past the range the attribute documents",
            attributes: json!({}),
            entries: json!([
                { "Id": "bad", "MessageBody": "one", "DelaySeconds": 901 },
                { "Id": "good", "MessageBody": "two" },
            ]),
            failures: &[("bad", "InvalidParameterValue")],
            enqueued: &["good"],
        },
        Case {
            label: "an entry with no MessageGroupId on a FIFO queue",
            attributes: json!({ "FifoQueue": "true", "ContentBasedDeduplication": "true" }),
            entries: json!([
                { "Id": "bad", "MessageBody": "one" },
                { "Id": "good", "MessageBody": "two", "MessageGroupId": "g" },
            ]),
            failures: &[("bad", "MissingParameter")],
            enqueued: &["good"],
        },
        Case {
            label: "two entries refused for two different reasons",
            attributes: json!({}),
            entries: json!([
                { "Id": "bad-body", "MessageBody": "payload\u{0}" },
                { "Id": "good", "MessageBody": "payload" },
                { "Id": "bad-delay", "MessageBody": "payload", "DelaySeconds": -1 },
            ]),
            failures: &[
                ("bad-body", "InvalidMessageContents"),
                ("bad-delay", "InvalidParameterValue"),
            ],
            enqueued: &["good"],
        },
    ];

    for case in cases {
        let router = support::emulator();
        let fifo = case.attributes["FifoQueue"] == json!("true");
        let name = if fifo {
            "mixed-send-batch.fifo"
        } else {
            "mixed-send-batch"
        };
        let url = support::create_ok(&router, name, &case.attributes).await;

        let answer = support::ok(
            &router,
            "SendMessageBatch",
            json!({ "QueueUrl": url, "Entries": case.entries }),
        )
        .await;

        assert_eq!(
            ids(&successful(&answer)),
            case.enqueued
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {}: expected only the acceptable entries enqueued, got {answer}",
            case.label
        );
        let refused = failed(&answer);
        let mut reported = ids(&refused);
        reported.sort();
        let mut expected: Vec<String> = case
            .failures
            .iter()
            .map(|(id, _)| (*id).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {}: expected every unacceptable entry reported per entry, got {answer}",
            case.label
        );
        for (id, code) in case.failures {
            let entry = refused
                .iter()
                .find(|entry| entry["Id"] == json!(id))
                .unwrap_or_else(|| panic!("case {}: no entry {id} in {answer}", case.label));
            assert_eq!(
                entry["Code"],
                json!(code),
                "case {}: expected the {code} code for {id}, got {entry}",
                case.label
            );
            assert_eq!(
                entry["SenderFault"],
                json!(true),
                "case {}: a caller-supplied entry is the sender's fault, got {entry}",
                case.label
            );
            assert!(
                entry["Message"].as_str().is_some_and(|text| !text.is_empty()),
                "case {}: expected a reason for {id}, got {entry}",
                case.label
            );
        }

        assert_eq!(
            held(&router, &url).await,
            (i64::try_from(case.enqueued.len()).unwrap_or_default(), 0),
            "case {}: only the accepted entries may be held",
            case.label
        );
    }
}

/// On a FIFO queue the batch is enqueued in the order it was sent, every entry is
/// sequenced, and the queue hands the group's messages back in that order. Rows differ
/// along how the deduplication token is obtained — supplied per entry, or derived from the
/// body under `ContentBasedDeduplication`.
#[tokio::test]
async fn a_fifo_batch_is_enqueued_in_the_order_it_was_sent() {
    let rows: [(&str, bool); 2] = [
        ("tokens supplied per entry", false),
        ("tokens derived from the body", true),
    ];
    for (case, content_based) in rows {
        let router = support::emulator();
        let mut attributes = json!({ "FifoQueue": "true" });
        if content_based {
            attributes["ContentBasedDeduplication"] = json!("true");
        }
        let url = support::create_ok(&router, "ordered-batch.fifo", &attributes).await;

        let bodies = ["first", "second", "third", "fourth"];
        let entries: Vec<Value> = bodies
            .iter()
            .enumerate()
            .map(|(index, body)| {
                let mut entry = json!({
                    "Id": format!("e{index}"),
                    "MessageBody": body,
                    "MessageGroupId": "group",
                });
                if !content_based {
                    entry["MessageDeduplicationId"] = json!(format!("token-{index}"));
                }
                entry
            })
            .collect();
        let answer = support::ok(
            &router,
            "SendMessageBatch",
            json!({ "QueueUrl": url, "Entries": entries }),
        )
        .await;

        let reported = successful(&answer);
        assert_eq!(
            ids(&reported),
            (0..bodies.len())
                .map(|index| format!("e{index}"))
                .collect::<Vec<_>>(),
            "case {case}: expected every entry accepted in request order, got {answer}"
        );
        let sequences: Vec<u128> = reported
            .iter()
            .map(|entry| {
                entry["SequenceNumber"]
                    .as_str()
                    .and_then(|value| value.parse::<u128>().ok())
                    .unwrap_or_else(|| panic!("case {case}: no SequenceNumber in {entry}"))
            })
            .collect();
        assert!(
            sequences.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case}: a FIFO batch must sequence its entries in send order, got {sequences:?}"
        );

        // The queue hands the group's messages back in exactly the order the batch sent
        // them: "for a FIFO queue, multiple messages within a single batch are enqueued
        // in the order they are sent".
        let received = receive(&router, &url, bodies.len()).await;
        let handed: Vec<String> = received
            .iter()
            .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            handed,
            bodies.map(str::to_string).to_vec(),
            "case {case}: expected the batch's own order back, got {received:?}"
        );
    }
}

/// An entry repeating a deduplication token the queue has already accepted is accepted
/// without a second copy being delivered, and reports the message that token admitted.
/// Rows differ along whether the token was already accepted by an EARLIER entry of the
/// same batch or by an earlier send.
#[tokio::test]
async fn a_repeated_deduplication_token_in_a_batch_delivers_one_message() {
    let rows: [(&str, bool); 2] = [
        ("the token repeats inside one batch", true),
        ("the token was accepted by an earlier send", false),
    ];
    for (case, inside_batch) in rows {
        let router = support::emulator();
        let url = support::create_ok(&router, "dedup-batch.fifo", &json!({ "FifoQueue": "true" }))
            .await;

        let first_id = if inside_batch {
            None
        } else {
            let sent = support::ok(
                &router,
                "SendMessage",
                json!({
                    "QueueUrl": url,
                    "MessageBody": "one",
                    "MessageGroupId": "group",
                    "MessageDeduplicationId": "token",
                }),
            )
            .await;
            Some(sent["MessageId"].as_str().unwrap_or_default().to_string())
        };

        let mut entries = vec![json!({
            "Id": "second",
            "MessageBody": "two",
            "MessageGroupId": "group",
            "MessageDeduplicationId": "token",
        })];
        if inside_batch {
            entries.insert(
                0,
                json!({
                    "Id": "first",
                    "MessageBody": "one",
                    "MessageGroupId": "group",
                    "MessageDeduplicationId": "token",
                }),
            );
        }
        let answer = support::ok(
            &router,
            "SendMessageBatch",
            json!({ "QueueUrl": url, "Entries": entries }),
        )
        .await;

        let reported = successful(&answer);
        assert_eq!(
            reported.len(),
            if inside_batch { 2 } else { 1 },
            "case {case}: every entry carrying an accepted token is still accepted, got {answer}"
        );
        let admitted = first_id.unwrap_or_else(|| {
            reported[0]["MessageId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        });
        let repeat = reported
            .iter()
            .find(|entry| entry["Id"] == json!("second"))
            .unwrap_or_else(|| panic!("case {case}: no repeated entry in {answer}"));
        assert_eq!(
            repeat["MessageId"],
            json!(admitted),
            "case {case}: the repeat must answer for the message the token admitted, got {answer}"
        );
        assert_eq!(
            held(&router, &url).await,
            (1, 0),
            "case {case}: the queue must hold one message for one accepted token"
        );
    }
}

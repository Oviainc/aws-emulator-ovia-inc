// SPDX-License-Identifier: CC0-1.0

//! WHICH QUEUE a receipt handle belongs to, driven end to end over the wire (`POST /`
//! with `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in
//! [`super::support`].
//!
//! The RULE asserted here: a receipt handle names the queue incarnation that minted it,
//! so an operation taking one answers for the queue the HANDLE names rather than for
//! whatever a live handle table happens to hold. A handle of the queue addressed resolves
//! (and a delete of it stays idempotent once its message is gone); a handle another queue
//! that STILL EXISTS minted is refused as a handle that is not valid for this queue; a
//! handle an incarnation that is GONE minted is refused as a queue that does not exist —
//! a queue created again under a deleted queue's name is a new incarnation, so the
//! handles the old one minted name the deleted queue and not the new one; and a value no
//! receive ever handed out is refused as not a receipt handle at all.
//!
//! The rows span what the rule generalises over: where the handle came from (the queue
//! addressed, another standard queue, another FIFO queue, a previous incarnation of the
//! name addressed, a queue deleted and never created again, nothing at all), the FORM of
//! a value nothing minted (shorter than a handle, one token of a minted identifier's
//! length, two such tokens joined the way a minted handle joins its parts) and the
//! operation the handle is presented to — `DeleteMessage`, `ChangeMessageVisibility` and
//! both of their batches, where the entry carrying such a handle is the only one that
//! fails.
//!
//! A name a delete removed becomes available again only once the deletion has propagated,
//! which no row sleeps for: every row first asserts that the re-create straight after the
//! delete is refused, then reaches that instant through the engine seam
//! [`super::support::emulator_over_engine`] hands back.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::ComAmazonawsSqsAmazonsqsState;
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    call, create, create_ok, emulator_over_engine, error_code, fifo_attributes, message_counts,
    messages, ok,
};

/// The sentence Amazon SQS answers for a value supplied where a receipt handle was
/// expected that is not a receipt handle at all: fixed text, naming no value.
const NOT_A_RECEIPT_HANDLE: &str = "The input receipt handle is invalid.";

/// The sentence every refusal of a queue that does not exist carries.
const NO_SUCH_QUEUE: &str = "The specified queue does not exist.";

/// The visibility timeout a row receives under, so the message it holds stays in flight
/// for the whole test. Inside the documented range of
/// `ReceiveMessageRequest$VisibilityTimeout` ("Values range: 0 to 43200").
const HELD_UNDER: i64 = 600;

/// The span a batch row asks for, inside the same documented range.
const CHANGED_TO: i64 = 60;

/// An opaque value of one minted identifier's length that no receive handed out.
const UNMINTED_IDENTIFIER: &str = "0f9a1cb7-4f63-4a45-9b0e-2b5f7c0f2d11";

/// Two tokens of a minted identifier's LENGTH that are not minted identifiers, joined the
/// way a minted handle joins its parts: the shape of a handle is not provenance, so this
/// names no queue either.
const UNMINTED_PAIR: &str =
    "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz/zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz";

/// How far into the past a pending deletion is moved for it to have propagated: longer
/// than any span the store defers a deleted name for.
const PROPAGATED_BY: Duration = Duration::from_secs(60);

/// How the refusal of a handle another queue minted names it.
fn another_queues_handle(receipt_handle: &str) -> String {
    format!("The receipt handle {receipt_handle:?} is not valid for this queue.")
}

/// The error code and message a refusal carries, as the awsJson envelope carries them.
fn refusal(body: &Value) -> (String, String) {
    (
        error_code(body),
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// Send one message to the queue at `url`; a FIFO queue's send names its message group.
async fn send_one(router: &MultiServiceRouter, url: &str, fifo: bool) {
    let mut body = json!({"QueueUrl": url, "MessageBody": "payload"});
    if fifo {
        body["MessageGroupId"] = json!("group");
    }
    let _ = ok(router, "SendMessage", body).await;
}

/// Receive exactly one message from the queue at `url` and answer the receipt handle it
/// was handed over with.
async fn receive_handle(router: &MultiServiceRouter, url: &str) -> String {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 1,
            "VisibilityTimeout": HELD_UNDER,
            // Inside the documented `WaitTimeSeconds` range ("an integer from 0 to 20"),
            // so a receive does not have to race the send it follows.
            "WaitTimeSeconds": 1,
        }),
    )
    .await;
    let handed = messages(&reply);
    assert_eq!(
        handed.len(),
        1,
        "case a receive of {url}: expected one message, got {reply}"
    );
    handed[0]["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case a receive of {url}: no ReceiptHandle in {reply}"))
        .to_string()
}

/// The name and attributes of one queue of a row: a FIFO queue carries the `.fifo` suffix
/// and the attribute that makes it one.
fn queue_of(slug: &str, role: &str, fifo: bool) -> (String, Value) {
    if fifo {
        (
            format!("{slug}-{role}.fifo"),
            fifo_attributes(&[("ContentBasedDeduplication", "true")]),
        )
    } else {
        (format!("{slug}-{role}"), json!({}))
    }
}

/// Move the pending deletion of the name `name` into the past, so the name has become
/// available again without the row waiting for it.
fn deletion_propagated(engine: &engine::InMemoryEngine, name: &str) {
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    let deleted_at = store
        .recently_deleted
        .get_mut(name)
        .unwrap_or_else(|| panic!("case the deletion of {name}: no deletion is pending"));
    *deleted_at = SystemTime::now()
        .checked_sub(PROPAGATED_BY)
        .unwrap_or_else(SystemTime::now);
}

/// Delete the queue called `name` at `url` and create that name again, answering the url
/// of the queue the second create made.
///
/// The re-create straight after the delete is refused, and once the deletion has
/// propagated the same name is served again — at the same url, as a NEW incarnation, which
/// is what makes a handle the first incarnation minted a handle of a queue that is gone.
async fn recreated(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    name: &str,
    attributes: &Value,
    url: &str,
) -> String {
    let _ = ok(router, "DeleteQueue", json!({"QueueUrl": url})).await;
    let (status, parsed) = create(router, name, attributes).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "QueueDeletedRecently"),
        "case re-creating {name} straight after its delete: expected the refusal, got \
         {status} with {parsed}"
    );
    deletion_propagated(engine, name);
    let again = create_ok(router, name, attributes).await;
    assert_eq!(
        again, url,
        "case re-creating {name}: a queue of the same name keeps its url, got {again}"
    );
    again
}

/// Where the handle a row presents came from.
#[derive(Debug, Clone, Copy)]
enum Minted {
    /// The queue the request addresses, which still holds the message it names.
    ByTheQueueAddressed,
    /// The queue the request addresses, and a delete has already consumed it.
    ByTheQueueAddressedAndConsumed,
    /// Another queue of this store that still exists.
    ByAnotherQueue { fifo: bool },
    /// An incarnation of the name the request addresses that a delete removed, before the
    /// name was created again.
    ByAPreviousIncarnation { fifo: bool },
    /// A queue that was deleted and whose name nothing created again.
    ByAQueueSinceDeleted,
    /// Nothing: the row presents a value no receive ever handed out.
    Never(&'static str),
}

/// What the call carrying such a handle must answer.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The handle resolves, so the call is served.
    Served,
    /// Not a receipt handle at all.
    NotAHandle,
    /// A handle, minted by a queue that still exists.
    AnotherQueues,
    /// A handle of a queue that is gone.
    QueueIsGone,
}

/// The status, error code and message an expectation stands for, or `None` when the call
/// is to be served. The handle is named only where the refusal names it.
fn as_refusal(expected: Expected, receipt_handle: &str) -> Option<(u16, &'static str, String)> {
    match expected {
        Expected::Served => None,
        Expected::NotAHandle => Some((
            404,
            "ReceiptHandleIsInvalid",
            NOT_A_RECEIPT_HANDLE.to_string(),
        )),
        Expected::AnotherQueues => Some((
            404,
            "ReceiptHandleIsInvalid",
            another_queues_handle(receipt_handle),
        )),
        Expected::QueueIsGone => Some((400, "QueueDoesNotExist", NO_SUCH_QUEUE.to_string())),
    }
}

/// Build the row's world and answer `(the url the request addresses, the handle it
/// carries)`. Every queue a row makes is named after that row, so the rows do not share
/// state.
async fn presented(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    slug: &str,
    minted: Minted,
) -> (String, String) {
    match minted {
        Minted::ByTheQueueAddressed => {
            let (name, attributes) = queue_of(slug, "own", false);
            let url = create_ok(router, &name, &attributes).await;
            send_one(router, &url, false).await;
            let handle = receive_handle(router, &url).await;
            (url, handle)
        }
        Minted::ByTheQueueAddressedAndConsumed => {
            let (name, attributes) = queue_of(slug, "own", false);
            let url = create_ok(router, &name, &attributes).await;
            send_one(router, &url, false).await;
            let handle = receive_handle(router, &url).await;
            let (status, answer) = call(
                router,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {slug}: the first delete of {handle} must be served, got {status} with \
                 {answer}"
            );
            (url, handle)
        }
        Minted::ByAnotherQueue { fifo } => {
            let (name, attributes) = queue_of(slug, "addressed", false);
            let url = create_ok(router, &name, &attributes).await;
            let (theirs, their_attributes) = queue_of(slug, "other", fifo);
            let their_url = create_ok(router, &theirs, &their_attributes).await;
            send_one(router, &their_url, fifo).await;
            let handle = receive_handle(router, &their_url).await;
            (url, handle)
        }
        Minted::ByAPreviousIncarnation { fifo } => {
            let (name, attributes) = queue_of(slug, "reborn", fifo);
            let url = create_ok(router, &name, &attributes).await;
            send_one(router, &url, fifo).await;
            let handle = receive_handle(router, &url).await;
            let again = recreated(router, engine, &name, &attributes, &url).await;
            (again, handle)
        }
        Minted::ByAQueueSinceDeleted => {
            let (name, attributes) = queue_of(slug, "addressed", false);
            let url = create_ok(router, &name, &attributes).await;
            let (gone, gone_attributes) = queue_of(slug, "gone", false);
            let gone_url = create_ok(router, &gone, &gone_attributes).await;
            send_one(router, &gone_url, false).await;
            let handle = receive_handle(router, &gone_url).await;
            let _ = ok(router, "DeleteQueue", json!({"QueueUrl": gone_url})).await;
            (url, handle)
        }
        Minted::Never(value) => {
            let (name, attributes) = queue_of(slug, "addressed", false);
            let url = create_ok(router, &name, &attributes).await;
            send_one(router, &url, false).await;
            (url, value.to_string())
        }
    }
}

/// `DeleteMessage` answers for the queue the HANDLE names: its own handle deletes (and
/// repeating it is idempotent), another live queue's handle is refused as that queue's, a
/// handle of an incarnation that is gone is refused as a queue that does not exist, and a
/// value nothing minted is refused as not a handle — none of which changes what the queue
/// addressed holds.
#[tokio::test]
async fn delete_message_answers_for_the_queue_the_handle_names() {
    let (router, engine) = emulator_over_engine();
    let rows: &[(&str, Minted, Expected)] = &[
        (
            "the queue's own live handle",
            Minted::ByTheQueueAddressed,
            Expected::Served,
        ),
        (
            "its own handle a delete already consumed",
            Minted::ByTheQueueAddressedAndConsumed,
            Expected::Served,
        ),
        (
            "another standard queue's handle",
            Minted::ByAnotherQueue { fifo: false },
            Expected::AnotherQueues,
        ),
        (
            "another FIFO queue's handle",
            Minted::ByAnotherQueue { fifo: true },
            Expected::AnotherQueues,
        ),
        (
            "a previous incarnation of the name addressed",
            Minted::ByAPreviousIncarnation { fifo: false },
            Expected::QueueIsGone,
        ),
        (
            "a previous FIFO incarnation of the name addressed",
            Minted::ByAPreviousIncarnation { fifo: true },
            Expected::QueueIsGone,
        ),
        (
            "a queue that was deleted and never created again",
            Minted::ByAQueueSinceDeleted,
            Expected::QueueIsGone,
        ),
        (
            "a value shorter than any handle",
            Minted::Never("garbage"),
            Expected::NotAHandle,
        ),
        (
            "one unminted token of a minted identifier's length",
            Minted::Never(UNMINTED_IDENTIFIER),
            Expected::NotAHandle,
        ),
        (
            "two unminted tokens joined as a minted handle joins its parts",
            Minted::Never(UNMINTED_PAIR),
            Expected::NotAHandle,
        ),
    ];

    for (index, (case, minted, expected)) in rows.iter().enumerate() {
        let slug = format!("deleting-{index}");
        let (url, handle) = presented(&router, &engine, &slug, *minted).await;
        let before = message_counts(&router, &url).await;
        let (status, answer) = call(
            &router,
            "DeleteMessage",
            &json!({"QueueUrl": url, "ReceiptHandle": handle}),
        )
        .await;
        match as_refusal(*expected, &handle) {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected the delete to be served, got {status} with {answer}"
                );
                // Read back through the queue's own counters: the message it named is gone.
                let after = message_counts(&router, &url).await;
                assert_eq!(
                    after,
                    (0, 0, 0),
                    "case {case}: the message must be gone, got {after:?}"
                );
            }
            Some((wanted_status, wanted_code, wanted_message)) => {
                let (code, message) = refusal(&answer);
                assert_eq!(
                    (status, code.as_str(), message.as_str()),
                    (wanted_status, wanted_code, wanted_message.as_str()),
                    "case {case}: got {status} with {answer}"
                );
                let after = message_counts(&router, &url).await;
                assert_eq!(
                    after, before,
                    "case {case}: a refused delete must change nothing, before {before:?} and \
                     after {after:?}"
                );
            }
        }
    }
}

/// `ChangeMessageVisibility` classifies a handle by the same rule: its own handle changes
/// the message it names, another live queue's handle is refused as that queue's, and a
/// handle of a queue that is gone is refused as a queue that does not exist.
#[tokio::test]
async fn change_message_visibility_answers_for_the_queue_the_handle_names() {
    let (router, engine) = emulator_over_engine();
    let rows: &[(&str, Minted, Expected)] = &[
        (
            "the queue's own live handle",
            Minted::ByTheQueueAddressed,
            Expected::Served,
        ),
        (
            "another standard queue's handle",
            Minted::ByAnotherQueue { fifo: false },
            Expected::AnotherQueues,
        ),
        (
            "another FIFO queue's handle",
            Minted::ByAnotherQueue { fifo: true },
            Expected::AnotherQueues,
        ),
        (
            "a previous incarnation of the name addressed",
            Minted::ByAPreviousIncarnation { fifo: false },
            Expected::QueueIsGone,
        ),
        (
            "a queue that was deleted and never created again",
            Minted::ByAQueueSinceDeleted,
            Expected::QueueIsGone,
        ),
    ];

    for (index, (case, minted, expected)) in rows.iter().enumerate() {
        let slug = format!("changing-{index}");
        let (url, handle) = presented(&router, &engine, &slug, *minted).await;
        let before = message_counts(&router, &url).await;
        let (status, answer) = call(
            &router,
            "ChangeMessageVisibility",
            &json!({
                "QueueUrl": url,
                "ReceiptHandle": handle,
                "VisibilityTimeout": CHANGED_TO,
            }),
        )
        .await;
        match as_refusal(*expected, &handle) {
            None => assert_eq!(
                status, 200,
                "case {case}: expected the change to be served, got {status} with {answer}"
            ),
            Some((wanted_status, wanted_code, wanted_message)) => {
                let (code, message) = refusal(&answer);
                assert_eq!(
                    (status, code.as_str(), message.as_str()),
                    (wanted_status, wanted_code, wanted_message.as_str()),
                    "case {case}: got {status} with {answer}"
                );
            }
        }
        // Whatever the answer, the message plane of the queue addressed is untouched: a
        // visibility change moves no message in or out of the queue.
        let after = message_counts(&router, &url).await;
        assert_eq!(
            after, before,
            "case {case}: the queue must still hold what it held, before {before:?} and after \
             {after:?}"
        );
    }
}

/// The failed entries of a batch answer, keyed by `Id`, as `(code, message, sender
/// fault)`.
fn failures(answer: &Value) -> BTreeMap<String, (String, String, bool)> {
    answer
        .get("Failed")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Id"].as_str().unwrap_or_default().to_string(),
                (
                    entry["Code"].as_str().unwrap_or_default().to_string(),
                    entry["Message"].as_str().unwrap_or_default().to_string(),
                    entry["SenderFault"].as_bool().unwrap_or_default(),
                ),
            )
        })
        .collect()
}

/// The `Id`s a batch answer reports as successful.
fn served(answer: &Value) -> Vec<String> {
    answer
        .get("Successful")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A batch judges each entry by the queue its own handle names: the entry carrying a live
/// handle is served while the entry carrying another queue's handle fails as that queue's
/// handle and the entry carrying a gone incarnation's handle fails as a queue that does
/// not exist — each with the code the single-message operation answers for the same
/// handle.
#[tokio::test]
async fn a_batch_entry_fails_for_the_queue_its_own_handle_names() {
    let (router, engine) = emulator_over_engine();
    let name = "batch-provenance";
    let attributes = json!({});

    // A handle of an incarnation of the name addressed that a delete removed.
    let first = create_ok(&router, name, &attributes).await;
    send_one(&router, &first, false).await;
    let stale = receive_handle(&router, &first).await;
    let url = recreated(&router, &engine, name, &attributes, &first).await;

    // A live handle of the queue addressed, and one of a queue that still exists.
    send_one(&router, &url, false).await;
    let live = receive_handle(&router, &url).await;
    let their_url = create_ok(&router, "batch-provenance-other", &attributes).await;
    send_one(&router, &their_url, false).await;
    let foreign = receive_handle(&router, &their_url).await;

    // The visibility change comes first: the delete consumes the live handle's message.
    for operation in ["ChangeMessageVisibilityBatch", "DeleteMessageBatch"] {
        let entries: Vec<Value> = [("live", &live), ("foreign", &foreign), ("stale", &stale)]
            .iter()
            .map(|(id, handle)| {
                let mut entry = json!({"Id": id, "ReceiptHandle": handle});
                if operation == "ChangeMessageVisibilityBatch" {
                    entry["VisibilityTimeout"] = json!(CHANGED_TO);
                }
                entry
            })
            .collect();
        let (status, answer) = call(
            &router,
            operation,
            &json!({"QueueUrl": url, "Entries": entries}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation}: a batch whose entries differ is answered per entry, got {status} \
             with {answer}"
        );
        assert_eq!(
            served(&answer),
            vec!["live".to_string()],
            "case {operation}: only the live handle's entry is served, got {answer}"
        );
        let failed = failures(&answer);
        assert_eq!(
            failed.get("foreign"),
            Some(&(
                "ReceiptHandleIsInvalid".to_string(),
                another_queues_handle(&foreign),
                true,
            )),
            "case {operation}: the entry of another queue's handle must fail as that handle, \
             got {failed:?}"
        );
        assert_eq!(
            failed.get("stale"),
            Some(&(
                "AWS.SimpleQueueService.NonExistentQueue".to_string(),
                NO_SUCH_QUEUE.to_string(),
                true,
            )),
            "case {operation}: the entry of a gone incarnation's handle must fail as a queue \
             that does not exist, got {failed:?}"
        );
    }

    // What the batch deleted is gone, read back through the queue's own counters.
    let after = message_counts(&router, &url).await;
    assert_eq!(
        after,
        (0, 0, 0),
        "case after the batches: the deleted message must be gone, got {after:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PurgeQueue`, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rules pinned here, each over a TABLE of inputs rather than one example:
//!
//! 1. A purge discards EVERY message the queue holds — "deletes available messages in a
//!    queue (including in-flight messages)" — whatever that message's visibility, so the
//!    three `ApproximateNumberOfMessages*` counts all fall to zero, a later receive hands
//!    nothing over and neither the receipt handle an earlier receive minted nor the batch
//!    a `ReceiveRequestAttemptId` remembers names anything. The rows span the queue type
//!    and which visibilities the queue held when it was purged.
//! 2. The queue itself survives: its configuration attributes and its sequence counter
//!    are what they were, because a purge empties a queue rather than re-creating it.
//! 3. A deduplication token stops suppressing a resend once the message it admitted has
//!    been purged: "you can't retrieve any messages deleted from a queue", so the resend
//!    is a message the queue no longer holds and is delivered.
//! 4. A purge arriving while the one before it is still deleting — "within the last 60
//!    seconds" — is refused with `PurgeQueueInProgress` and changes nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    all_attributes, call, create_ok, emulator, error_code, message_counts, messages, ok,
};

/// The longest a send may ask a message to wait before becoming available, from
/// `SendMessageRequest$DelaySeconds` ("Valid values: 0 to 900"). Used for the rows that
/// need a message to still be delayed when the purge arrives.
const MAX_DELAY_SECONDS: u32 = 900;

/// One row of the purge table: the queue it is driven against and how many messages of
/// each visibility that queue holds when the purge arrives.
struct Row {
    case: &'static str,
    /// The queue name, which carries the `.fifo` suffix exactly when `fifo` is set.
    name: &'static str,
    fifo: bool,
    /// Attributes beyond the FIFO designation, as `(name, value)`.
    attributes: &'static [(&'static str, &'static str)],
    visible: usize,
    in_flight: usize,
    /// How many messages are still waiting out a delay. On a standard queue the send
    /// carries the delay; on a FIFO queue, where a per-message delay is refused, the
    /// queue's own `DelaySeconds` is what delays them.
    delayed: usize,
}

/// One row of the deduplication table: the case, the queue's name, the attributes it is
/// created with and the token each send carries (`None` when the queue derives it).
type DeduplicationRow<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], Option<&'a str>);

/// The `Attributes` map a queue is created with.
fn queue_attributes(fifo: bool, extra: &[(&str, &str)]) -> Value {
    let mut attributes = if fifo {
        json!({ "FifoQueue": "true" })
    } else {
        json!({})
    };
    for (name, value) in extra {
        attributes[*name] = json!(value);
    }
    attributes
}

/// Send one message, carrying the members the queue type requires: a FIFO send needs a
/// `MessageGroupId` and a deduplication token, and refuses a per-message delay.
async fn send_one(
    router: &MultiServiceRouter,
    url: &str,
    fifo: bool,
    body: &str,
    delay: Option<u32>,
) -> Value {
    let mut request = json!({ "QueueUrl": url, "MessageBody": body });
    if fifo {
        // Each message gets its own group, so a group with one message in flight never
        // holds another message back from the receive these rows drive.
        request["MessageGroupId"] = json!(format!("group-{body}"));
        request["MessageDeduplicationId"] = json!(format!("token-{body}"));
    } else if let Some(seconds) = delay {
        request["DelaySeconds"] = json!(seconds);
    }
    ok(router, "SendMessage", request).await
}

/// Receive up to `max` messages from the queue at `url`, leaving them in flight.
async fn receive(router: &MultiServiceRouter, url: &str, max: u32) -> Vec<Value> {
    let answer = ok(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": max }),
    )
    .await;
    messages(&answer)
}

/// Purge the queue at `url`, which must be accepted, and answer the reply.
async fn purge_ok(router: &MultiServiceRouter, url: &str) -> Value {
    ok(router, "PurgeQueue", json!({ "QueueUrl": url })).await
}

/// The configuration a queue carries: everything `GetQueueAttributes` reports except the
/// three counts over the messages it holds, which a purge is expected to change.
async fn configuration(router: &MultiServiceRouter, url: &str) -> BTreeMap<String, String> {
    let mut reported = all_attributes(router, url).await;
    reported.retain(|name, _| !name.starts_with("ApproximateNumberOfMessages"));
    reported
}

/// Bring the queue described by `row` to the state its row names, and answer the receipt
/// handles the messages put in flight were handed over under.
async fn fill(router: &MultiServiceRouter, row: &Row, url: &str) -> Vec<String> {
    // The messages that end up in flight are sent and received first, so the receive
    // cannot take the ones that have to stay visible.
    for index in 0..row.in_flight {
        send_one(router, url, row.fifo, &format!("{}-flight-{index}", row.name), None).await;
    }
    let mut handles: Vec<String> = Vec::new();
    if row.in_flight > 0 {
        let received = receive(router, url, u32::try_from(row.in_flight).unwrap_or(1)).await;
        assert_eq!(
            received.len(),
            row.in_flight,
            "case {}: expected {} message(s) to go in flight, got {received:?}",
            row.case,
            row.in_flight
        );
        handles = received
            .iter()
            .map(|message| {
                message["ReceiptHandle"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {}: no ReceiptHandle in {message}", row.case))
                    .to_string()
            })
            .collect();
    }
    for index in 0..row.visible {
        send_one(router, url, row.fifo, &format!("{}-visible-{index}", row.name), None).await;
    }
    for index in 0..row.delayed {
        send_one(
            router,
            url,
            row.fifo,
            &format!("{}-delayed-{index}", row.name),
            Some(MAX_DELAY_SECONDS),
        )
        .await;
    }
    handles
}

/// Every message a queue holds goes, whatever its visibility, and the queue's own
/// configuration survives. The rows span the queue type (standard and FIFO), the
/// visibilities the queue holds when the purge arrives (visible, in flight, delayed, all
/// three together) and the empty queue.
#[tokio::test]
async fn a_purge_discards_every_message_whatever_its_visibility() {
    let rows: &[Row] = &[
        Row {
            case: "a standard queue holding only visible messages",
            name: "purge-visible",
            fifo: false,
            attributes: &[],
            visible: 3,
            in_flight: 0,
            delayed: 0,
        },
        Row {
            case: "a standard queue with messages in flight",
            name: "purge-in-flight",
            fifo: false,
            attributes: &[],
            visible: 1,
            in_flight: 2,
            delayed: 0,
        },
        Row {
            case: "a standard queue holding only delayed messages",
            name: "purge-delayed",
            fifo: false,
            attributes: &[],
            visible: 0,
            in_flight: 0,
            delayed: 2,
        },
        Row {
            case: "a standard queue holding all three visibilities",
            name: "purge-every-visibility",
            fifo: false,
            attributes: &[],
            visible: 2,
            in_flight: 1,
            delayed: 1,
        },
        Row {
            case: "a standard queue holding nothing",
            name: "purge-empty",
            fifo: false,
            attributes: &[],
            visible: 0,
            in_flight: 0,
            delayed: 0,
        },
        Row {
            case: "a standard queue whose visibility timeout is long",
            name: "purge-long-timeout",
            fifo: false,
            attributes: &[("VisibilityTimeout", "600")],
            visible: 1,
            in_flight: 3,
            delayed: 0,
        },
        Row {
            case: "a FIFO queue with messages in flight",
            name: "purge-fifo.fifo",
            fifo: true,
            attributes: &[],
            visible: 2,
            in_flight: 2,
            delayed: 0,
        },
        Row {
            case: "a FIFO queue whose own DelaySeconds delays every send",
            name: "purge-fifo-delayed.fifo",
            fifo: true,
            attributes: &[("DelaySeconds", "900")],
            visible: 0,
            in_flight: 0,
            delayed: 3,
        },
    ];

    for row in rows {
        let router = emulator();
        let url = create_ok(&router, row.name, &queue_attributes(row.fifo, row.attributes)).await;
        let handles = fill(&router, row, &url).await;

        let held = message_counts(&router, &url).await;
        let expected = (
            i64::try_from(row.visible).unwrap_or_default(),
            i64::try_from(row.delayed).unwrap_or_default(),
            i64::try_from(row.in_flight).unwrap_or_default(),
        );
        assert_eq!(
            held, expected,
            "case {}: expected (visible, delayed, in flight) {expected:?} before the purge, got \
             {held:?}",
            row.case
        );
        let before = configuration(&router, &url).await;

        let reply = purge_ok(&router, &url).await;
        assert_eq!(
            reply.as_object().map(serde_json::Map::len),
            Some(0),
            "case {}: a purge answers an empty body, got {reply}",
            row.case
        );

        let after_counts = message_counts(&router, &url).await;
        assert_eq!(
            after_counts,
            (0, 0, 0),
            "case {}: every message must be gone, got (visible, delayed, in flight) \
             {after_counts:?}",
            row.case
        );
        let handed = receive(&router, &url, 10).await;
        assert!(
            handed.is_empty(),
            "case {}: a receive after the purge must hand nothing over, got {handed:?}",
            row.case
        );

        // A handle minted before the purge names a message that no longer exists, so the
        // delete it was minted for is refused rather than silently applied.
        for handle in &handles {
            let (status, answer) = call(
                &router,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (404, "ReceiptHandleIsInvalid"),
                "case {}: a pre-purge receipt handle must name nothing, got {status} with \
                 {answer}",
                row.case
            );
        }

        let after = configuration(&router, &url).await;
        assert_eq!(
            after, before,
            "case {}: the purge must leave the queue's configuration alone",
            row.case
        );
    }
}

/// A purge empties the message plane and nothing else: the queue keeps taking messages
/// straight afterwards, and its sequence counter "continues to increase" rather than
/// starting over, so the purge did not re-create the queue. The rows span the two queue
/// types.
#[tokio::test]
async fn a_queue_keeps_working_after_a_purge() {
    // (case, queue name, fifo)
    let rows: &[(&str, &str, bool)] = &[
        ("a standard queue", "reusable", false),
        ("a FIFO queue", "reusable.fifo", true),
    ];

    for (case, name, fifo) in rows {
        let router = emulator();
        let url = create_ok(&router, name, &queue_attributes(*fifo, &[])).await;

        let before = send_one(&router, &url, *fifo, "before", None).await;
        purge_ok(&router, &url).await;
        let after = send_one(&router, &url, *fifo, "after", None).await;

        assert_ne!(
            after["MessageId"].as_str(),
            before["MessageId"].as_str(),
            "case {case}: a send after the purge is a new message, got {after} after {before}"
        );
        if *fifo {
            let (first, next) = (
                before["SequenceNumber"].as_str().unwrap_or_default(),
                after["SequenceNumber"].as_str().unwrap_or_default(),
            );
            assert!(
                next > first && next.len() == first.len(),
                "case {case}: the sequence number must keep increasing across a purge, got {next} \
                 after {first}"
            );
        }

        let handed = receive(&router, &url, 10).await;
        let bodies: Vec<&str> = handed
            .iter()
            .map(|message| message["Body"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            bodies,
            vec!["after"],
            "case {case}: only the message sent after the purge is held, got {bodies:?}"
        );
    }
}

/// A deduplication token admits a message again once the message it first admitted has
/// been purged: before the purge the resend is accepted without delivering a copy, after
/// it the same token delivers one. The rows span how the token arises — supplied by the
/// send, and derived from the body under `ContentBasedDeduplication`.
#[tokio::test]
async fn a_purge_lets_a_deduplication_token_admit_a_message_again() {
    // (case, queue name, queue attributes, the token each send carries)
    let rows: &[DeduplicationRow<'_>] = &[
        (
            "a supplied deduplication token",
            "dedup-purged.fifo",
            &[],
            Some("token"),
        ),
        (
            "a token derived from the body",
            "dedup-content-purged.fifo",
            &[("ContentBasedDeduplication", "true")],
            None,
        ),
    ];

    for (case, name, attributes, token) in rows {
        let router = emulator();
        let url = create_ok(&router, name, &queue_attributes(true, attributes)).await;

        let send = |body: &'static str| {
            let mut request = json!({
                "QueueUrl": url,
                "MessageBody": body,
                "MessageGroupId": "group",
            });
            if let Some(token) = token {
                request["MessageDeduplicationId"] = json!(token);
            }
            ok(&router, "SendMessage", request)
        };

        let first = send("same body").await;
        let resend = send("same body").await;
        assert_eq!(
            resend["MessageId"].as_str(),
            first["MessageId"].as_str(),
            "case {case}: inside the interval the resend answers for the first message, got \
             {resend} after {first}"
        );
        let held = message_counts(&router, &url).await;
        assert_eq!(
            held.0, 1,
            "case {case}: the queue holds one copy before the purge, got {held:?}"
        );

        purge_ok(&router, &url).await;

        let after = send("same body").await;
        assert_ne!(
            after["MessageId"].as_str(),
            first["MessageId"].as_str(),
            "case {case}: the token admits a new message once the purge removed the old one, got \
             {after} after {first}"
        );
        let handed = receive(&router, &url, 10).await;
        assert_eq!(
            handed.len(),
            1,
            "case {case}: the resend after the purge is delivered, got {handed:?}"
        );
        assert_eq!(
            handed[0]["MessageId"].as_str(),
            after["MessageId"].as_str(),
            "case {case}: the message delivered is the one the resend minted, got {:?}",
            handed[0]
        );
    }
}

/// A batch a `ReceiveRequestAttemptId` remembers names messages the purge discarded, so
/// a retry carrying that token after a purge replays nothing: "you can't retrieve any
/// messages deleted from a queue". The retry is still answered, it simply finds the
/// queue the purge left behind.
#[tokio::test]
async fn a_retry_of_a_remembered_receive_replays_nothing_after_a_purge() {
    let router = emulator();
    let url = create_ok(&router, "attempted.fifo", &queue_attributes(true, &[])).await;
    for body in ["one", "two"] {
        send_one(&router, &url, true, body, None).await;
    }

    let attempt = json!({
        "QueueUrl": url,
        "MaxNumberOfMessages": 10,
        "ReceiveRequestAttemptId": "attempt-1",
        "WaitTimeSeconds": 0,
    });
    let first = messages(&ok(&router, "ReceiveMessage", attempt.clone()).await);
    assert_eq!(
        first.len(),
        2,
        "case the remembered receive: expected both messages, got {first:?}"
    );
    let replayed = messages(&ok(&router, "ReceiveMessage", attempt.clone()).await);
    assert_eq!(
        replayed.len(),
        2,
        "case the retry before the purge: the token replays its batch, got {replayed:?}"
    );

    purge_ok(&router, &url).await;

    let after = messages(&ok(&router, "ReceiveMessage", attempt).await);
    assert!(
        after.is_empty(),
        "case the retry after the purge: nothing the purge discarded may come back, got {after:?}"
    );
}

/// A purge arriving while the one before it is still deleting is refused with
/// `PurgeQueueInProgress`, carrying Amazon SQS's own sentence for that class — which
/// names the queue the request addressed and the interval the deletion can take — and
/// changes nothing: the messages sent since the accepted purge are still there. The rows
/// span the two queue types and whether the queue holds anything when the refused purge
/// arrives.
#[tokio::test]
async fn a_purge_arriving_while_one_is_still_deleting_is_refused_and_changes_nothing() {
    // (case, queue name, fifo, how many messages are sent between the two purges)
    let rows: &[(&str, &str, bool, usize)] = &[
        ("a standard queue left empty", "second-empty", false, 0),
        ("a standard queue refilled", "second-refilled", false, 2),
        ("a FIFO queue left empty", "second-empty.fifo", true, 0),
        ("a FIFO queue refilled", "second-refilled.fifo", true, 3),
    ];

    for (case, name, fifo, refilled) in rows {
        let router = emulator();
        let url = create_ok(&router, name, &queue_attributes(*fifo, &[])).await;
        send_one(&router, &url, *fifo, "doomed", None).await;
        purge_ok(&router, &url).await;
        for index in 0..*refilled {
            send_one(&router, &url, *fifo, &format!("kept-{index}"), None).await;
        }

        // Every further purge inside the interval is refused, not only the one that
        // follows immediately.
        for attempt in 1..=2 {
            let (status, answer) = call(&router, "PurgeQueue", &json!({ "QueueUrl": url })).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (403, "PurgeQueueInProgress"),
                "case {case}, attempt {attempt}: expected a 403 PurgeQueueInProgress, got \
                 {status} with {answer}"
            );
            let message = answer["message"]
                .as_str()
                .or_else(|| answer["Message"].as_str())
                .unwrap_or_default();
            // The sentence is written out here rather than read from the emulator, so
            // the wording this suite pins does not move when the implementation does.
            // Amazon SQS words this error class: "Only one PurgeQueue operation on
            // <queue> is allowed every 60 seconds.", the 60 seconds being the interval
            // `PurgeQueueInProgress` documents for the deletion process.
            assert_eq!(
                message,
                format!("Only one PurgeQueue operation on {name} is allowed every 60 seconds."),
                "case {case}, attempt {attempt}: expected the sentence for this error class, got \
                 {message:?}"
            );
        }

        // The messages the refused calls did not touch are still held.
        let held = message_counts(&router, &url).await;
        assert_eq!(
            held.0,
            i64::try_from(*refilled).unwrap_or_default(),
            "case {case}: the refused purge must leave the queue as it was, got {held:?}"
        );
    }
}

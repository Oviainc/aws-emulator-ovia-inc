// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for FIFO receive-side idempotency and the FIFO attribute pair,
//! driven over the wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`)
//! through the shared harness in [`super::support`].
//!
//! The rules pinned here, each over a TABLE of inputs rather than one example:
//!
//! 1. A `ReceiveMessage` on a FIFO queue carrying a `ReceiveRequestAttemptId` is
//!    idempotent for as long as that token lasts: a retry under the same token hands the
//!    identical messages and receipt handles back, counts as no further delivery and
//!    resets the visibility timeout; past the token's window the receive is a fresh one.
//! 2. The retry is refused once the batch it named has been modified.
//! 3. The token is bounded the way its own documentation bounds it, on either queue
//!    type, and a standard queue — which keeps no receive attempts — serves a receive
//!    carrying one as if it carried none.
//! 4. A deduplication token suppresses a resend only inside its interval, and is
//!    reusable once the interval has elapsed.
//! 5. The per-message-group FIFO throughput limit holds only together with the
//!    message-group deduplication scope, on either call that can leave a queue carrying
//!    the pair.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    self, DEDUPLICATION_INTERVAL, MAX_MESSAGE_TOKEN_LENGTH, RECEIVE_REQUEST_ATTEMPT_INTERVAL,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    all_attributes, as_map, attribute_count, call, create, create_ok, emulator,
    emulator_over_engine, error_code, fifo_attributes as fifo, messages, ok,
};

/// What one receive handed over, as the tuple a retry must reproduce exactly: the
/// message id, the receipt handle, the body and the system attributes reported for it.
type HandedOver = (String, String, String, BTreeMap<String, String>);

/// One row of the batch-shape table: the case, the sends as `(group, body)`, and the
/// `MaxNumberOfMessages` the first receive and its retry each ask for.
type BatchRow<'a> = (&'a str, &'a [(&'a str, &'a str)], u32, u32);

/// One row of the create table: the case, the attributes beyond `FifoQueue`, and whether
/// the queue is created.
type AttributePairRow<'a> = (&'a str, &'a [(&'a str, &'a str)], bool);

/// One row of the `SetQueueAttributes` table: the case, the queue, the attributes to set
/// and whether the call is accepted.
type SetAttributeRow<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], bool);

/// Send one message to a FIFO queue and return the `MessageId` it was accepted under.
async fn send_one(
    router: &MultiServiceRouter,
    url: &str,
    group: &str,
    body: &str,
    token: &str,
) -> String {
    let answer = ok(
        router,
        "SendMessage",
        json!({
            "QueueUrl": url,
            "MessageBody": body,
            "MessageGroupId": group,
            "MessageDeduplicationId": token,
        }),
    )
    .await;
    answer["MessageId"]
        .as_str()
        .unwrap_or_else(|| panic!("case send {body}: no MessageId in {answer}"))
        .to_string()
}

/// Receive from `url`, optionally under `attempt_id` and with an explicit
/// `VisibilityTimeout`. Every system attribute is selected, so what a retry reports can
/// be compared against what the first receive reported.
async fn receive(
    router: &MultiServiceRouter,
    url: &str,
    attempt_id: Option<&str>,
    max: u32,
    visibility: Option<i64>,
) -> (u16, Value) {
    let mut body = json!({
        "QueueUrl": url,
        "MaxNumberOfMessages": max,
        "MessageSystemAttributeNames": ["All"],
    });
    if let Some(attempt_id) = attempt_id {
        body["ReceiveRequestAttemptId"] = json!(attempt_id);
    }
    if let Some(visibility) = visibility {
        body["VisibilityTimeout"] = json!(visibility);
    }
    call(router, "ReceiveMessage", &body).await
}

/// The same receive, which must succeed, as the list of messages it handed over.
async fn receive_ok(
    router: &MultiServiceRouter,
    url: &str,
    attempt_id: Option<&str>,
    max: u32,
    visibility: Option<i64>,
) -> Vec<Value> {
    let (status, answer) = receive(router, url, attempt_id, max, visibility).await;
    assert_eq!(
        status, 200,
        "case receive from {url} under {attempt_id:?}: expected HTTP 200, got {status} with \
         {answer}"
    );
    messages(&answer)
}

/// What `received` handed over, in the order it was handed over.
fn handed_over(received: &[Value]) -> Vec<HandedOver> {
    received
        .iter()
        .map(|message| {
            (
                message["MessageId"].as_str().unwrap_or_default().to_string(),
                message["ReceiptHandle"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                message["Body"].as_str().unwrap_or_default().to_string(),
                message.get("Attributes").map(as_map).unwrap_or_default(),
            )
        })
        .collect()
}

/// Delete the message `received` names, which is what "modifies" the batch it belongs to.
async fn delete(router: &MultiServiceRouter, url: &str, received: &Value) {
    let handle = received["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case delete: no ReceiptHandle in {received}"));
    ok(
        router,
        "DeleteMessage",
        json!({ "QueueUrl": url, "ReceiptHandle": handle }),
    )
    .await;
}

/// Move every receive attempt the queue called `name` remembers `by` further into the
/// past.
///
/// The emulator measures the retry window off the instant the receive was served, so
/// rewinding that instant is exactly what the clock reaching the far side of the window
/// does — and the only way a test can stand there without waiting out a span AWS
/// measures in minutes.
fn rewind_receive_attempts(engine: &Arc<engine::InMemoryEngine>, name: &str, by: Duration) {
    let store = state::service_state(&**engine).expect("this service's store");
    let mut store = store.lock();
    let queue = store
        .queues
        .get_mut(name)
        .unwrap_or_else(|| panic!("case rewind: no queue called {name}"));
    for attempt in queue.receive_attempts.values_mut() {
        attempt.served_at = attempt
            .served_at
            .checked_sub(by)
            .expect("an instant inside SystemTime");
    }
}

/// The same, for the deduplication tokens the queue called `name` remembers.
fn rewind_deduplication(engine: &Arc<engine::InMemoryEngine>, name: &str, by: Duration) {
    let store = state::service_state(&**engine).expect("this service's store");
    let mut store = store.lock();
    let queue = store
        .queues
        .get_mut(name)
        .unwrap_or_else(|| panic!("case rewind: no queue called {name}"));
    for remembered in queue.deduplication.values_mut() {
        remembered.accepted_at = remembered
            .accepted_at
            .checked_sub(by)
            .expect("an instant inside SystemTime");
    }
}

/// "It is possible to retry the same action with an identical ReceiveRequestAttemptId to
/// retrieve the same set of messages, even if their visibility timeout has not yet
/// expired ... subsequent calls with the same ReceiveRequestAttemptId return the same
/// messages and receipt handles."
///
/// The rows span the shapes a batch can have — how many messages, how many message
/// groups, and a retry asking for a different number of messages than the receive it
/// retries.
#[tokio::test]
async fn a_retry_under_the_same_attempt_id_hands_the_identical_batch_back() {
    let ten_groups: Vec<(String, String)> = (0..10)
        .map(|index| (format!("group-{index}"), format!("body-{index}")))
        .collect();
    let ten_groups: Vec<(&str, &str)> = ten_groups
        .iter()
        .map(|(group, body)| (group.as_str(), body.as_str()))
        .collect();

    // (case, sends as (group, body), MaxNumberOfMessages asked for, the same on the retry)
    let rows: &[BatchRow<'_>] = &[
        ("one message in one group", &[("alpha", "a1")], 1, 1),
        (
            "one group of three, of which a FIFO receive hands over the head",
            &[("alpha", "a1"), ("alpha", "a2"), ("alpha", "a3")],
            10,
            10,
        ),
        (
            "three groups of one",
            &[("alpha", "a1"), ("beta", "b1"), ("gamma", "c1")],
            10,
            10,
        ),
        (
            "a retry asking for fewer messages than the batch holds",
            &[("alpha", "a1"), ("beta", "b1"), ("gamma", "c1")],
            3,
            1,
        ),
        (
            "a retry asking for more messages than the batch holds",
            &[("alpha", "a1"), ("beta", "b1")],
            2,
            10,
        ),
        (
            "ten groups, the most messages one receive may hand over",
            &ten_groups,
            10,
            10,
        ),
    ];

    for (index, (case, sends, max, retry_max)) in rows.iter().enumerate() {
        let router = emulator();
        let url = create_ok(&router, &format!("replay-{index}.fifo"), &fifo(&[])).await;
        for (position, (group, body)) in sends.iter().enumerate() {
            send_one(&router, &url, group, body, &format!("token-{position}")).await;
        }

        let first = handed_over(&receive_ok(&router, &url, Some("r1"), *max, None).await);
        assert!(
            !first.is_empty(),
            "case {case}: the first receive must hand something over"
        );

        let replay = handed_over(&receive_ok(&router, &url, Some("r1"), *retry_max, None).await);
        assert_eq!(
            replay, first,
            "case {case}: the retry must hand the identical batch back, got {replay:?} after \
             {first:?}"
        );

        // The batch is in flight, so a receive that is NOT that retry gets nothing: the
        // replay is the token's doing, not a second delivery.
        let other = receive_ok(&router, &url, Some("r2"), 10, None).await;
        assert!(
            other.is_empty(),
            "case {case}: a receive under another attempt id must hand nothing over while the \
             batch is in flight, got {other:?}"
        );
    }
}

/// "If a retry occurs within the deduplication interval, it resets the visibility
/// timeout" — so a batch whose timeout has already lapsed is hidden again by the retry,
/// for the retry's own visibility timeout.
#[tokio::test]
async fn a_retry_resets_the_visibility_timeout_of_the_batch() {
    let router = emulator();
    let name = "visibility-reset.fifo";
    let url = create_ok(&router, name, &fifo(&[])).await;
    let message_id = send_one(&router, &url, "alpha", "a1", "token").await;

    // One second is the shortest visibility timeout a test can let lapse; the attribute
    // accepts "an integer from 0 to 43,200".
    let first = handed_over(&receive_ok(&router, &url, Some("r1"), 1, Some(1)).await);
    assert_eq!(
        first.len(),
        1,
        "case first receive: expected the one message, got {first:?}"
    );
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert_eq!(
        attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
        1,
        "case after the timeout lapsed: the message must be visible again"
    );

    let replay = handed_over(&receive_ok(&router, &url, Some("r1"), 1, Some(30)).await);
    assert_eq!(
        replay, first,
        "case retry after the timeout lapsed: expected the same batch, got {replay:?} after \
         {first:?}"
    );
    assert_eq!(
        replay[0].0, message_id,
        "case retry: expected the message that was sent, got {}",
        replay[0].0
    );
    assert_eq!(
        attribute_count(&router, &url, "ApproximateNumberOfMessagesNotVisible").await,
        1,
        "case after the retry: the retry's visibility timeout must hide the message again"
    );
    let plain = receive_ok(&router, &url, None, 10, None).await;
    assert!(
        plain.is_empty(),
        "case after the retry: the message is in flight again, so a plain receive gets nothing, \
         got {plain:?}"
    );
}

/// "It is possible to retry the ReceiveMessage action with the same
/// ReceiveRequestAttemptId if none of the messages have been modified (deleted or had
/// their visibility changes)": the rows delete a message of the batch at each position,
/// delete all of them, and — the control — delete a message the batch does not name.
#[tokio::test]
async fn a_retry_is_refused_once_the_batch_has_been_modified() {
    // (case, which of the batch's messages are deleted, delete one outside it, retry served)
    let rows: &[(&str, &[usize], bool, bool)] = &[
        ("the first message of the batch deleted", &[0], false, false),
        ("the last message of the batch deleted", &[1], false, false),
        ("every message of the batch deleted", &[0, 1], false, false),
        ("nothing deleted at all", &[], false, true),
        ("only a message outside the batch deleted", &[], true, true),
    ];

    for (index, (case, deleted, delete_outside, served)) in rows.iter().enumerate() {
        let router = emulator();
        let url = create_ok(&router, &format!("modified-{index}.fifo"), &fifo(&[])).await;
        for (group, body) in [("alpha", "a1"), ("beta", "b1"), ("gamma", "c1")] {
            send_one(&router, &url, group, body, body).await;
        }

        // The batch this attempt id names: the heads of the first two groups.
        let batch = receive_ok(&router, &url, Some("r1"), 2, None).await;
        assert_eq!(
            batch.len(),
            2,
            "case {case}: expected a batch of two, got {batch:?}"
        );
        let expected = handed_over(&batch);

        // A message the batch does not name, taken under its own attempt id.
        let outside = receive_ok(&router, &url, Some("r-outside"), 1, None).await;
        assert_eq!(
            outside.len(),
            1,
            "case {case}: expected the third group's head, got {outside:?}"
        );
        if *delete_outside {
            delete(&router, &url, &outside[0]).await;
        }
        for position in *deleted {
            delete(&router, &url, &batch[*position]).await;
        }

        let (status, answer) = receive(&router, &url, Some("r1"), 2, None).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case}: expected the retry to be served, got {status} with {answer}"
            );
            assert_eq!(
                handed_over(&messages(&answer)),
                expected,
                "case {case}: expected the identical batch, got {answer}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "InvalidParameterValue"),
                "case {case}: expected a 400 InvalidParameterValue, got {status} with {answer}"
            );
            assert!(
                answer["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("ReceiveRequestAttemptId")
                        && message.contains("r1")),
                "case {case}: the refusal must name the attempt id it refuses, got {answer}"
            );
        }
    }
}

/// "You can use ReceiveRequestAttemptId only for 5 minutes after a ReceiveMessage
/// action": inside the window the token replays its batch, past it the receive carrying
/// the token is a fresh one — which is a further delivery of the message, not a replay.
#[tokio::test]
async fn an_attempt_id_replays_only_inside_its_window() {
    let (router, engine) = emulator_over_engine();
    let name = "window.fifo";
    let url = create_ok(&router, name, &fifo(&[])).await;
    let message_id = send_one(&router, &url, "alpha", "a1", "token").await;

    // A zero visibility timeout leaves the message visible, so what the token replays is
    // not merely what nothing else could take.
    let first = handed_over(&receive_ok(&router, &url, Some("r1"), 1, Some(0)).await);
    assert_eq!(
        first.len(),
        1,
        "case first receive: expected the one message, got {first:?}"
    );
    let replay = handed_over(&receive_ok(&router, &url, Some("r1"), 1, Some(0)).await);
    assert_eq!(
        replay, first,
        "case inside the window: expected the same batch and handle, got {replay:?} after \
         {first:?}"
    );
    assert_eq!(
        replay[0].3.get("ApproximateReceiveCount").map(String::as_str),
        Some("1"),
        "case inside the window: a retry is the same receive, so it counts as no further \
         delivery, got {:?}",
        replay[0].3
    );

    rewind_receive_attempts(
        &engine,
        name,
        RECEIVE_REQUEST_ATTEMPT_INTERVAL + Duration::from_secs(1),
    );

    let fresh = handed_over(&receive_ok(&router, &url, Some("r1"), 1, Some(0)).await);
    assert_eq!(
        fresh.len(),
        1,
        "case past the window: expected the message to be handed over afresh, got {fresh:?}"
    );
    assert_eq!(
        fresh[0].0, message_id,
        "case past the window: expected the same message, got {}",
        fresh[0].0
    );
    assert_ne!(
        fresh[0].1, first[0].1,
        "case past the window: a fresh receive mints a new receipt handle, got {}",
        fresh[0].1
    );
    assert_eq!(
        fresh[0].3.get("ApproximateReceiveCount").map(String::as_str),
        Some("2"),
        "case past the window: a fresh receive is a further delivery, got {:?}",
        fresh[0].3
    );
}

/// The token's own form rule — "the maximum length of ReceiveRequestAttemptId is 128
/// characters. ReceiveRequestAttemptId can contain alphanumeric characters (a-z, A-Z,
/// 0-9) and punctuation (!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~)" — which the member
/// publishes whatever queue the receive addresses, and which is all a STANDARD queue
/// does with the token: "this parameter applies only to FIFO (first-in-first-out)
/// queues", so a standard queue has no batch to replay under it.
#[tokio::test]
async fn an_attempt_id_is_bounded_in_its_form_on_either_queue_type() {
    let router = emulator();
    let fifo_url = create_ok(&router, "attempt-form.fifo", &fifo(&[])).await;
    let standard_url = create_ok(&router, "attempt-form", &json!({})).await;

    // (case, token, accepted)
    let rows: &[(&str, String, bool)] = &[
        ("one character", "r".to_string(), true),
        (
            "the published bound, 128 characters",
            "r".repeat(MAX_MESSAGE_TOKEN_LENGTH),
            true,
        ),
        (
            "one character past the bound",
            "r".repeat(MAX_MESSAGE_TOKEN_LENGTH + 1),
            false,
        ),
        ("empty", String::new(), false),
        (
            "every punctuation character the set lists",
            "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~".to_string(),
            true,
        ),
        ("digits and mixed case", "aZ09".to_string(), true),
        ("a space, which is neither", "a b".to_string(), false),
        ("a tab, which is neither", "a\tb".to_string(), false),
        (
            "a non-ASCII letter, which is neither",
            "grüße".to_string(),
            false,
        ),
    ];

    for (case, token, accepted) in rows {
        let (status, answer) = receive(&router, &fifo_url, Some(token), 1, None).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case FIFO {case}: expected HTTP 200, got {status} with {answer}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "InvalidParameterValue"),
                "case FIFO {case}: expected a 400 InvalidParameterValue, got {status} with \
                 {answer}"
            );
            assert!(
                answer["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("ReceiveRequestAttemptId")),
                "case FIFO {case}: the refusal must name the member, got {answer}"
            );
        }

        // A standard queue keeps no receive attempts — nothing there is ordered or
        // deduplicated — so the token names no batch to replay and the receive is served
        // as if it carried none. Its form is still the form the member publishes.
        let (status, answer) = receive(&router, &standard_url, Some(token), 1, None).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case standard {case}: expected HTTP 200, got {status} with {answer}"
            );
            assert!(
                messages(&answer).is_empty(),
                "case standard {case}: the empty queue hands nothing over, got {answer}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "InvalidParameterValue"),
                "case standard {case}: expected a 400 InvalidParameterValue, got {status} with \
                 {answer}"
            );
            assert!(
                answer["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("ReceiveRequestAttemptId")),
                "case standard {case}: the refusal must name the member, got {answer}"
            );
        }
    }
}

/// A `MessageDeduplicationId` suppresses a resend only "during the 5-minute
/// deduplication interval"; once it has elapsed, "Amazon SQS can't detect duplicate
/// messages" and the same token admits a new message. The rows walk the interval: inside
/// it, exactly at its published bound, and well past it.
#[tokio::test]
async fn a_deduplication_token_is_reusable_once_its_interval_has_elapsed() {
    // (case, how far the ledger has been left behind, the resend is a new message)
    let rows: &[(&str, Duration, bool)] = &[
        ("no time has passed", Duration::ZERO, false),
        (
            "just inside the interval",
            DEDUPLICATION_INTERVAL - Duration::from_secs(5),
            false,
        ),
        (
            "the published bound: the interval has just elapsed",
            DEDUPLICATION_INTERVAL,
            true,
        ),
        ("well past the interval", DEDUPLICATION_INTERVAL * 3, true),
    ];

    for (index, (case, elapsed, reusable)) in rows.iter().enumerate() {
        let (router, engine) = emulator_over_engine();
        let name = format!("dedup-window-{index}.fifo");
        let url = create_ok(&router, &name, &fifo(&[])).await;
        let first = send_one(&router, &url, "alpha", "first", "token").await;

        rewind_deduplication(&engine, &name, *elapsed);

        let answer = ok(
            &router,
            "SendMessage",
            json!({
                "QueueUrl": url,
                "MessageBody": "second",
                "MessageGroupId": "alpha",
                "MessageDeduplicationId": "token",
            }),
        )
        .await;
        let resent = answer["MessageId"].as_str().unwrap_or_default().to_string();
        let held = attribute_count(&router, &url, "ApproximateNumberOfMessages").await;

        if *reusable {
            assert_ne!(
                resent, first,
                "case {case}: past the interval the token must admit a new message, got {resent}"
            );
            assert_eq!(
                held, 2,
                "case {case}: expected both messages held, got {held}"
            );
        } else {
            assert_eq!(
                resent, first,
                "case {case}: inside the interval the resend must answer for the message the \
                 token admitted, got {resent} against {first}"
            );
            assert_eq!(
                held, 1,
                "case {case}: expected one copy delivered, got {held}"
            );
        }
    }
}

/// "The perMessageGroupId value is allowed only when the value for DeduplicationScope is
/// messageGroup": the pair is judged over the attribute set the call would leave the
/// queue carrying, so the rows walk both values of each attribute, each attribute alone,
/// and neither.
#[tokio::test]
async fn the_per_message_group_throughput_limit_requires_the_message_group_scope() {
    // (case, attributes beyond FifoQueue, created)
    let rows: &[AttributePairRow<'_>] = &[
        ("neither attribute", &[], true),
        (
            "perMessageGroupId with the messageGroup scope",
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "messageGroup"),
            ],
            true,
        ),
        (
            "perMessageGroupId with the queue scope",
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "queue"),
            ],
            false,
        ),
        (
            "perMessageGroupId with no scope at all",
            &[("FifoThroughputLimit", "perMessageGroupId")],
            false,
        ),
        (
            "perQueue with the queue scope",
            &[
                ("FifoThroughputLimit", "perQueue"),
                ("DeduplicationScope", "queue"),
            ],
            true,
        ),
        (
            "perQueue with the messageGroup scope",
            &[
                ("FifoThroughputLimit", "perQueue"),
                ("DeduplicationScope", "messageGroup"),
            ],
            true,
        ),
        (
            "the messageGroup scope alone",
            &[("DeduplicationScope", "messageGroup")],
            true,
        ),
        (
            "the throughput limit left at perQueue alone",
            &[("FifoThroughputLimit", "perQueue")],
            true,
        ),
    ];

    let router = emulator();
    for (index, (case, attributes, created)) in rows.iter().enumerate() {
        let name = format!("throughput-{index}.fifo");
        let (status, answer) = create(&router, &name, &fifo(attributes)).await;
        if *created {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {answer}"
            );
            let reported = all_attributes(&router, answer["QueueUrl"].as_str().unwrap_or_default())
                .await;
            for (attribute, value) in *attributes {
                assert_eq!(
                    reported.get(*attribute).map(String::as_str),
                    Some(*value),
                    "case {case}: the created queue must report {attribute}={value}, got \
                     {reported:?}"
                );
            }
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "InvalidAttributeValue"),
                "case {case}: expected a 400 InvalidAttributeValue, got {status} with {answer}"
            );
            // Refused at 400 means no queue was created, so the name resolves to nothing.
            let (status, answer) = call(&router, "GetQueueUrl", &json!({ "QueueName": name })).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "QueueDoesNotExist"),
                "case {case}: the refused create must have created no queue, got {status} with \
                 {answer}"
            );
        }
    }
}

/// The same rule on `SetQueueAttributes`, which can leave a queue carrying the pair just
/// as a create can: either half may come from the request or from the queue it is
/// applied to.
#[tokio::test]
async fn set_queue_attributes_holds_the_same_throughput_pair_rule() {
    let router = emulator();
    let high = create_ok(
        &router,
        "throughput-set-high.fifo",
        &fifo(&[
            ("FifoThroughputLimit", "perMessageGroupId"),
            ("DeduplicationScope", "messageGroup"),
        ]),
    )
    .await;
    let plain = create_ok(&router, "throughput-set-plain.fifo", &fifo(&[])).await;

    // (case, queue, attributes to set, accepted)
    let rows: &[SetAttributeRow<'_>] = &[
        (
            "moving a high-throughput queue to the queue scope",
            &high,
            &[("DeduplicationScope", "queue")],
            false,
        ),
        (
            "moving a high-throughput queue to the per-queue limit",
            &high,
            &[("FifoThroughputLimit", "perQueue")],
            true,
        ),
        (
            "the per-message-group limit alone on a queue with no scope",
            &plain,
            &[("FifoThroughputLimit", "perMessageGroupId")],
            false,
        ),
        (
            "both halves of the pair in one call",
            &plain,
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "messageGroup"),
            ],
            true,
        ),
    ];

    for (case, url, attributes, accepted) in rows {
        let mut supplied = json!({});
        for (attribute, value) in *attributes {
            supplied[*attribute] = json!(value);
        }
        let (status, answer) = call(
            &router,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": supplied }),
        )
        .await;
        let reported = all_attributes(&router, url).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {answer}"
            );
            for (attribute, value) in *attributes {
                assert_eq!(
                    reported.get(*attribute).map(String::as_str),
                    Some(*value),
                    "case {case}: expected {attribute}={value} reported, got {reported:?}"
                );
            }
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "InvalidAttributeValue"),
                "case {case}: expected a 400 InvalidAttributeValue, got {status} with {answer}"
            );
            for (attribute, value) in *attributes {
                assert_ne!(
                    reported.get(*attribute).map(String::as_str),
                    Some(*value),
                    "case {case}: a refused call must change nothing, got {reported:?}"
                );
            }
        }
    }
}

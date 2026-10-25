// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SQS MESSAGE plane: `SendMessage`, `ReceiveMessage`,
//! `DeleteMessage`, `DeleteQueue` and `ListQueues`, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the same `MultiServiceRouter` the
//! emulator's front doors use.
//!
//! The queue-configuration plane (`CreateQueue`, `GetQueueUrl`, `GetQueueAttributes`,
//! `SetQueueAttributes`) is covered by `queue_configuration.rs`; this suite uses those
//! operations only to set a queue up and to read the message counters back, so every
//! assertion here is about state a message operation wrote.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support;
use super::support::HOST;

/// The emulator, mounted exactly as the router mounts it in production.
///
/// A thin wrapper over the harness every suite in this crate shares
/// ([`super::support`]): the request shape, the host, the credential scope and the
/// url and ARN forms are written once there, and this type only adds the
/// message-plane shorthands the tests below read better with.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        Self {
            router: support::emulator(),
        }
    }

    /// Call `operation` with `body` and return the HTTP status and the JSON answer.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        support::call(&self.router, operation, body).await
    }

    /// Create a queue with `attributes` and return its url, failing on anything else.
    async fn create_queue_with(&self, name: &str, attributes: &Value) -> String {
        support::create_ok(&self.router, name, attributes).await
    }

    /// Create a standard queue with the documented defaults.
    async fn create_queue(&self, name: &str) -> String {
        self.create_queue_with(name, &json!({})).await
    }

    /// One derived counter, read back through `GetQueueAttributes`.
    async fn count(&self, url: &str, attribute: &str) -> String {
        let (status, answer) = self
            .call(
                "GetQueueAttributes",
                &json!({ "QueueUrl": url, "AttributeNames": [attribute] }),
            )
            .await;
        assert_eq!(status, 200, "case {attribute}: got {answer:?}");
        answer["Attributes"][attribute]
            .as_str()
            .unwrap_or_else(|| panic!("case {attribute}: not reported in {answer:?}"))
            .to_string()
    }
}

/// The wire error code of a failure answer (`__type`, without its namespace) — the
/// shared harness's reader, so one suite cannot drift from another on what a refusal
/// is called.
fn error_code(answer: &Value) -> String {
    support::error_code(answer)
}

/// Known MD5 digests: the RFC 1321 (appendix A.5) test suite, plus the `hello world`
/// case. The rule under test is that `MD5OfMessageBody`/`MD5OfBody` is the real MD5 hex
/// digest of the body's bytes, so the rows differ in length, alphabet and content.
const MD5_VECTORS: &[(&str, &str)] = &[
    ("a", "0cc175b9c0f1b6a831c399e269772661"),
    ("abc", "900150983cd24fb0d6963f7d28e17f72"),
    ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
    (
        "abcdefghijklmnopqrstuvwxyz",
        "c3fcd3d76192e4007dfb496cca67e13b",
    ),
    (
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
        "d174ab98d277d9f5a5611c2c9f419d9f",
    ),
    (
        "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
        "57edf4a22be3c955ac49da2e2107b67a",
    ),
    ("hello world", "5eb63bbbe01eeed093cb22bb8f5acdc3"),
];

/// `MD5OfMessageBody` is the real digest of the body, and the body comes back from
/// state byte for byte with the same digest.
#[tokio::test]
async fn md5_of_the_body_is_the_real_digest_for_every_body() {
    let sqs = Emulator::new();
    let url = sqs.create_queue("digest-queue").await;

    for (body, expected) in MD5_VECTORS {
        let (status, sent) = sqs
            .call(
                "SendMessage",
                &json!({ "QueueUrl": url, "MessageBody": body }),
            )
            .await;
        assert_eq!(status, 200, "case {body:?}: got {sent:?}");
        assert_eq!(
            sent["MD5OfMessageBody"].as_str(),
            Some(*expected),
            "case {body:?}: expected MD5OfMessageBody {expected}, got {:?}",
            sent["MD5OfMessageBody"]
        );
        assert!(
            sent["MessageId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {body:?}: expected a MessageId, got {sent:?}"
        );

        let (status, received) = sqs
            .call(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": 1, "VisibilityTimeout": 0 }),
            )
            .await;
        assert_eq!(status, 200, "case {body:?}: got {received:?}");
        let message = &received["Messages"][0];
        assert_eq!(
            message["Body"].as_str(),
            Some(*body),
            "case {body:?}: expected the body back, got {:?}",
            message["Body"]
        );
        assert_eq!(
            message["MD5OfBody"].as_str(),
            Some(*expected),
            "case {body:?}: expected MD5OfBody {expected}, got {:?}",
            message["MD5OfBody"]
        );
        assert_eq!(
            message["MessageId"].as_str(),
            sent["MessageId"].as_str(),
            "case {body:?}: the received MessageId must be the one SendMessage minted, got {:?}",
            message["MessageId"]
        );

        let handle = message["ReceiptHandle"]
            .as_str()
            .unwrap_or_else(|| panic!("case {body:?}: no ReceiptHandle in {received:?}"));
        let (status, deleted) = sqs
            .call(
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
        assert_eq!(status, 200, "case {body:?}: DeleteMessage got {deleted:?}");
    }
}

/// The whole message lifecycle, in order: send, count, receive, re-receive, delete,
/// delete the queue. Each step reads back through an implemented operation, so nothing
/// is asserted that is not in state.
#[tokio::test]
async fn the_message_lifecycle_round_trips_through_state() {
    let sqs = Emulator::new();
    let url = sqs.create_queue("demo-queue").await;

    // A fresh queue holds nothing.
    for (counter, expected) in [
        ("ApproximateNumberOfMessages", "0"),
        ("ApproximateNumberOfMessagesNotVisible", "0"),
        ("ApproximateNumberOfMessagesDelayed", "0"),
    ] {
        assert_eq!(
            sqs.count(&url, counter).await,
            expected,
            "case fresh queue: {counter}"
        );
    }

    // Send one message; the queue now reports one visible message.
    let (status, sent) = sqs
        .call(
            "SendMessage",
            &json!({ "QueueUrl": url, "MessageBody": "hello world" }),
        )
        .await;
    assert_eq!(status, 200, "case SendMessage: got {sent:?}");
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        "1",
        "case after send: expected one visible message"
    );

    // Receive it: it becomes invisible, so an immediate second receive is empty.
    let (status, received) = sqs
        .call("ReceiveMessage", &json!({ "QueueUrl": url }))
        .await;
    assert_eq!(status, 200, "case ReceiveMessage: got {received:?}");
    assert_eq!(
        received["Messages"].as_array().map(Vec::len),
        Some(1),
        "case ReceiveMessage: expected one message, got {received:?}"
    );
    let handle = received["Messages"][0]["ReceiptHandle"]
        .as_str()
        .expect("a receipt handle")
        .to_string();

    let (status, again) = sqs
        .call("ReceiveMessage", &json!({ "QueueUrl": url }))
        .await;
    assert_eq!(status, 200, "case second ReceiveMessage: got {again:?}");
    assert!(
        again.get("Messages").is_none(),
        "case second ReceiveMessage: the in-flight copy must not be returned, got {again:?}"
    );
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        "0",
        "case in flight: nothing is visible"
    );
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessagesNotVisible")
            .await,
        "1",
        "case in flight: one message is in flight"
    );

    // Delete it with its handle; it is gone for good.
    let (status, deleted) = sqs
        .call(
            "DeleteMessage",
            &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
        )
        .await;
    assert_eq!(status, 200, "case DeleteMessage: got {deleted:?}");
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessagesNotVisible")
            .await,
        "0",
        "case after delete: nothing is in flight"
    );
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        "0",
        "case after delete: nothing is visible"
    );

    // Delete the queue; it stops existing for every operation and stops being listed.
    let (status, answer) = sqs.call("DeleteQueue", &json!({ "QueueUrl": url })).await;
    assert_eq!(status, 200, "case DeleteQueue: got {answer:?}");
    let (status, answer) = sqs
        .call(
            "GetQueueAttributes",
            &json!({ "QueueUrl": url, "AttributeNames": ["All"] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "QueueDoesNotExist"),
        "case GetQueueAttributes after DeleteQueue: got {answer:?}"
    );
    let (status, answer) = sqs.call("ListQueues", &json!({})).await;
    assert_eq!(
        status, 200,
        "case ListQueues after DeleteQueue: got {answer:?}"
    );
    assert!(
        answer.get("QueueUrls").is_none(),
        "case ListQueues after DeleteQueue: expected no urls, got {answer:?}"
    );
}

/// Every operation that names a queue by url answers `QueueDoesNotExist` (400) for a
/// url no queue in state has, whatever shape that url takes.
#[tokio::test]
async fn an_unknown_queue_url_is_queue_does_not_exist_everywhere() {
    let sqs = Emulator::new();
    // A real queue exists, so the failure is about the url and not about empty state.
    let real = sqs.create_queue("real-queue").await;

    let unknown = [
        format!("https://{HOST}/000000000000/never-created"),
        format!("{real}-but-longer"),
        format!("https://{HOST}/000000000000/"),
        "not-even-a-url".to_string(),
        // The right queue name under the wrong account is not this emulator's queue.
        format!("https://{HOST}/999999999999/real-queue"),
        "https://unrelated.example.com/anything/real-queue".to_string(),
    ];
    let calls: [(&str, Value); 4] = [
        ("SendMessage", json!({ "MessageBody": "x" })),
        ("ReceiveMessage", json!({})),
        (
            "DeleteMessage",
            json!({ "ReceiptHandle": "00000000-0000-4000-8000-000000000000" }),
        ),
        ("DeleteQueue", json!({})),
    ];

    for url in &unknown {
        for (operation, extra) in &calls {
            let mut body = extra.clone();
            body["QueueUrl"] = json!(url);
            let (status, answer) = sqs.call(operation, &body).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "QueueDoesNotExist"),
                "case {operation} on {url:?}: got {status} {answer:?}"
            );
        }
    }

    // The same queue reached through another endpoint host, but under this emulator's
    // account, still resolves — that is the leniency the account check scopes.
    let (status, answer) = sqs
        .call(
            "SendMessage",
            &json!({
                "QueueUrl": "https://sqs.eu-west-1.amazonaws.com/000000000000/real-queue",
                "MessageBody": "reachable",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case another endpoint host under this account: got {status} {answer:?}"
    );
}

/// A receipt handle the queue never minted is `ReceiptHandleIsInvalid` (404) — and so is
/// one whose message has already been deleted.
#[tokio::test]
async fn a_handle_the_queue_never_minted_is_receipt_handle_is_invalid() {
    let sqs = Emulator::new();
    let url = sqs.create_queue("handle-queue").await;
    sqs.call(
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": "payload" }),
    )
    .await;
    let (_, received) = sqs
        .call("ReceiveMessage", &json!({ "QueueUrl": url }))
        .await;
    let real_handle = received["Messages"][0]["ReceiptHandle"]
        .as_str()
        .expect("a receipt handle")
        .to_string();

    // The genuine handle works exactly once.
    let (status, answer) = sqs
        .call(
            "DeleteMessage",
            &json!({ "QueueUrl": url, "ReceiptHandle": real_handle }),
        )
        .await;
    assert_eq!(status, 200, "case the minted handle: got {answer:?}");

    for handle in [
        real_handle.as_str(),
        "garbage",
        "",
        "00000000-0000-4000-8000-000000000000",
        "AQEBzbVv…+not+base64/at+all==",
    ] {
        let (status, answer) = sqs
            .call(
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "ReceiptHandleIsInvalid"),
            "case handle {handle:?}: got {status} {answer:?}"
        );
    }
}

/// Re-receiving a message replaces its receipt handle rather than adding another: only
/// the most recently received handle deletes the message, and a loop of receives leaves
/// exactly one live handle however many times it runs — so the handle map cannot grow
/// with the request count.
///
/// The rule is Amazon SQS's own: "If you receive a message more than once, each time you
/// receive it, you get a different receipt handle. You must provide the most recently
/// received receipt handle when you request to delete the message."
#[tokio::test]
async fn only_the_most_recently_received_handle_deletes_a_message() {
    let sqs = Emulator::new();
    let url = sqs.create_queue("handle-churn-queue").await;
    sqs.call(
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": "payload" }),
    )
    .await;

    let mut previous: Option<String> = None;
    // Many receives of the SAME still-visible message, the shape that would accumulate
    // one handle per request if a receive added rather than replaced.
    for round in 0..50 {
        let (status, received) = sqs
            .call(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "VisibilityTimeout": 0 }),
            )
            .await;
        assert_eq!(status, 200, "case round {round}: got {received:?}");
        assert_eq!(
            received["Messages"].as_array().map(Vec::len),
            Some(1),
            "case round {round}: expected the one message back, got {received:?}"
        );
        let handle = received["Messages"][0]["ReceiptHandle"]
            .as_str()
            .unwrap_or_else(|| panic!("case round {round}: no ReceiptHandle in {received:?}"))
            .to_string();

        if let Some(stale) = previous.replace(handle.clone()) {
            assert_ne!(
                stale, handle,
                "case round {round}: each receive must mint a different handle"
            );
            let (status, answer) = sqs
                .call(
                    "DeleteMessage",
                    &json!({ "QueueUrl": url, "ReceiptHandle": stale }),
                )
                .await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (404, "ReceiptHandleIsInvalid"),
                "case round {round}: the superseded handle must no longer be live, \
                 got {status} {answer:?}"
            );
        }
    }

    // Only the newest handle deletes it, and the message really goes.
    let newest = previous.expect("a handle from the last receive");
    let (status, answer) = sqs
        .call(
            "DeleteMessage",
            &json!({ "QueueUrl": url, "ReceiptHandle": newest }),
        )
        .await;
    assert_eq!(status, 200, "case the newest handle: got {answer:?}");
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        "0",
        "case after delete: the message must be gone"
    );
}

/// `MaxNumberOfMessages` caps a receive at its value across the whole documented range
/// (1 to 10); outside it the request is refused. Every message sent is delivered exactly
/// once while it stays in flight.
#[tokio::test]
async fn max_number_of_messages_caps_the_batch_across_its_documented_range() {
    for requested in 1..=10_usize {
        let sqs = Emulator::new();
        let url = sqs.create_queue("batch-queue").await;
        // Ten messages, so every cap in the range is actually exercised.
        let mut sent = Vec::new();
        for index in 0..10 {
            let (status, answer) = sqs
                .call(
                    "SendMessage",
                    &json!({ "QueueUrl": url, "MessageBody": format!("body-{index}") }),
                )
                .await;
            assert_eq!(status, 200, "case cap {requested}: send got {answer:?}");
            sent.push(answer["MessageId"].as_str().expect("an id").to_string());
        }

        let (status, received) = sqs
            .call(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": requested }),
            )
            .await;
        assert_eq!(status, 200, "case cap {requested}: got {received:?}");
        let delivered = received["Messages"].as_array().expect("messages").clone();
        assert_eq!(
            delivered.len(),
            requested,
            "case cap {requested}: expected {requested} messages, got {}",
            delivered.len()
        );
        let mut ids: Vec<&str> = delivered
            .iter()
            .map(|message| message["MessageId"].as_str().expect("an id"))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            requested,
            "case cap {requested}: the same message was delivered twice: {delivered:?}"
        );
        for id in &ids {
            assert!(
                sent.iter().any(|minted| minted == id),
                "case cap {requested}: {id:?} was never sent"
            );
        }
    }

    let sqs = Emulator::new();
    let url = sqs.create_queue("batch-bounds").await;
    for outside in [0, 11, -1, 1000] {
        let (status, answer) = sqs
            .call(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": outside }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case MaxNumberOfMessages={outside}: expected a refusal, got {status} {answer:?}"
        );
    }
}

/// A receive makes the message invisible for the effective visibility timeout — the
/// request's value when it gives one, else the queue's attribute — and the receive count
/// rises each time the message is handed over.
#[tokio::test]
async fn the_effective_visibility_timeout_decides_when_a_message_returns() {
    // `(queue VisibilityTimeout, request VisibilityTimeout, visible again at once)`
    let cases: [(&str, Option<i32>, bool); 5] = [
        ("30", None, false),
        ("0", None, true),
        ("30", Some(0), true),
        ("0", Some(30), false),
        ("1", None, false),
    ];

    for (index, (queue_timeout, request_timeout, immediately_visible)) in cases.iter().enumerate() {
        let case = format!("queue={queue_timeout} request={request_timeout:?}");
        let sqs = Emulator::new();
        let url = sqs
            .create_queue_with(
                &format!("vis-{index}"),
                &json!({ "VisibilityTimeout": queue_timeout }),
            )
            .await;
        sqs.call(
            "SendMessage",
            &json!({ "QueueUrl": url, "MessageBody": "payload" }),
        )
        .await;

        let mut first = json!({ "QueueUrl": url });
        if let Some(timeout) = request_timeout {
            first["VisibilityTimeout"] = json!(timeout);
        }
        let (status, received) = sqs.call("ReceiveMessage", &first).await;
        assert_eq!(status, 200, "case {case}: got {received:?}");
        assert_eq!(
            received["Messages"][0]["Attributes"]["ApproximateReceiveCount"].as_str(),
            None,
            "case {case}: no system attribute was requested, so none is reported"
        );

        let (status, again) = sqs
            .call(
                "ReceiveMessage",
                &json!({
                    "QueueUrl": url,
                    "MessageSystemAttributeNames": ["ApproximateReceiveCount"],
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case}: got {again:?}");
        if *immediately_visible {
            assert_eq!(
                again["Messages"][0]["Attributes"]["ApproximateReceiveCount"].as_str(),
                Some("2"),
                "case {case}: expected the message back on its second receive, got {again:?}"
            );
        } else {
            assert!(
                again.get("Messages").is_none(),
                "case {case}: the message must stay in flight, got {again:?}"
            );
        }
    }
}

/// A message sent with `DelaySeconds` is counted as delayed and withheld until its delay
/// elapses; then it is visible.
#[tokio::test]
async fn a_delayed_message_is_withheld_until_its_delay_elapses() {
    let sqs = Emulator::new();
    let url = sqs.create_queue("delay-queue").await;
    sqs.call(
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": "later", "DelaySeconds": 1 }),
    )
    .await;

    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessagesDelayed").await,
        "1",
        "case freshly delayed: expected it counted as delayed"
    );
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        "0",
        "case freshly delayed: expected nothing visible"
    );
    let (status, received) = sqs
        .call("ReceiveMessage", &json!({ "QueueUrl": url }))
        .await;
    assert_eq!(status, 200, "case freshly delayed: got {received:?}");
    assert!(
        received.get("Messages").is_none(),
        "case freshly delayed: expected no messages, got {received:?}"
    );

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        "1",
        "case after the delay: expected it visible"
    );
    let (status, received) = sqs
        .call("ReceiveMessage", &json!({ "QueueUrl": url }))
        .await;
    assert_eq!(status, 200, "case after the delay: got {received:?}");
    assert_eq!(
        received["Messages"][0]["Body"].as_str(),
        Some("later"),
        "case after the delay: got {received:?}"
    );

    // A delay outside the documented 0..900 range is refused.
    for outside in [-1, 901] {
        let (status, answer) = sqs
            .call(
                "SendMessage",
                &json!({ "QueueUrl": url, "MessageBody": "x", "DelaySeconds": outside }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case DelaySeconds={outside}: expected a refusal, got {status} {answer:?}"
        );
    }
}

/// A long-polling receive waits for `WaitTimeSeconds` before answering empty, and
/// returns as soon as a message is there.
#[tokio::test]
async fn a_long_polling_receive_waits_for_the_requested_time() {
    let sqs = Emulator::new();
    let url = sqs.create_queue("poll-queue").await;

    let started = Instant::now();
    let (status, received) = sqs
        .call(
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "WaitTimeSeconds": 1 }),
        )
        .await;
    let waited = started.elapsed();
    assert_eq!(status, 200, "case empty queue: got {received:?}");
    assert!(
        received.get("Messages").is_none(),
        "case empty queue: got {received:?}"
    );
    assert!(
        waited >= Duration::from_millis(900),
        "case empty queue: expected to wait about a second, waited {waited:?}"
    );

    // With a message already waiting, the same call returns at once.
    sqs.call(
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": "ready" }),
    )
    .await;
    let started = Instant::now();
    let (status, received) = sqs
        .call(
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "WaitTimeSeconds": 20 }),
        )
        .await;
    let waited = started.elapsed();
    assert_eq!(status, 200, "case message waiting: got {received:?}");
    assert_eq!(
        received["Messages"][0]["Body"].as_str(),
        Some("ready"),
        "case message waiting: got {received:?}"
    );
    assert!(
        waited < Duration::from_millis(500),
        "case message waiting: expected an immediate answer, waited {waited:?}"
    );

    // Outside the documented 0..20 range the request is refused.
    for outside in [-1, 21] {
        let (status, answer) = sqs
            .call(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "WaitTimeSeconds": outside }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case WaitTimeSeconds={outside}: expected a refusal, got {status} {answer:?}"
        );
    }
}

/// `MD5OfMessageAttributes` is the MD5 of the exact byte layout Amazon SQS defines, not
/// merely *some* stable digest.
///
/// Each row writes that layout out by hand from the Amazon SQS Developer Guide
/// ("Calculating the MD5 message digest for message attributes") — name, `DataType` and
/// value each prefixed with a 4-byte big-endian length, with one transport-type byte
/// (`1` String, `2` Binary) before the value, attributes in name order — and the
/// expected digest is that literal buffer run through the `md-5` crate. A layout that
/// disagreed with the guide would fail here even though it was stable.
#[tokio::test]
async fn md5_of_message_attributes_matches_the_documented_byte_layout() {
    use md5::{Digest, Md5};

    // `(label, request attributes, the documented encoding of those attributes)`
    let cases: [(&str, Value, Vec<u8>); 4] = [
        (
            "one String attribute",
            json!({ "colour": { "DataType": "String", "StringValue": "green" } }),
            [
                &[0, 0, 0, 6][..],
                b"colour",
                &[0, 0, 0, 6][..],
                b"String",
                &[1][..],
                &[0, 0, 0, 5][..],
                b"green",
            ]
            .concat(),
        ),
        (
            "a Number attribute (DataType is part of the digest)",
            json!({ "colour": { "DataType": "Number", "StringValue": "green" } }),
            [
                &[0, 0, 0, 6][..],
                b"colour",
                &[0, 0, 0, 6][..],
                b"Number",
                &[1][..],
                &[0, 0, 0, 5][..],
                b"green",
            ]
            .concat(),
        ),
        (
            "two attributes, digested in name order",
            json!({
                "b": { "DataType": "Number", "StringValue": "2" },
                "a": { "DataType": "String", "StringValue": "1" },
            }),
            [
                &[0, 0, 0, 1][..],
                b"a",
                &[0, 0, 0, 6][..],
                b"String",
                &[1][..],
                &[0, 0, 0, 1][..],
                b"1",
                &[0, 0, 0, 1][..],
                b"b",
                &[0, 0, 0, 6][..],
                b"Number",
                &[1][..],
                &[0, 0, 0, 1][..],
                b"2",
            ]
            .concat(),
        ),
        (
            "a Binary attribute (transport type 2)",
            // 3q2+7w== is base64 for DE AD BE EF.
            json!({ "blob": { "DataType": "Binary", "BinaryValue": "3q2+7w==" } }),
            [
                &[0, 0, 0, 4][..],
                b"blob",
                &[0, 0, 0, 6][..],
                b"Binary",
                &[2][..],
                &[0, 0, 0, 4][..],
                &[0xDE, 0xAD, 0xBE, 0xEF][..],
            ]
            .concat(),
        ),
    ];

    let sqs = Emulator::new();
    for (index, (label, attributes, encoding)) in cases.iter().enumerate() {
        let url = sqs.create_queue(&format!("layout-{index}")).await;
        let expected = hex::encode(Md5::digest(encoding));
        let (status, sent) = sqs
            .call(
                "SendMessage",
                &json!({
                    "QueueUrl": url,
                    "MessageBody": "payload",
                    "MessageAttributes": attributes,
                }),
            )
            .await;
        assert_eq!(status, 200, "case {label}: got {sent:?}");
        assert_eq!(
            sent["MD5OfMessageAttributes"].as_str(),
            Some(expected.as_str()),
            "case {label}: expected MD5OfMessageAttributes {expected}, got {:?}",
            sent["MD5OfMessageAttributes"]
        );

        // The same digest comes back on the receive, over the attributes it returns.
        let (status, received) = sqs
            .call(
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MessageAttributeNames": ["All"] }),
            )
            .await;
        assert_eq!(status, 200, "case {label}: got {received:?}");
        assert_eq!(
            received["Messages"][0]["MD5OfMessageAttributes"].as_str(),
            Some(expected.as_str()),
            "case {label}: the received digest must be the documented one, got {received:?}"
        );
    }

    // No attributes at all: SQS reports no digest.
    let bare = sqs.create_queue("layout-bare").await;
    let (status, sent) = sqs
        .call(
            "SendMessage",
            &json!({ "QueueUrl": bare, "MessageBody": "payload" }),
        )
        .await;
    assert_eq!(status, 200, "case no attributes: got {sent:?}");
    assert!(
        sent.get("MD5OfMessageAttributes").is_none(),
        "case no attributes: expected no digest, got {sent:?}"
    );
}

/// `MessageAttributeNames` selects by exact name, by `prefix.*`, or everything with
/// `All`/`.*` — and an absent list selects nothing.
#[tokio::test]
async fn message_attribute_names_select_exactly_what_was_asked_for() {
    let sqs = Emulator::new();
    let url = sqs.create_queue("attr-select-queue").await;
    sqs.call(
        "SendMessage",
        &json!({
            "QueueUrl": url,
            "MessageBody": "payload",
            "MessageAttributes": {
                "bar.baz": { "DataType": "String", "StringValue": "1" },
                "barbaz": { "DataType": "String", "StringValue": "2" },
                "qux": { "DataType": "String", "StringValue": "3" },
            },
        }),
    )
    .await;

    // `(requested names, the attribute names the answer must carry)`
    let cases: [(Vec<&str>, Vec<&str>); 8] = [
        (vec!["All"], vec!["bar.baz", "barbaz", "qux"]),
        (vec![".*"], vec!["bar.baz", "barbaz", "qux"]),
        // The `.` belongs to the prefix, so `barbaz` is NOT selected by `bar.*`.
        (vec!["bar.*"], vec!["bar.baz"]),
        // The documented selector forms are only `All`, `.*` and `prefix.*`: `bar*` and
        // a bare `*` are exact names, which nothing here is called.
        (vec!["bar*"], vec![]),
        (vec!["*"], vec![]),
        (vec!["qux"], vec!["qux"]),
        (vec!["qux", "barbaz"], vec!["barbaz", "qux"]),
        (vec![], vec![]),
    ];

    for (requested, expected) in &cases {
        let mut body = json!({ "QueueUrl": url, "VisibilityTimeout": 0 });
        if !requested.is_empty() {
            body["MessageAttributeNames"] = json!(requested);
        }
        let (status, received) = sqs.call("ReceiveMessage", &body).await;
        assert_eq!(status, 200, "case {requested:?}: got {received:?}");
        let mut selected: Vec<String> = received["Messages"][0]["MessageAttributes"]
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        selected.sort();
        assert_eq!(
            selected, *expected,
            "case {requested:?}: expected {expected:?}, got {selected:?}"
        );
    }
}

/// Members `SendMessage` cannot honour are refused by name, not accepted and silently
/// dropped on a 200: the list-valued message attributes and the value-less one the model
/// documents as "Not implemented. Reserved for future use.", the FIFO-only member a
/// standard queue has no meaning for, and the trace-header member.
#[tokio::test]
async fn send_message_refuses_the_members_it_cannot_honour() {
    let sqs = Emulator::new();
    let url = sqs.create_queue("refusal-queue").await;

    let attributes: [(&str, Value); 3] = [
        (
            "StringListValues",
            json!({ "many": { "DataType": "String.Array", "StringListValues": ["a", "b"] } }),
        ),
        (
            "BinaryListValues",
            json!({ "many": { "DataType": "Binary.Array", "BinaryListValues": ["3q2+7w=="] } }),
        ),
        // No value at all: there is no transport-type byte for "nothing", so SQS
        // defines no digest encoding for it.
        ("no value", json!({ "empty": { "DataType": "String" } })),
    ];
    for (label, attribute) in &attributes {
        let (status, answer) = sqs
            .call(
                "SendMessage",
                &json!({
                    "QueueUrl": url,
                    "MessageBody": "payload",
                    "MessageAttributes": attribute,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {label}: expected a refusal, got {status} {answer:?}"
        );
    }

    let unsupported: [(&str, Value); 2] = [
        (
            "MessageDeduplicationId",
            json!({ "MessageDeduplicationId": "dedup-1" }),
        ),
        (
            "MessageSystemAttributes",
            json!({
                "MessageSystemAttributes": {
                    "AWSTraceHeader": { "DataType": "String", "StringValue": "Root=1-00-00" },
                },
            }),
        ),
    ];
    for (label, extra) in &unsupported {
        let mut body = json!({ "QueueUrl": url, "MessageBody": "payload" });
        for (member, value) in extra.as_object().expect("an object of members") {
            body[member] = value.clone();
        }
        let (status, answer) = sqs.call("SendMessage", &body).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {label}: expected a refusal naming the member, got {status} {answer:?}"
        );
        assert!(
            answer["message"]
                .as_str()
                .is_some_and(|message| message.contains(label)),
            "case {label}: the refusal must name the member, got {answer:?}"
        );
    }

    // Nothing was stored by any of those refusals.
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        "0",
        "case after the refusals: the queue must still be empty"
    );
}

/// A message over the queue's `MaximumMessageSize` is refused rather than stored, and
/// the limit covers the message attributes as well as the body — "All parts of a message
/// are included in the ... size limit: the message body, the message attributes". A
/// message may carry at most the documented 10 attributes.
#[tokio::test]
async fn a_message_over_the_queues_maximum_message_size_is_refused() {
    use service_com_amazonaws_sqs_amazonsqs::state::MAX_MESSAGE_ATTRIBUTES;

    // `k` (1) + `String` (6) + `v` (1): the name, the data type and the value.
    const ATTRIBUTE_BYTES: usize = 8;
    let attribute = json!({ "k": { "DataType": "String", "StringValue": "v" } });

    for (index, maximum) in [1_024_usize, 2_048, 262_144].into_iter().enumerate() {
        let sqs = Emulator::new();
        let url = sqs
            .create_queue_with(
                &format!("size-{index}"),
                &json!({ "MaximumMessageSize": maximum.to_string() }),
            )
            .await;

        // `(label, body length, send the attribute, accepted)`
        let cases: [(&str, usize, bool, bool); 5] = [
            ("body alone at the limit", maximum, false, true),
            ("body alone one over", maximum + 1, false, false),
            ("body at the limit plus an attribute", maximum, true, false),
            (
                "body and attribute exactly at the limit",
                maximum - ATTRIBUTE_BYTES,
                true,
                true,
            ),
            (
                "body and attribute one over",
                maximum - ATTRIBUTE_BYTES + 1,
                true,
                false,
            ),
        ];
        for (label, body_len, with_attribute, accepted) in &cases {
            let mut body = json!({ "QueueUrl": url, "MessageBody": "x".repeat(*body_len) });
            if *with_attribute {
                body["MessageAttributes"] = attribute.clone();
            }
            let (status, answer) = sqs.call("SendMessage", &body).await;
            if *accepted {
                assert_eq!(
                    status, 200,
                    "case maximum {maximum} / {label}: expected acceptance, got {status} {answer:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (400, "ValidationException"),
                    "case maximum {maximum} / {label}: expected a refusal, got {status} {answer:?}"
                );
            }
        }
    }

    // At most 10 attributes: the published bound is accepted, one past it refused.
    let sqs = Emulator::new();
    let roomy = sqs.create_queue("attribute-count").await;
    for count in [1, MAX_MESSAGE_ATTRIBUTES, MAX_MESSAGE_ATTRIBUTES + 1] {
        let attributes: serde_json::Map<String, Value> = (0..count)
            .map(|index| {
                (
                    format!("a{index}"),
                    json!({ "DataType": "String", "StringValue": "v" }),
                )
            })
            .collect();
        let (status, answer) = sqs
            .call(
                "SendMessage",
                &json!({
                    "QueueUrl": roomy,
                    "MessageBody": "payload",
                    "MessageAttributes": Value::Object(attributes),
                }),
            )
            .await;
        if count <= MAX_MESSAGE_ATTRIBUTES {
            assert_eq!(
                status, 200,
                "case {count} attributes: expected acceptance, got {status} {answer:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {count} attributes: expected a refusal, got {status} {answer:?}"
            );
        }
    }
}

/// What this in-memory store can be made to hold is bounded: a queue holds at most
/// `MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE` message bytes and `MAX_MESSAGES_PER_QUEUE`
/// messages, and the whole store at most `MAX_RETAINED_MESSAGE_BYTES` — each refused with
/// the service's modelled `OverLimit`, never by silently evicting a message a client was
/// told was accepted.
#[tokio::test]
async fn the_retained_message_bytes_and_count_are_bounded() {
    use service_com_amazonaws_sqs_amazonsqs::state::{
        MAX_MESSAGES_PER_QUEUE, MAX_RETAINED_MESSAGE_BYTES, MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE,
    };
    const MIB: usize = 1024 * 1024;

    // Per-queue byte budget: 1 MiB bodies until the budget is full, then a refusal.
    let sqs = Emulator::new();
    let url = sqs
        .create_queue_with(
            "byte-budget",
            &json!({ "MaximumMessageSize": MIB.to_string() }),
        )
        .await;
    let body = "x".repeat(MIB);
    for index in 0..MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE / MIB {
        let (status, answer) = sqs
            .call(
                "SendMessage",
                &json!({ "QueueUrl": url, "MessageBody": body }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case MiB {index}: expected acceptance below the budget, got {status} {answer:?}"
        );
    }
    let (status, answer) = sqs
        .call(
            "SendMessage",
            &json!({ "QueueUrl": url, "MessageBody": body }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (403, "OverLimit"),
        "case one MiB past the per-queue budget: got {status} {answer:?}"
    );
    // Nothing was evicted to make room: every accepted message is still there.
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        (MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE / MIB).to_string(),
        "case refusal not eviction: the accepted messages must all still be held"
    );

    // Store-wide byte budget: fill enough queues to exhaust it, then any further send
    // on a queue that is itself empty is still refused.
    let queues = MAX_RETAINED_MESSAGE_BYTES / MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE;
    for queue in 1..queues {
        let url = sqs
            .create_queue_with(
                &format!("byte-budget-{queue}"),
                &json!({ "MaximumMessageSize": MIB.to_string() }),
            )
            .await;
        for index in 0..MAX_RETAINED_MESSAGE_BYTES_PER_QUEUE / MIB {
            let (status, answer) = sqs
                .call(
                    "SendMessage",
                    &json!({ "QueueUrl": url, "MessageBody": body }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case queue {queue} MiB {index}: got {status} {answer:?}"
            );
        }
    }
    let fresh = sqs.create_queue("byte-budget-fresh").await;
    let (status, answer) = sqs
        .call(
            "SendMessage",
            &json!({ "QueueUrl": fresh, "MessageBody": body }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (403, "OverLimit"),
        "case store-wide budget exhausted: an empty queue must still refuse, \
         got {status} {answer:?}"
    );

    // Per-queue message count: tiny bodies, so only the count can bind.
    let sqs = Emulator::new();
    let url = sqs.create_queue("count-budget").await;
    for index in 0..MAX_MESSAGES_PER_QUEUE {
        let (status, answer) = sqs
            .call(
                "SendMessage",
                &json!({ "QueueUrl": url, "MessageBody": "x" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case message {index}: expected acceptance below the bound, got {status} {answer:?}"
        );
    }
    let (status, answer) = sqs
        .call(
            "SendMessage",
            &json!({ "QueueUrl": url, "MessageBody": "x" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (403, "OverLimit"),
        "case one message past the bound: got {status} {answer:?}"
    );
}

/// A FIFO queue serves the message plane like any other queue — the FIFO rules
/// themselves (the tokens a send must carry, the sequence number it is assigned,
/// per-group order on receive and deduplication) are pinned by `fifo_queue.rs`; what
/// this suite pins is that a send to one is accepted and that the queue then holds it.
#[tokio::test]
async fn sending_to_a_fifo_queue_is_accepted_and_held() {
    let sqs = Emulator::new();
    let url = sqs
        .create_queue_with("ordered.fifo", &json!({ "FifoQueue": "true" }))
        .await;

    let (status, answer) = sqs
        .call(
            "SendMessage",
            &json!({
                "QueueUrl": url,
                "MessageBody": "x",
                "MessageGroupId": "g",
                "MessageDeduplicationId": "d",
            }),
        )
        .await;
    assert_eq!(status, 200, "case send to a FIFO queue: got {status} {answer:?}");
    assert!(
        answer["SequenceNumber"]
            .as_str()
            .is_some_and(|number| !number.is_empty()),
        "case send to a FIFO queue: expected a SequenceNumber, got {answer:?}"
    );
    assert_eq!(
        sqs.count(&url, "ApproximateNumberOfMessages").await,
        "1",
        "case after the send: the FIFO queue must hold the message"
    );

    let (status, received) = sqs
        .call("ReceiveMessage", &json!({ "QueueUrl": url }))
        .await;
    assert_eq!(status, 200, "case ReceiveMessage on a FIFO queue: {received:?}");
    assert_eq!(
        received["Messages"][0]["Body"].as_str(),
        Some("x"),
        "case ReceiveMessage on a FIFO queue: expected the message back, got {received:?}"
    );
}

/// `ListQueues` lists the queues in state, filtered by `QueueNamePrefix` and paged by
/// `MaxResults`/`NextToken`.
#[tokio::test]
async fn list_queues_filters_by_prefix_and_pages() {
    let sqs = Emulator::new();
    let mut alpha = Vec::new();
    for name in ["alpha-1", "alpha-2", "alpha-3"] {
        alpha.push(sqs.create_queue(name).await);
    }
    let beta = sqs.create_queue("beta-1").await;
    alpha.sort();

    // `(prefix, expected urls)`
    let cases: Vec<(Option<&str>, Vec<String>)> = vec![
        (None, {
            let mut all = alpha.clone();
            all.push(beta.clone());
            all.sort();
            all
        }),
        (Some("alpha"), alpha.clone()),
        (Some("alpha-2"), vec![alpha[1].clone()]),
        (Some("beta"), vec![beta.clone()]),
        (Some("gamma"), vec![]),
    ];
    for (prefix, expected) in &cases {
        let mut body = json!({});
        if let Some(prefix) = prefix {
            body["QueueNamePrefix"] = json!(prefix);
        }
        let (status, answer) = sqs.call("ListQueues", &body).await;
        assert_eq!(status, 200, "case prefix {prefix:?}: got {answer:?}");
        let listed: Vec<String> = answer["QueueUrls"]
            .as_array()
            .map(|urls| {
                urls.iter()
                    .map(|url| url.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            &listed, expected,
            "case prefix {prefix:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // Paging walks every queue exactly once, for every page size.
    for page_size in 1..=4 {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(next) = &token {
                body["NextToken"] = json!(next);
            }
            let (status, answer) = sqs.call("ListQueues", &body).await;
            assert_eq!(status, 200, "case page size {page_size}: got {answer:?}");
            let page: Vec<String> = answer["QueueUrls"]
                .as_array()
                .map(|urls| {
                    urls.iter()
                        .map(|url| url.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page held {} urls",
                page.len()
            );
            seen.extend(page);
            token = answer["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        let mut expected = alpha.clone();
        expected.push(beta.clone());
        expected.sort();
        seen.sort();
        assert_eq!(
            seen, expected,
            "case page size {page_size}: paging must visit every queue exactly once"
        );
    }
}

/// `ListQueues` without `MaxResults` cannot be made to answer with the whole store: it
/// pages at the emulator's default size, and the caller walks the rest with `NextToken`.
/// A `NextToken` this listing could not have issued is refused rather than silently
/// restarting.
#[tokio::test]
async fn an_unparameterised_list_queues_pages_at_the_default_size() {
    use service_com_amazonaws_sqs_amazonsqs::state::DEFAULT_LIST_PAGE_SIZE;

    let sqs = Emulator::new();
    let total = DEFAULT_LIST_PAGE_SIZE + 25;
    for index in 0..total {
        // Zero-padded, so the lexicographic listing order is the numeric one.
        sqs.create_queue(&format!("page-{index:04}")).await;
    }

    let (status, answer) = sqs.call("ListQueues", &json!({})).await;
    assert_eq!(status, 200, "case unparameterised: got {answer:?}");
    assert_eq!(
        answer["QueueUrls"].as_array().map(Vec::len),
        Some(DEFAULT_LIST_PAGE_SIZE),
        "case unparameterised: expected a page of {DEFAULT_LIST_PAGE_SIZE}, got {:?}",
        answer["QueueUrls"].as_array().map(Vec::len)
    );
    let token = answer["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case unparameterised: expected a NextToken in {answer:?}"))
        .to_string();

    let (status, answer) = sqs
        .call("ListQueues", &json!({ "NextToken": token }))
        .await;
    assert_eq!(status, 200, "case second page: got {answer:?}");
    assert_eq!(
        answer["QueueUrls"].as_array().map(Vec::len),
        Some(total - DEFAULT_LIST_PAGE_SIZE),
        "case second page: got {answer:?}"
    );
    assert!(
        answer.get("NextToken").is_none(),
        "case second page: expected the listing to end, got {answer:?}"
    );

    for bogus in ["not-a-cursor", "", "-1", "1.5", "99999"] {
        let (status, answer) = sqs
            .call("ListQueues", &json!({ "NextToken": bogus }))
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case NextToken {bogus:?}: expected a refusal, got {status} {answer:?}"
        );
    }
}

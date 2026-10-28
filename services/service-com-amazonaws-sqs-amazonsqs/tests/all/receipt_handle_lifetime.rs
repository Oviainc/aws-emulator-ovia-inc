// SPDX-License-Identifier: CC0-1.0

//! How long a RECEIPT HANDLE names its message, driven over the wire through the shared
//! harness in [`super::support`].
//!
//! The rule under test: a receipt handle names the message a receive handed over for as
//! long as the queue still HOLDS that message, not only while the message is hidden — so
//! an operation that resolves a handle (a visibility change, a delete) is answered from
//! whether that message still exists. A consumer whose visibility window has lapsed
//! therefore re-hides, releases or deletes the message it is still working on, and
//! `VisibilityTimeout` is applied from the instant of the call whatever the message's
//! visibility was before it ("the timeout value is applied immediately"). The other side
//! of the same rule — a handle whose message was deleted, purged, expired or superseded by
//! a later receive names nothing, and is refused — is pinned by
//! [`super::visibility_change`].
//!
//! The tables span what the rule generalises over rather than one recorded request: the
//! queue TYPE (standard and FIFO), the span each receive hides the message for, HOW MANY
//! times that window has lapsed before the handle is used again, the span the call then
//! asks for (the documented minimum, the queue's own, the documented maximum), and the
//! OPERATION the handle is presented to (both visibility changes and both deletes).
//!
//! A visibility timeout is a documented duration measured in seconds at the shortest, so
//! no row sleeps for one: they move the instant a stored message stays hidden until into
//! the past through the engine seam [`super::support::emulator_over_engine`] hands back,
//! and everything they then assert goes over the wire like any other request.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    queue_name_from_url, ComAmazonawsSqsAmazonsqsState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    call, create_ok, emulator_over_engine, fifo_attributes, message_counts, messages, ok, REGION,
};

/// The bounds `ChangeMessageVisibilityRequest$VisibilityTimeout` documents: "Values range:
/// 0 to 43200. Maximum: 12 hours."
const MIN_TIMEOUT: i64 = 0;
const MAX_TIMEOUT: i64 = 43_200;

/// The one message group every FIFO row sends under; order inside a group is not what
/// these rows are about, and a standard queue needs none.
const GROUP: &str = "handles";

/// Make every message the queue at `url` still hides visible again, by moving the instant
/// it stays hidden until into the past — the lapse of a visibility window, reached without
/// sleeping for it. Nothing receives the message, so the handle the earlier receive minted
/// is still the one the queue holds it under.
fn lapse_visibility(engine: &engine::InMemoryEngine, url: &str) {
    let name = queue_name_from_url(url)
        .unwrap_or_else(|| panic!("case lapsing {url}: the url names no queue"))
        .to_string();
    let past = SystemTime::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(SystemTime::now);
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    let queue = store
        .queue_mut(REGION, &name)
        .unwrap_or_else(|| panic!("case lapsing {url}: no queue named {name}"));
    for message in &mut queue.messages {
        if message.invisible_until.is_some() {
            message.invisible_until = Some(past);
        }
    }
}

/// Send one message to the queue at `url`, which carries a group id on a FIFO queue and
/// none on a standard one.
async fn send(router: &MultiServiceRouter, url: &str, body: &str, fifo: bool) {
    let mut request = json!({"QueueUrl": url, "MessageBody": body});
    if fifo {
        request["MessageGroupId"] = json!(GROUP);
        // A FIFO send supplies its own deduplication id unless the queue derives one, and
        // the member takes "alphanumeric and punctuation characters" only, so the row's
        // own words are joined into one token.
        request["MessageDeduplicationId"] = json!(body.replace(' ', "-"));
    }
    let _ = ok(router, "SendMessage", request).await;
}

/// Receive the one message the queue at `url` holds and answer the handle that receive
/// minted.
async fn receive_handle(router: &MultiServiceRouter, url: &str, case: &str) -> String {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 1, "WaitTimeSeconds": 1}),
    )
    .await;
    let handed = messages(&reply);
    assert_eq!(
        handed.len(),
        1,
        "case {case:?}: the receive must hand over the one message sent, got {reply}"
    );
    handed[0]["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a received message carries a handle, got {reply}"))
        .to_string()
}

/// Ask for `requested` seconds on the handle `receipt_handle` of the queue at `url`.
async fn change(
    router: &MultiServiceRouter,
    url: &str,
    receipt_handle: &str,
    requested: i64,
) -> (u16, Value) {
    call(
        router,
        "ChangeMessageVisibility",
        &json!({
            "QueueUrl": url,
            "ReceiptHandle": receipt_handle,
            "VisibilityTimeout": requested,
        }),
    )
    .await
}

/// One row of the lapsed-handle table.
struct LapsedRow {
    case: &'static str,
    /// Whether the queue the row drives is a FIFO queue.
    fifo: bool,
    /// The queue's own `VisibilityTimeout`: the span each receive hides the message for,
    /// and the span the row re-hides it for between two lapses.
    queue_timeout: i64,
    /// How many times that window lapses before the row makes its change.
    lapses: usize,
    /// The span the change then asks for.
    requested: i64,
}

/// A handle whose visibility window has lapsed still names its message, so the change is
/// applied: the message is hidden again from the instant of the call, and a span of zero
/// leaves it available.
#[tokio::test]
async fn a_lapsed_handle_still_names_its_message_for_a_visibility_change() {
    let rows: &[LapsedRow] = &[
        LapsedRow {
            case: "a window that has just lapsed, hidden again for the queue's own span",
            fifo: false,
            queue_timeout: 30,
            lapses: 1,
            requested: 30,
        },
        LapsedRow {
            case: "a lapsed handle asking for the documented minimum",
            fifo: false,
            queue_timeout: 30,
            lapses: 1,
            requested: MIN_TIMEOUT,
        },
        LapsedRow {
            case: "a lapsed handle asking for the documented maximum",
            fifo: false,
            queue_timeout: 1,
            lapses: 1,
            requested: MAX_TIMEOUT,
        },
        LapsedRow {
            case: "a window that has lapsed three times over",
            fifo: false,
            queue_timeout: 2,
            lapses: 3,
            requested: 600,
        },
        LapsedRow {
            case: "a lapsed handle of a FIFO queue",
            fifo: true,
            queue_timeout: 30,
            lapses: 1,
            requested: 120,
        },
        LapsedRow {
            case: "a lapsed handle of a FIFO queue, released again",
            fifo: true,
            queue_timeout: 5,
            lapses: 2,
            requested: MIN_TIMEOUT,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, engine) = emulator_over_engine();
        let timeout = row.queue_timeout.to_string();
        let (name, attributes) = if row.fifo {
            (
                format!("lapsed-handle-{index}.fifo"),
                fifo_attributes(&[("VisibilityTimeout", &timeout)]),
            )
        } else {
            (
                format!("lapsed-handle-{index}"),
                json!({"VisibilityTimeout": timeout}),
            )
        };
        let url = create_ok(&router, &name, &attributes).await;
        send(&router, &url, row.case, row.fifo).await;
        let handle = receive_handle(&router, &url, row.case).await;

        for lapse in 1..=row.lapses {
            lapse_visibility(&engine, &url);
            let counts = message_counts(&router, &url).await;
            assert_eq!(
                (counts.0, counts.2),
                (1, 0),
                "case {:?}: after lapse {lapse} the message is visible again, got \
                 {counts:?} (available, delayed, in flight)",
                row.case
            );
            if lapse < row.lapses {
                // The same handle hides it again, so the NEXT lapse has a window of its
                // own to lapse — and the handle is never re-minted, because nothing
                // receives the message again.
                let (status, body) = change(&router, &url, &handle, row.queue_timeout).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: hiding the message again after lapse {lapse} must be \
                     accepted, got {status} with {body}",
                    row.case
                );
            }
        }

        let (status, body) = change(&router, &url, &handle, row.requested).await;
        assert_eq!(
            (status, &body),
            (200, &json!({})),
            "case {:?}: a lapsed handle still names its message, so the change for {} \
             seconds must be accepted, got {status} with {body}",
            row.case,
            row.requested
        );

        // The change is applied from the instant of the call: a span of zero leaves the
        // message available, anything else hides it again.
        let hidden = row.requested != MIN_TIMEOUT;
        let counts = message_counts(&router, &url).await;
        assert_eq!(
            (counts.0, counts.2),
            (i64::from(!hidden), i64::from(hidden)),
            "case {:?}: {} seconds must leave the message {}, got {counts:?} (available, \
             delayed, in flight)",
            row.case,
            row.requested,
            if hidden { "hidden" } else { "available" }
        );
        let handed = messages(
            &ok(
                &router,
                "ReceiveMessage",
                json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
            )
            .await,
        )
        .len();
        assert_eq!(
            handed,
            usize::from(!hidden),
            "case {:?}: a receive hands over exactly what the change left available, got \
             {handed} message(s)",
            row.case
        );
    }
}

/// What the row's operation leaves of the message it named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    /// Hidden again, under the span the call asked for.
    Hidden,
    /// Nothing: the queue no longer holds it.
    Gone,
}

/// One row of the operations table: an operation that resolves a receipt handle, and what
/// it leaves of the message once it has been given a lapsed one.
struct OperationRow {
    case: &'static str,
    operation: &'static str,
    left: Left,
}

/// The lifetime belongs to the HANDLE, not to one operation: every operation that names a
/// message by its receipt handle takes one whose visibility window has lapsed.
#[tokio::test]
async fn a_lapsed_handle_is_taken_by_every_operation_that_names_a_message() {
    let rows: &[OperationRow] = &[
        OperationRow {
            case: "a visibility change",
            operation: "ChangeMessageVisibility",
            left: Left::Hidden,
        },
        OperationRow {
            case: "a visibility change in a batch",
            operation: "ChangeMessageVisibilityBatch",
            left: Left::Hidden,
        },
        OperationRow {
            case: "a delete",
            operation: "DeleteMessage",
            left: Left::Gone,
        },
        OperationRow {
            case: "a delete in a batch",
            operation: "DeleteMessageBatch",
            left: Left::Gone,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, engine) = emulator_over_engine();
        let url = create_ok(
            &router,
            &format!("lapsed-handle-for-{index}"),
            &json!({"VisibilityTimeout": "5"}),
        )
        .await;
        send(&router, &url, row.case, false).await;
        let handle = receive_handle(&router, &url, row.case).await;
        lapse_visibility(&engine, &url);

        let request = match row.operation {
            "ChangeMessageVisibility" => {
                json!({"QueueUrl": url, "ReceiptHandle": handle, "VisibilityTimeout": 30})
            }
            "ChangeMessageVisibilityBatch" => json!({
                "QueueUrl": url,
                "Entries": [{"Id": "only", "ReceiptHandle": handle, "VisibilityTimeout": 30}],
            }),
            "DeleteMessage" => json!({"QueueUrl": url, "ReceiptHandle": handle}),
            "DeleteMessageBatch" => json!({
                "QueueUrl": url,
                "Entries": [{"Id": "only", "ReceiptHandle": handle}],
            }),
            other => panic!("case {:?}: {other} is not one of the rows' operations", row.case),
        };
        let (status, body) = call(&router, row.operation, &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: {} must take a lapsed handle, got {status} with {body}",
            row.case, row.operation
        );
        let failed = body
            .get("Failed")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            failed.is_empty(),
            "case {:?}: no entry of the batch can be refused, got {failed:?}",
            row.case
        );

        let counts = message_counts(&router, &url).await;
        let expected = match row.left {
            Left::Hidden => (0, 1),
            Left::Gone => (0, 0),
        };
        assert_eq!(
            (counts.0, counts.2),
            expected,
            "case {:?}: expected (available, in flight) {expected:?}, got {counts:?} \
             (available, delayed, in flight)",
            row.case
        );
        let handed = messages(
            &ok(
                &router,
                "ReceiveMessage",
                json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
            )
            .await,
        )
        .len();
        assert_eq!(
            handed, 0,
            "case {:?}: the message the operation acted on is not handed over again, got \
             {handed} message(s)",
            row.case
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `DeleteMessageBatch` end to end over the wire (`POST /` with `X-Amz-Target:
//! AmazonSQS.DeleteMessageBatch`) through the same `MultiServiceRouter` the emulator's
//! front doors use.
//!
//! The RULES asserted here, each over a TABLE of inputs:
//!
//! * a batch that is not one AWS would act on is refused WHOLE and deletes nothing: no
//!   entries, more than ten entries, an entry `Id` outside the specification, two entries
//!   sharing an `Id`, and a `QueueUrl` no queue answers to;
//! * past those, each entry is resolved on its own: a live receipt handle deletes its
//!   message and is reported in `Successful`, a handle the queue does not hold is reported
//!   in `Failed` with `SenderFault` and the `ReceiptHandleIsInvalid` code, and the two
//!   outcomes may appear in the same answer — "the result of the action on each message is
//!   reported individually in the response";
//! * what the batch deleted is gone for good, which is read back through the queue's own
//!   counters rather than from the response.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{MAX_BATCH_ENTRIES, MAX_BATCH_ENTRY_ID_LEN};
use smithy_server_runtime::MultiServiceRouter;

use super::support;

/// The per-entry code for a receipt handle the queue does not hold, spelled out here
/// rather than imported: it is the name of the modelled error `DeleteMessage` answers for
/// the same condition.
const RECEIPT_HANDLE_IS_INVALID: &str = "ReceiptHandleIsInvalid";

/// Send `count` messages to the queue at `url`, bodies `m0..m<count>`.
async fn given_messages(router: &MultiServiceRouter, url: &str, count: usize) {
    for index in 0..count {
        let _ = support::ok(
            router,
            "SendMessage",
            json!({ "QueueUrl": url, "MessageBody": format!("m{index}") }),
        )
        .await;
    }
}

/// Receive up to `count` messages and answer their receipt handles.
async fn handles(router: &MultiServiceRouter, url: &str, count: usize) -> Vec<String> {
    let received = support::ok(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": count }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            message["ReceiptHandle"]
                .as_str()
                .unwrap_or_else(|| panic!("no ReceiptHandle in {message}"))
                .to_string()
        })
        .collect()
}

/// How many messages the queue at `url` still holds, visible and in flight.
async fn remaining(router: &MultiServiceRouter, url: &str) -> (i64, i64) {
    let attributes = support::all_attributes(router, url).await;
    let read = |name: &str| {
        attributes
            .get(name)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or_else(|| panic!("no {name} in {attributes:?}"))
    };
    (
        read("ApproximateNumberOfMessages"),
        read("ApproximateNumberOfMessagesNotVisible"),
    )
}

/// Every entry carrying a live handle is deleted and reported in `Successful`; the
/// messages are gone afterwards. Rows differ along the batch's SIZE, up to the documented
/// maximum.
#[tokio::test]
async fn a_batch_of_live_handles_deletes_every_message() {
    for size in [1usize, 2, 5, MAX_BATCH_ENTRIES] {
        let router = support::emulator();
        let url = support::create_ok(&router, &format!("batch-{size}"), &json!({})).await;
        given_messages(&router, &url, size).await;
        let receipts = handles(&router, &url, size).await;
        assert_eq!(
            receipts.len(),
            size,
            "case size {size}: expected {size} handle(s) to delete, got {receipts:?}"
        );

        let entries: Vec<Value> = receipts
            .iter()
            .enumerate()
            .map(|(index, handle)| json!({ "Id": format!("e{index}"), "ReceiptHandle": handle }))
            .collect();
        let answer = support::ok(
            &router,
            "DeleteMessageBatch",
            json!({ "QueueUrl": url, "Entries": entries }),
        )
        .await;

        let mut deleted: Vec<String> = answer["Successful"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        deleted.sort();
        let mut expected: Vec<String> = (0..size).map(|index| format!("e{index}")).collect();
        expected.sort();
        assert_eq!(
            deleted, expected,
            "case size {size}: expected every entry reported successful, got {answer}"
        );
        assert_eq!(
            answer["Failed"].as_array().map(Vec::len).unwrap_or(0),
            0,
            "case size {size}: expected no failed entry, got {answer}"
        );
        assert_eq!(
            remaining(&router, &url).await,
            (0, 0),
            "case size {size}: every deleted message must be gone"
        );
    }
}

/// One bad handle fails ALONE: the live entries of the same batch are still deleted, and
/// the bad one is reported in `Failed` with the sender's fault. Rows differ along WHY a
/// handle is not one the queue holds, and along where the bad entry sits in the batch.
#[tokio::test]
async fn a_bad_handle_fails_on_its_own_entry() {
    let router = support::emulator();
    let url = support::create_ok(&router, "mixed-batch", &json!({})).await;
    given_messages(&router, &url, 3).await;
    let receipts = handles(&router, &url, 3).await;

    // A handle that was never minted, and an empty one: neither names a live message.
    let never_minted = "00000000-0000-4000-8000-000000000000".to_string();

    let entries = json!([
        { "Id": "bad-first", "ReceiptHandle": never_minted },
        { "Id": "good", "ReceiptHandle": receipts[0] },
        { "Id": "bad-last", "ReceiptHandle": "" },
    ]);
    let answer = support::ok(
        &router,
        "DeleteMessageBatch",
        json!({ "QueueUrl": url, "Entries": entries }),
    )
    .await;

    let successful: Vec<String> = answer["Successful"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        successful,
        vec!["good".to_string()],
        "expected only the live handle deleted, got {answer}"
    );
    let failed = answer["Failed"].as_array().cloned().unwrap_or_default();
    let mut failed_ids: Vec<String> = failed
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    failed_ids.sort();
    assert_eq!(
        failed_ids,
        vec!["bad-first".to_string(), "bad-last".to_string()],
        "expected both bad handles reported per entry, got {answer}"
    );
    for entry in &failed {
        assert_eq!(
            entry["Code"],
            json!(RECEIPT_HANDLE_IS_INVALID),
            "expected the {RECEIPT_HANDLE_IS_INVALID} code, got {entry}"
        );
        assert_eq!(
            entry["SenderFault"],
            json!(true),
            "a caller-supplied handle is the sender's fault, got {entry}"
        );
        assert!(
            entry["Message"].as_str().is_some_and(|text| !text.is_empty()),
            "expected a message saying why the entry failed, got {entry}"
        );
    }
}

/// A batch AWS would not act on is refused whole, with the error for WHY, and deletes
/// nothing. Rows differ along what makes the batch unacceptable.
#[tokio::test]
async fn a_batch_that_is_not_acceptable_is_refused_whole() {
    // One row per refusal: a label, the entries (built from the live handles), whether
    // the queue url is the real one, and the expected status and wire code.
    /// Builds a row's `Entries` from the queue's live receipt handles.
    type Entries = Box<dyn Fn(&[String]) -> Value>;
    struct Case {
        label: &'static str,
        entries: Entries,
        real_queue: bool,
        status: u16,
        code: &'static str,
    }
    let long_id = "i".repeat(MAX_BATCH_ENTRY_ID_LEN + 1);
    let cases = vec![
        Case {
            label: "no entries at all",
            entries: Box::new(|_| json!([])),
            real_queue: true,
            status: 400,
            code: "EmptyBatchRequest",
        },
        Case {
            label: "one entry more than the documented maximum",
            entries: Box::new(|handles| {
                Value::Array(
                    (0..=MAX_BATCH_ENTRIES)
                        .map(|index| {
                            json!({
                                "Id": format!("e{index}"),
                                "ReceiptHandle": handles.first().cloned().unwrap_or_default(),
                            })
                        })
                        .collect(),
                )
            }),
            real_queue: true,
            status: 400,
            code: "TooManyEntriesInBatchRequest",
        },
        Case {
            label: "an entry id carrying a character the specification excludes",
            entries: Box::new(|handles| {
                json!([{ "Id": "not ok!", "ReceiptHandle": handles[0] }])
            }),
            real_queue: true,
            status: 400,
            code: "InvalidBatchEntryId",
        },
        Case {
            label: "an entry id past the documented length",
            entries: Box::new(move |handles| {
                json!([{ "Id": long_id, "ReceiptHandle": handles[0] }])
            }),
            real_queue: true,
            status: 400,
            code: "InvalidBatchEntryId",
        },
        Case {
            label: "an empty entry id",
            entries: Box::new(|handles| json!([{ "Id": "", "ReceiptHandle": handles[0] }])),
            real_queue: true,
            status: 400,
            code: "InvalidBatchEntryId",
        },
        Case {
            label: "two entries sharing an id",
            entries: Box::new(|handles| {
                json!([
                    { "Id": "same", "ReceiptHandle": handles[0] },
                    { "Id": "same", "ReceiptHandle": handles[1] },
                ])
            }),
            real_queue: true,
            status: 400,
            code: "BatchEntryIdsNotDistinct",
        },
        Case {
            label: "a queue url no queue answers to",
            entries: Box::new(|handles| json!([{ "Id": "e0", "ReceiptHandle": handles[0] }])),
            real_queue: false,
            status: 400,
            code: "QueueDoesNotExist",
        },
    ];

    for case in cases {
        let router = support::emulator();
        let url = support::create_ok(&router, "refused-batch", &json!({})).await;
        given_messages(&router, &url, 2).await;
        let receipts = handles(&router, &url, 2).await;
        let before = remaining(&router, &url).await;

        let queue_url = if case.real_queue {
            url.clone()
        } else {
            support::expected_url("no-such-queue")
        };
        let (status, body) = support::call(
            &router,
            "DeleteMessageBatch",
            &json!({ "QueueUrl": queue_url, "Entries": (case.entries)(&receipts) }),
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
            remaining(&router, &url).await,
            before,
            "case {}: a refused batch must delete nothing",
            case.label
        );
    }
}

/// The published bound on an entry id is a BOUNDARY: an id of exactly the documented
/// length is accepted, and one character more is not (asserted above). An id made only of
/// the accepted characters is accepted too.
#[tokio::test]
async fn an_entry_id_at_the_documented_bound_is_accepted() {
    let rows = [
        ("the maximum length", "i".repeat(MAX_BATCH_ENTRY_ID_LEN)),
        ("one character", "x".to_string()),
        ("hyphens and underscores", "a-b_c-9".to_string()),
        ("digits only", "0123456789".to_string()),
    ];
    for (case, id) in rows {
        let router = support::emulator();
        let url = support::create_ok(&router, "bound-batch", &json!({})).await;
        given_messages(&router, &url, 1).await;
        let receipts = handles(&router, &url, 1).await;
        let answer = support::ok(
            &router,
            "DeleteMessageBatch",
            json!({
                "QueueUrl": url,
                "Entries": [{ "Id": id, "ReceiptHandle": receipts[0] }],
            }),
        )
        .await;
        assert_eq!(
            answer["Successful"][0]["Id"],
            json!(id),
            "case {case}: expected the entry reported successful under its own id, got {answer}"
        );
        assert_eq!(
            remaining(&router, &url).await,
            (0, 0),
            "case {case}: the deleted message must be gone"
        );
    }
}

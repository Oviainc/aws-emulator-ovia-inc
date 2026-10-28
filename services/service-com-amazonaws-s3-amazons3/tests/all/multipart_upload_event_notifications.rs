// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EVENT NOTIFICATIONS the multipart upload loop owes: what a
//! bucket's notification configuration delivers when an upload is COMPLETED, and what it
//! delivers when an upload is ABORTED.
//!
//! `PutBucketNotificationConfiguration` records "the event types that you want Amazon S3
//! to publish and the destination where you want Amazon S3 to publish an event
//! notification when it detects an event of the specified type"
//! (`PutBucketNotificationConfiguration`'s own documentation). The rule under test is the
//! one [`super::object_event_notifications`] pins for a one-shot write, over the multipart
//! loop:
//!
//! * the call that CREATES an object publishes exactly one event record naming that
//!   object, to every destination whose `Events` list names the created event — literally
//!   or through the `s3:<Type>:*` form, which stands for every event of that type — and
//!   whose key-name `Filter` the key passes; a multipart upload's completion is such a
//!   creation, and its event value is `s3:ObjectCreated:CompleteMultipartUpload`, which is
//!   what the record then names as `ObjectCreated:CompleteMultipartUpload`;
//! * a call that creates NO object publishes nothing, so discarding an upload
//!   (`AbortMultipartUpload`: "After a multipart upload is aborted, no additional parts can
//!   be uploaded using that upload ID") delivers no record at all — not even to a
//!   configuration that asked for every `s3:ObjectCreated` event.
//!
//! Both halves are asserted on the SAME queue in
//! [`an_abort_publishes_nothing_on_a_queue_a_completion_reaches`], because "nothing was
//! delivered for the abort" is only worth asserting on a queue a completion demonstrably
//! reaches: an emitter that publishes nothing at all would otherwise satisfy the negative
//! half for the wrong reason.
//!
//! Every assertion drives real HTTP requests through a router carrying S3 beside SQS, so
//! what is pinned is the message a consumer receives rather than any handler's internals.
//! The table's rows differ along what the rule generalises over: how the configuration
//! names the event (the completion value itself, the type wildcard, a list naming another
//! detail of the type, a list naming another type), the key-name filter (two rules the key
//! passes, one it fails, none), the form of the key (a bare name, a nested prefix, a name
//! carrying a space), how many parts the upload is assembled from (one, two, three), and
//! which call ends the upload (a completion, an abort).

use super::multipart_upload_abort::element_of;
use super::multipart_upload_completion::{completion_payload, expected_multipart_etag};
use super::object_event_notifications::{
    queue_arn, request_key, Destination, Emulator, CONFIGURATION_ID, REGION,
};
use super::object_partial_reads::seeded_payload;

/// The fewest bytes a part other than the last one may carry: "Part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3
/// User Guide, *Amazon S3 multipart upload limits*).
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// The `eventName` a record of a completed multipart upload carries: the `Event` value
/// `s3:ObjectCreated:CompleteMultipartUpload` without its `s3:` prefix, which is how a
/// record names an event (Amazon S3 User Guide, *Event message structure*).
const COMPLETION_EVENT_NAME: &str = "ObjectCreated:CompleteMultipartUpload";

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, answering the upload id the
/// store minted.
async fn initiate(emulator: &Emulator, bucket: &str, key: &str) -> String {
    let initiated = emulator
        .send(
            "s3",
            "POST",
            &format!("/{bucket}/{}?uploads", request_key(key)),
            &[("content-type", "application/octet-stream")],
            Vec::new(),
        )
        .await;
    assert_eq!(
        initiated.status,
        200,
        "initiating an upload of {key:?} must succeed, got {} with {}",
        initiated.status,
        initiated.text()
    );
    element_of(&initiated.text(), "UploadId").unwrap_or_else(|| {
        panic!(
            "an initiation must report an UploadId, got {}",
            initiated.text()
        )
    })
}

/// `PUT /<bucket>/<key>?partNumber=<n>&uploadId=<id>` — `UploadPart`, answering the entity
/// tag the part was recorded with, which the completion then names it by.
async fn upload_part(
    emulator: &Emulator,
    bucket: &str,
    key: &str,
    upload_id: &str,
    part_number: i32,
    part: &[u8],
) -> String {
    let uploaded = emulator
        .send(
            "s3",
            "PUT",
            &format!(
                "/{bucket}/{}?partNumber={part_number}&uploadId={upload_id}",
                request_key(key)
            ),
            &[("content-type", "application/octet-stream")],
            part.to_vec(),
        )
        .await;
    assert_eq!(
        uploaded.status,
        200,
        "uploading part {part_number} of {key:?} must succeed, got {} with {}",
        uploaded.status,
        uploaded.text()
    );
    uploaded
        .header("etag")
        .expect("a part upload must report an ETag header")
}

/// `POST /<bucket>/<key>?uploadId=<id>` — `CompleteMultipartUpload`, with the parts list as
/// its payload (built by the completion suite's own
/// [`super::multipart_upload_completion::completion_payload`]).
async fn complete(
    emulator: &Emulator,
    bucket: &str,
    key: &str,
    upload_id: &str,
    parts: &[(i32, String)],
) {
    let completed = emulator
        .s3_xml(
            "POST",
            &format!("/{bucket}/{}?uploadId={upload_id}", request_key(key)),
            &completion_payload(parts),
        )
        .await;
    assert_eq!(
        completed.status,
        200,
        "completing the upload of {key:?} must succeed, got {} with {}",
        completed.status,
        completed.text()
    );
}

/// `DELETE /<bucket>/<key>?uploadId=<id>&x-id=AbortMultipartUpload` —
/// `AbortMultipartUpload`, which the model binds to the 204 asserted here
/// (`smithy_query(query="operation",operation="AbortMultipartUpload")` reports
/// `http {method: DELETE, code: 204}`). The `x-id` literal is part of that binding and is
/// what distinguishes the call from a delete of the object at the same address.
async fn abort(emulator: &Emulator, bucket: &str, key: &str, upload_id: &str) {
    let aborted = emulator
        .s3_xml(
            "DELETE",
            &format!(
                "/{bucket}/{}?uploadId={upload_id}&x-id=AbortMultipartUpload",
                request_key(key)
            ),
            "",
        )
        .await;
    assert_eq!(
        aborted.status,
        204,
        "aborting the upload of {key:?} must succeed, got {} with {}",
        aborted.status,
        aborted.text()
    );
}

/// Initiate an upload at `key`, upload `parts` in ascending part-number order, and end it
/// with `ending`.
async fn drive_upload(
    emulator: &Emulator,
    bucket: &str,
    key: &str,
    parts: &[Vec<u8>],
    ending: Ending,
) {
    let upload_id = initiate(emulator, bucket, key).await;
    let mut uploaded = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let part_number = i32::try_from(index + 1).expect("a row names few enough parts");
        let etag = upload_part(emulator, bucket, key, &upload_id, part_number, part).await;
        uploaded.push((part_number, etag));
    }
    match ending {
        Ending::Complete => complete(emulator, bucket, key, &upload_id, &uploaded).await,
        Ending::Abort => abort(emulator, bucket, key, &upload_id).await,
    }
}

/// Which call ends the upload a row drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// `CompleteMultipartUpload` — the call that creates the object.
    Complete,
    /// `AbortMultipartUpload` — the call that discards the upload and creates nothing.
    Abort,
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The `Events` the queue configuration names.
    configured_events: &'static [&'static str],
    /// The `Filter`'s key-name rules, as `(Name, Value)` pairs.
    filter_rules: &'static [(&'static str, &'static str)],
    /// The key the upload is initiated at.
    key: &'static str,
    /// The key as a delivered RECORD spells it — "URL encoded" (Amazon S3 User Guide,
    /// *Event message structure*).
    published_key: &'static str,
    /// The parts the upload carries, in ascending part-number order.
    parts: Vec<Vec<u8>>,
    ending: Ending,
    /// The `eventName` the configured queue must be delivered, or `None` when this upload
    /// owes it no record at all.
    expected_event_name: Option<&'static str>,
}

/// The rule, over the class it generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a completion named by its own event value, assembled from one part",
            configured_events: &["s3:ObjectCreated:CompleteMultipartUpload"],
            filter_rules: &[],
            key: "assembled.bin",
            published_key: "assembled.bin",
            parts: vec![seeded_payload(3, 64)],
            ending: Ending::Complete,
            expected_event_name: Some(COMPLETION_EVENT_NAME),
        },
        Row {
            case: "a completion named by the type wildcard, assembled from two parts",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[],
            key: "input/2026/assembled.bin",
            published_key: "input/2026/assembled.bin",
            parts: vec![
                seeded_payload(5, MIN_MULTIPART_PART_BYTES),
                seeded_payload(7, 128),
            ],
            ending: Ending::Complete,
            expected_event_name: Some(COMPLETION_EVENT_NAME),
        },
        Row {
            case: "a completion whose key passes both filter rules, assembled from three parts",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[("prefix", "input/"), ("suffix", ".bin")],
            key: "input/deep/assembled.bin",
            published_key: "input/deep/assembled.bin",
            parts: vec![
                seeded_payload(11, MIN_MULTIPART_PART_BYTES),
                seeded_payload(13, MIN_MULTIPART_PART_BYTES),
                seeded_payload(17, 9),
            ],
            ending: Ending::Complete,
            expected_event_name: Some(COMPLETION_EVENT_NAME),
        },
        Row {
            case: "a completion whose key fails the suffix rule",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[("prefix", "input/"), ("suffix", ".bin")],
            key: "input/assembled.csv",
            published_key: "input/assembled.csv",
            parts: vec![seeded_payload(19, 48)],
            ending: Ending::Complete,
            expected_event_name: None,
        },
        Row {
            case: "a completion whose configuration names one-shot writes only",
            configured_events: &["s3:ObjectCreated:Put"],
            filter_rules: &[],
            key: "assembled.bin",
            published_key: "assembled.bin",
            parts: vec![seeded_payload(23, 48)],
            ending: Ending::Complete,
            expected_event_name: None,
        },
        Row {
            case: "a completion whose configuration names removals only",
            configured_events: &["s3:ObjectRemoved:*"],
            filter_rules: &[],
            key: "assembled.bin",
            published_key: "assembled.bin",
            parts: vec![seeded_payload(29, 48)],
            ending: Ending::Complete,
            expected_event_name: None,
        },
        Row {
            case: "a completion whose key carries a space",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[],
            key: "red flower.bin",
            published_key: "red+flower.bin",
            parts: vec![seeded_payload(31, 72)],
            ending: Ending::Complete,
            expected_event_name: Some(COMPLETION_EVENT_NAME),
        },
        Row {
            case: "an abort of an upload the type wildcard covers",
            configured_events: &["s3:ObjectCreated:*"],
            filter_rules: &[],
            key: "input/discarded.bin",
            published_key: "input/discarded.bin",
            parts: vec![seeded_payload(37, 48)],
            ending: Ending::Abort,
            expected_event_name: None,
        },
        Row {
            case: "an abort of a two-part upload whose configuration names the completion value",
            configured_events: &["s3:ObjectCreated:CompleteMultipartUpload"],
            filter_rules: &[],
            key: "discarded.bin",
            published_key: "discarded.bin",
            parts: vec![
                seeded_payload(41, MIN_MULTIPART_PART_BYTES),
                seeded_payload(43, 32),
            ],
            ending: Ending::Abort,
            expected_event_name: None,
        },
    ]
}

#[tokio::test]
async fn a_multipart_upload_notifies_its_bucket_only_when_it_creates_an_object() {
    let emulator = Emulator::new();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let bucket = format!("multipart-notify-{index}");
        let queue = format!("multipart-notify-queue-{index}");

        emulator.create_bucket(&bucket).await;
        let queue_url = emulator.create_queue(&queue).await;
        emulator
            .put_notification(
                &bucket,
                &Destination::queue(&queue_arn(&queue)),
                row.configured_events,
                row.filter_rules,
                false,
            )
            .await;

        drive_upload(&emulator, &bucket, row.key, &row.parts, row.ending).await;

        let Some(event_name) = row.expected_event_name else {
            let delivered = emulator.nothing_delivered(&queue_url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: this upload owes the configured queue no record, so the \
                 queue must stay empty, got {delivered:?}"
            );
            continue;
        };

        let message = emulator.delivered(&queue_url).await.unwrap_or_else(|| {
            panic!("case {case:?}: no notification reached the configured queue")
        });
        let records = message["Records"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected exactly one record in the envelope, got {message}"
        );
        let record = &records[0];
        assert_eq!(
            record["eventName"].as_str(),
            Some(event_name),
            "case {case:?}: expected eventName {event_name:?}, got {message}"
        );
        assert_eq!(
            record["eventSource"].as_str(),
            Some("aws:s3"),
            "case {case:?}: expected eventSource aws:s3, got {message}"
        );
        assert_eq!(
            record["awsRegion"].as_str(),
            Some(REGION),
            "case {case:?}: expected awsRegion {REGION:?}, got {message}"
        );
        assert_eq!(
            record["s3"]["configurationId"].as_str(),
            Some(CONFIGURATION_ID),
            "case {case:?}: expected configurationId {CONFIGURATION_ID:?}, got {message}"
        );
        assert_eq!(
            record["s3"]["bucket"]["name"].as_str(),
            Some(bucket.as_str()),
            "case {case:?}: expected the record's bucket to be {bucket:?}, got {message}"
        );
        assert_eq!(
            record["s3"]["bucket"]["arn"].as_str(),
            Some(format!("arn:aws:s3:::{bucket}").as_str()),
            "case {case:?}: expected the record's bucket ARN, got {message}"
        );
        assert_eq!(
            record["s3"]["object"]["key"].as_str(),
            Some(row.published_key),
            "case {case:?}: expected the record's key {:?}, got {message}",
            row.published_key
        );
        assert_eq!(
            record["s3"]["object"]["size"].as_i64(),
            Some(row.parts.concat().len() as i64),
            "case {case:?}: expected the assembled object's size, got {message}"
        );
        // The record reports the stored object's entity tag, which for an object a
        // multipart upload assembled is the digest over its parts' digests with the part
        // count appended, not the digest of its bytes. Compared as the tag's VALUE, so
        // what this row pins is the object the record names rather than the quoting the
        // envelope carries it with.
        let expected_etag = expected_multipart_etag(&row.parts);
        assert_eq!(
            record["s3"]["object"]["eTag"]
                .as_str()
                .map(|etag| etag.trim_matches('"')),
            Some(expected_etag.trim_matches('"')),
            "case {case:?}: expected the assembled object's multipart entity tag \
             {expected_etag:?}, got {message}"
        );
        assert!(
            record["s3"]["object"]["sequencer"]
                .as_str()
                .is_some_and(|sequencer| !sequencer.is_empty()),
            "case {case:?}: expected a sequencer on the record, got {message}"
        );

        // Exactly one: the fan-out this upload owed is the record just read, and no second
        // delivery follows it.
        let rest = emulator.nothing_delivered(&queue_url).await;
        assert!(
            rest.is_empty(),
            "case {case:?}: one creation owes one notification, got {} more: {rest:?}",
            rest.len()
        );
    }
}

#[tokio::test]
async fn an_abort_publishes_nothing_on_a_queue_a_completion_reaches() {
    let emulator = Emulator::new();
    let bucket = "multipart-notify-abort-contract";
    let queue = "multipart-notify-abort-contract-queue";
    let aborted_key = "uploads/discarded.bin";
    let completed_key = "uploads/assembled.bin";
    // One part each, since what this test is about is which CALL publishes rather than how
    // many parts were assembled (the table above spans the part count): a single part is the
    // last part, which no minimum part size applies to.
    let parts = vec![seeded_payload(47, 96)];

    emulator.create_bucket(bucket).await;
    let queue_url = emulator.create_queue(queue).await;
    emulator
        .put_notification(
            bucket,
            &Destination::queue(&queue_arn(queue)),
            &["s3:ObjectCreated:*"],
            &[],
            false,
        )
        .await;

    // The negative half: an upload that is discarded created no object, so it owes no
    // record.
    drive_upload(&emulator, bucket, aborted_key, &parts, Ending::Abort).await;
    let after_abort = emulator.nothing_delivered(&queue_url).await;
    assert!(
        after_abort.is_empty(),
        "case \"an aborted upload\": an abort creates no object, so the queue must stay \
         empty, got {after_abort:?}"
    );

    // The positive half, on the SAME queue and the same configuration: a completion
    // reaches it, which is what makes the quiet above a statement about the abort.
    drive_upload(&emulator, bucket, completed_key, &parts, Ending::Complete).await;
    let message = emulator.delivered(&queue_url).await.unwrap_or_else(|| {
        panic!(
            "case \"a completed upload\": no notification reached the queue the aborted \
             upload was quiet on"
        )
    });
    let record = &message["Records"][0];
    assert_eq!(
        record["eventName"].as_str(),
        Some(COMPLETION_EVENT_NAME),
        "case \"a completed upload\": expected eventName {COMPLETION_EVENT_NAME:?}, got \
         {message}"
    );
    assert_eq!(
        record["s3"]["object"]["key"].as_str(),
        Some(completed_key),
        "case \"a completed upload\": expected the record's key {completed_key:?}, got \
         {message}"
    );

    // The record names the object the store really holds: the key it reports reads back,
    // carrying the very entity tag the record carries. Compared as the tag's VALUE, the
    // same way the table above compares it, so neither assertion pins the quoting an
    // envelope or a bound header wraps the tag in.
    let head = emulator
        .send(
            "s3",
            "HEAD",
            &format!("/{bucket}/{completed_key}"),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(
        head.status,
        200,
        "case \"a completed upload\": the key the record names must read back, got {} with \
         {}",
        head.status,
        head.text()
    );
    let stored_etag = head
        .header("etag")
        .expect("a read of the assembled object must report an ETag header");
    assert_eq!(
        record["s3"]["object"]["eTag"]
            .as_str()
            .map(|etag| etag.trim_matches('"')),
        Some(stored_etag.trim_matches('"')),
        "case \"a completed upload\": expected the stored object's entity tag \
         {stored_etag:?} on the record, got {message}"
    );

    // And nothing else: the abort's record does not arrive late, so the only delivery the
    // whole exchange owed is the completion's.
    let rest = emulator.nothing_delivered(&queue_url).await;
    assert!(
        rest.is_empty(),
        "case \"an aborted upload\": only the completion owed a notification, got {} more: \
         {rest:?}",
        rest.len()
    );
}

// SPDX-License-Identifier: CC0-1.0

//! The BYTE FIDELITY of a subscription filter's delivery out of a CMK-encrypted log group,
//! end to end over the wire with the Kinesis emulator mounted beside this service and read
//! back where a consumer reads it (`GetShardIterator` TRIM_HORIZON -> `GetRecords`).
//!
//! RULE: a log event a `PutLogEvents` batch stores and the group's subscription filter
//! selects is handed to the filter's destination as a GZIP FRAME whose JSON envelope carries
//! that event's `message` exactly as it was accepted, byte for byte and in ingestion order
//! ("you can subscribe to a real-time stream of log events ingested through PutLogEvents and
//! have them delivered to a specific destination [...] When log events are sent to the
//! receiving service, they are Base64 encoded and compressed with the GZIP format",
//! `PutSubscriptionFilter`), and the key named by the group's `kmsKeyId` — which encrypts the
//! group's OWN stored log data (`AssociateKmsKey`) and nothing else — neither re-encodes the
//! payload nor withholds the delivery: each successive batch owes its own record, so a
//! consumer polling the destination sees one frame per batch in the order the batches were
//! ingested, and the group reports the same `kmsKeyId` afterwards.
//!
//! The suites beside this one grade other halves of the same rule: `subscription_fan_out.rs`
//! grades the envelope's SHAPE across the filter-pattern forms, `cmk_group_fan_out_isolation.rs`
//! grades that the group's key does not leak into the destination or into the delivered
//! members, and `subscription_filter_delivery.rs` grades WHICH filters a batch is fanned out
//! through. None of them varies the MESSAGE BYTES or reads the frame before it is
//! decompressed, which is what the recorded flow this suite was derived from asserts: a
//! marker written into a CMK-encrypted group is readable as plaintext in the decompressed
//! bytes of the record the destination stream serves, and the record's data begins with the
//! gzip magic rather than with a ciphertext blob.
//!
//! The recorded flow writes successive batches of uniquely marked ASCII messages into one
//! CMK-encrypted group and polls the destination until its marker appears; that is one row of
//! the table below. The rows differ along what the rule generalises over — the BYTES a
//! message is made of (ASCII, multi-byte UTF-8 up to four bytes per character, characters
//! JSON must escape, embedded control whitespace, the one-character minimum
//! `InputLogEvent$message` admits, and a message tens of kilobytes long) and how many events
//! one batch carries.
//!
//! Every case drives the real wire path (`POST /` with `X-Amz-Target:
//! <Service>.<Operation>` through `MultiServiceRouter::dispatch`), so what is asserted here
//! is what a client sees.

use serde_json::{json, Value};

use crate::harness::{base_ms, REGION};
use crate::subscription_fan_out::{
    assert_envelope, gunzip, past_activation, role_arn, stream_arn, Emulator, BY_LOG_STREAM,
};

/// The two fixed bytes every gzip member begins with (`ID1`, `ID2`). Source: RFC 1952,
/// the gzip file format `PutSubscriptionFilter` names ("compressed with the GZIP format").
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// The log group, log stream and filter every batch below is written through: one group, so
/// the rows accumulate on one destination shard the way a polling consumer reads them.
const GROUP: &str = "/aws/emulator/cmk-byte-fidelity";
const LOG_STREAM: &str = "app";
const FILTER: &str = "to-kinesis";

/// The destination stream the filter delivers to.
const STREAM: &str = "cmk-byte-fidelity";

/// How long the long-message row's message is: comfortably past one `PutLogEvents` batch's
/// worth of small events and far enough under the per-record payload quota that the batch is
/// still one record, so what it grades is the bytes surviving the frame rather than the split.
const LONG_MESSAGE_BYTES: usize = 64 * 1024;

/// A customer managed key ARN, the form `CreateLogGroup$kmsKeyId` documents ("The Amazon
/// Resource Name (ARN) of the KMS key to use when encrypting log data").
fn cmk_key_arn() -> String {
    engine::emulator_arn("kms", REGION, "key/7c1d9f40-2a6b-4f58-8f0c-3d6e2b4a9c71")
}

/// One row: a single `PutLogEvents` batch, named by what its message bytes are made of.
struct Row {
    /// What the case is called in every assertion it makes.
    case: &'static str,
    /// The messages the batch carries, in the order it carries them. The filter matches
    /// everything, so all of them are delivered in one envelope.
    messages: Vec<String>,
    /// Whether each message's own bytes appear VERBATIM in the decompressed frame — true
    /// wherever JSON does not have to escape them, which is the strongest reading of
    /// "delivered as plaintext": the consumer finds the marker it wrote without unescaping
    /// anything.
    literal_in_frame: bool,
}

/// The table: one row per form of message bytes the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: plain ASCII markers.
            case: "ascii markers",
            messages: vec!["marker-alpha ERROR disk full".to_string()],
            literal_in_frame: true,
        },
        Row {
            // Two-, three- and four-byte UTF-8 characters, none of which JSON escapes.
            case: "multi-byte utf-8 markers",
            messages: vec!["marker-β ΔΩ 障害発生 🚀 ошибка".to_string()],
            literal_in_frame: true,
        },
        Row {
            case: "characters json must escape",
            messages: vec![r#"marker-gamma {"path":"c:\logs\app.log","q":"\"quoted\""}"#.to_string()],
            literal_in_frame: false,
        },
        Row {
            case: "embedded control whitespace",
            messages: vec!["marker-delta line one\nline two\tcolumn".to_string()],
            literal_in_frame: false,
        },
        Row {
            // The minimum `InputLogEvent$message` admits (len 1..).
            case: "the one-character minimum",
            messages: vec!["x".to_string()],
            literal_in_frame: true,
        },
        Row {
            case: "several events in one batch",
            messages: vec![
                "marker-epsilon first".to_string(),
                "marker-épsilon second".to_string(),
                "marker-epsilon third".to_string(),
            ],
            literal_in_frame: true,
        },
        Row {
            case: "a message tens of kilobytes long",
            messages: vec![format!(
                "marker-zeta {}",
                "0123456789abcdef".repeat(LONG_MESSAGE_BYTES / 16)
            )],
            literal_in_frame: true,
        },
    ]
}

/// The `kmsKeyId` CloudWatch Logs reports for [`GROUP`], or `None` when it reports none.
async fn group_key(emulator: &Emulator, case: &str) -> Option<String> {
    let page = emulator
        .logs(
            case,
            "DescribeLogGroups",
            &json!({"logGroupNamePrefix": GROUP}),
        )
        .await;
    page["logGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|described| described["logGroupName"].as_str() == Some(GROUP))
        .unwrap_or_else(|| panic!("case {case}: the log group must still exist, got {page}"))
        .get("kmsKeyId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The `message` of every event one envelope carries, in the order the envelope carries them.
fn envelope_messages(case: &str, envelope: &Value) -> Vec<String> {
    envelope["logEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: an envelope carries logEvents, got {envelope}"))
        .iter()
        .map(|event| {
            event["message"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a delivered event carries a message, got {event}")
                })
                .to_string()
        })
        .collect()
}

/// Whether `haystack` holds `needle` as a contiguous run of bytes.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|window| window == needle)
}

/// The table: successive batches written into a CMK-encrypted log group reach the filter's
/// Kinesis destination as one gzip frame each, in ingestion order, each carrying its batch's
/// messages byte for byte.
#[tokio::test]
async fn a_cmk_encrypted_groups_filter_delivers_each_batch_verbatim_in_order() {
    let emulator = Emulator::new();
    let rows = rows();
    let key = cmk_key_arn();

    emulator.create_stream("the destination stream", STREAM).await;
    tokio::time::sleep(past_activation()).await;

    emulator
        .logs(
            "the encrypted group",
            "CreateLogGroup",
            &json!({"logGroupName": GROUP, "kmsKeyId": key}),
        )
        .await;
    emulator
        .logs(
            "the encrypted group",
            "CreateLogStream",
            &json!({"logGroupName": GROUP, "logStreamName": LOG_STREAM}),
        )
        .await;
    let key_before = group_key(&emulator, "the encrypted group").await;
    assert_eq!(
        key_before.as_deref(),
        Some(key.as_str()),
        "case {:?}: the group reports the key it was created with, got {key_before:?}",
        "the encrypted group"
    );

    emulator
        .logs(
            "the filter",
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": GROUP,
                "filterName": FILTER,
                "filterPattern": "",
                "destinationArn": stream_arn(STREAM),
                "roleArn": role_arn(),
                "distribution": BY_LOG_STREAM,
            }),
        )
        .await;

    // One batch per row, written in row order and stamped with strictly increasing
    // timestamps, so the order the destination serves is the order of ingestion.
    let base = base_ms();
    let mut next_offset: i64 = 0;
    let mut stamped: Vec<Vec<i64>> = Vec::with_capacity(rows.len());
    for row in &rows {
        let timestamps: Vec<i64> = row
            .messages
            .iter()
            .map(|_| {
                let timestamp = base + next_offset;
                next_offset += 1;
                timestamp
            })
            .collect();
        let events: Vec<Value> = row
            .messages
            .iter()
            .zip(&timestamps)
            .map(|(message, timestamp)| json!({"timestamp": timestamp, "message": message}))
            .collect();
        let answer = emulator
            .logs(
                row.case,
                "PutLogEvents",
                &json!({
                    "logGroupName": GROUP,
                    "logStreamName": LOG_STREAM,
                    "logEvents": events,
                }),
            )
            .await;
        assert!(
            answer.get("rejectedLogEventsInfo").is_none(),
            "case {}: an accepted batch reports no rejectedLogEventsInfo, got {answer}",
            row.case
        );
        stamped.push(timestamps);
    }

    // One read of the whole shard from its oldest retained record: one frame per batch, in
    // the order the batches were ingested.
    let frames = emulator.delivered_frames("the delivered frames", STREAM).await;
    assert_eq!(
        frames.len(),
        rows.len(),
        "case {:?}: each batch owes its own record, expected {} records, got {}",
        "the delivered frames",
        rows.len(),
        frames.len()
    );

    for ((row, timestamps), (partition_key, framed)) in
        rows.iter().zip(&stamped).zip(frames.iter())
    {
        let case = row.case;

        // The record's own bytes are a gzip frame, not a ciphertext blob and not an
        // envelope wrapping a key.
        assert!(
            framed.starts_with(&GZIP_MAGIC),
            "case {case}: a delivered record's data begins with the gzip magic \
             {GZIP_MAGIC:?}, got {:?}",
            &framed[..framed.len().min(GZIP_MAGIC.len())]
        );
        assert_eq!(
            partition_key, LOG_STREAM,
            "case {case}: log data grouped by log stream is keyed on the log stream, got \
             {partition_key}"
        );

        let plain = gunzip(case, framed);
        let envelope: Value = serde_json::from_slice(&plain)
            .unwrap_or_else(|error| panic!("case {case}: the frame holds JSON, got {error}"));
        let expected: Vec<&str> = row.messages.iter().map(String::as_str).collect();
        assert_envelope(
            case,
            &envelope,
            GROUP,
            LOG_STREAM,
            FILTER,
            &expected,
            timestamps,
        );

        // Byte for byte and in order: the messages the envelope carries ARE the bytes
        // `PutLogEvents` accepted.
        let delivered = envelope_messages(case, &envelope);
        let delivered_bytes: Vec<&[u8]> = delivered.iter().map(|m| m.as_bytes()).collect();
        let accepted_bytes: Vec<&[u8]> = row.messages.iter().map(|m| m.as_bytes()).collect();
        assert_eq!(
            delivered_bytes, accepted_bytes,
            "case {case}: the delivered messages are the accepted bytes in order, got \
             {delivered:?}"
        );

        // Where JSON escapes nothing, each message's bytes are findable VERBATIM in the
        // decompressed frame: the marker is plaintext to the consumer.
        if row.literal_in_frame {
            for message in &row.messages {
                assert!(
                    contains_bytes(&plain, message.as_bytes()),
                    "case {case}: the decompressed frame carries {message:?} verbatim, got a \
                     {}-byte frame",
                    plain.len()
                );
            }
        }

        // And no part of the group's key travels with it.
        assert!(
            !contains_bytes(&plain, key.as_bytes()),
            "case {case}: the group's key is not part of what is delivered, got a {}-byte \
             frame",
            plain.len()
        );
    }

    // The group's own encryption is where it was: delivering and reading changed nothing.
    let key_after = group_key(&emulator, "after the deliveries").await;
    assert_eq!(
        key_after, key_before,
        "case {:?}: the group still reports the kmsKeyId it held before the deliveries, got \
         {key_after:?}",
        "after the deliveries"
    );

    // What the group STORED is the same bytes, in the same order — the delivery is a copy of
    // the ingested event, not a re-encoding of it.
    let page = emulator
        .logs(
            "the stored events",
            "GetLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": LOG_STREAM,
                "startFromHead": true,
                "limit": 100,
            }),
        )
        .await;
    let stored = crate::harness::messages(&page);
    let expected: Vec<String> = rows
        .iter()
        .flat_map(|row| row.messages.iter().cloned())
        .collect();
    assert_eq!(
        stored.len(),
        expected.len(),
        "case {:?}: every accepted event is stored, expected {} events, got {}",
        "the stored events",
        expected.len(),
        stored.len()
    );
    for (index, (stored, expected)) in stored.iter().zip(&expected).enumerate() {
        assert_eq!(
            stored.as_bytes(),
            expected.as_bytes(),
            "case {:?}: stored event {index} is the bytes that were accepted, got {stored:?}",
            "the stored events"
        );
    }
}

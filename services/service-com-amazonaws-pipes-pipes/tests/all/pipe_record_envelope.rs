// SPDX-License-Identifier: CC0-1.0

//! The Amazon SQS RECORD ENVELOPE a pipe delivers, end to end over the wire.
//!
//! Pipes and SQS are mounted on the SAME engine and every call goes through
//! `MultiServiceRouter::dispatch` — the one path a socket-served request takes — because
//! what the pipe wrote is read back the way a client reads it: off the target queue.
//!
//! The rule asserted over a TABLE: the envelope a pipe hands its target for a polled
//! Amazon SQS message reports that message as the queue reports it — the message's id,
//! receipt handle, body, system attributes, message attributes and the DIGESTS Amazon SQS
//! publishes for it, beside which source the record came from. A digest the queue
//! publishes for a message is on the envelope with the value the queue published, and a
//! digest the queue publishes for no message (the attribute digest of a message carrying
//! no message attributes) is on no envelope either.
//!
//! The rows differ along what the rule generalises over: how many message attributes the
//! source record carries (none, one, several) and of which types (`String`, `Number`,
//! `Binary`), which is exactly what the attribute digest is computed over.
//!
//! The second test asserts the same rule holds for EVERY delivery of a long-lived pipe,
//! including the one after a record the pipe's filter dropped: a dropped record is
//! consumed and leaves the pipe polling.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{collect_at_least, collect_during, emulator, given_queue, sqs, QUIET};
use crate::pipe_lifecycle::{call, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-record-envelope";

/// How long a wait for the one delivery a row expects is given before it is a failure.
/// Well above the data plane's poll interval and the span a pipe takes to reach RUNNING.
const BUDGET: Duration = Duration::from_secs(20);

/// The members an Amazon SQS record envelope carries for EVERY polled message.
///
/// Source: the `Message` members `ReceiveMessage` reports (`MessageId`, `ReceiptHandle`,
/// `Body`, `Attributes`, `MessageAttributes`, `MD5OfBody`), as the record envelope spells
/// them, plus the three that say which source the record came from.
const ALWAYS_PRESENT: &[&str] = &[
    "attributes",
    "awsRegion",
    "body",
    "eventSource",
    "eventSourceARN",
    "md5OfBody",
    "messageAttributes",
    "messageId",
    "receiptHandle",
];

/// The member carrying the digest of the message's ATTRIBUTES — `Message$MD5OfBody`'s
/// counterpart `Message$MD5OfMessageAttributes`, which Amazon SQS reports for a message
/// that carries message attributes and for no other.
const ATTRIBUTE_DIGEST: &str = "md5OfMessageAttributes";

/// The member carrying the digest of the message's BODY.
const BODY_DIGEST: &str = "md5OfBody";

/// One message attribute a row's source record carries: its name, its `DataType` and the
/// value, which is a `StringValue` for every type but `Binary` and a `BinaryValue` for it.
struct Attribute {
    name: &'static str,
    data_type: &'static str,
    /// The base64 text a `Binary` attribute is sent as, or `None` for a value sent as a
    /// string.
    binary: Option<&'static str>,
    string: Option<&'static str>,
}

/// One row of the envelope table.
struct Row {
    /// The case, which is also the pipe's and the queues' name.
    case: &'static str,
    /// The body the row sends.
    body: &'static str,
    /// The message attributes the row sends with it.
    attributes: &'static [Attribute],
}

/// RULE: the record envelope a pipe delivers reports the polled message as Amazon SQS
/// reports it, digests included: `md5OfBody` always, and `md5OfMessageAttributes` with the
/// value the queue published for exactly those records the queue published one for.
#[tokio::test]
async fn a_delivered_record_carries_the_digests_the_queue_published_for_it() {
    let rows = [
        Row {
            case: "no-message-attributes",
            body: r#"{"kind": "plain"}"#,
            attributes: &[],
        },
        Row {
            case: "one-string-attribute",
            body: r#"{"kind": "tagged"}"#,
            attributes: &[Attribute {
                name: "probe",
                data_type: "String",
                binary: None,
                string: Some("attr-good"),
            }],
        },
        Row {
            case: "three-attributes-of-three-types",
            body: "a body spelling no document at all",
            attributes: &[
                Attribute {
                    name: "probe",
                    data_type: "String",
                    binary: None,
                    string: Some("attr-good"),
                },
                Attribute {
                    name: "count",
                    data_type: "Number",
                    binary: None,
                    string: Some("42"),
                },
                Attribute {
                    name: "blob",
                    data_type: "Binary",
                    // base64 of "pipes", so the envelope's value is assertable text.
                    binary: Some("cGlwZXM="),
                    string: None,
                },
            ],
        },
        Row {
            case: "one-binary-attribute",
            body: r#"{"kind": "binary"}"#,
            attributes: &[Attribute {
                name: "blob",
                data_type: "Binary",
                binary: Some("cGlwZXM="),
                string: None,
            }],
        },
    ];

    for row in &rows {
        run_row(row).await;
    }
}

/// Keep the table honest: every row is driven by this one body.
async fn run_row(row: &Row) {
    let case = row.case;
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{case}-src")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{case}-tgt")).await;

    // What the queue itself published for this message: the digests the envelope has to
    // report are read off the send that produced them, never recomputed here.
    let published = send(&router, &source_url, row).await;

    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}"),
        Some(json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": 10 } },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    let delivered = collect_at_least(&router, &target_url, 1, BUDGET).await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: expected the one sent record delivered, got {delivered:?}"
    );
    assert_envelope(case, &delivered[0], &published, row);
}

/// Assert that `payload` is the record envelope of a message sent as `row` for which the
/// queue published `published`.
fn assert_envelope(case: &str, payload: &str, published: &Published, row: &Row) {
    let envelope: Value = serde_json::from_str(payload).unwrap_or_else(|error| {
        panic!("case {case:?}: a delivered body is not JSON ({error}): {payload:?}")
    });
    let members = envelope
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected the record envelope, got {envelope}"));

    // The envelope's MEMBERS: the ones every record carries, plus the attribute digest
    // exactly when the queue published one for this message.
    let mut expected: Vec<String> = ALWAYS_PRESENT.iter().map(|name| name.to_string()).collect();
    if published.attribute_digest.is_some() {
        expected.push(ATTRIBUTE_DIGEST.to_string());
    }
    expected.sort();
    let mut present: Vec<String> = members.keys().cloned().collect();
    present.sort();
    assert_eq!(
        present, expected,
        "case {case:?}: expected the record envelope to carry {expected:?}, got {envelope}"
    );

    assert_eq!(
        members.get(BODY_DIGEST).and_then(Value::as_str),
        Some(published.body_digest.as_str()),
        "case {case:?}: expected the body digest the queue published \
         ({:?}), got {envelope}",
        published.body_digest
    );
    assert_eq!(
        members.get(ATTRIBUTE_DIGEST).and_then(Value::as_str),
        published.attribute_digest.as_deref(),
        "case {case:?}: expected the attribute digest the queue published ({:?}), \
         got {envelope}",
        published.attribute_digest
    );
    assert_eq!(
        members.get("body").and_then(Value::as_str),
        Some(row.body),
        "case {case:?}: expected the message body as sent, got {envelope}"
    );

    // The digest is a digest OF the attributes, so the attributes it covers are on the
    // envelope too, each as the type and value it was sent with.
    for attribute in row.attributes {
        let reported = &envelope["messageAttributes"][attribute.name];
        assert_eq!(
            reported["dataType"].as_str(),
            Some(attribute.data_type),
            "case {case:?}: expected attribute {:?} reported as {:?}, got {envelope}",
            attribute.name,
            attribute.data_type
        );
        if let Some(text) = attribute.string {
            assert_eq!(
                reported["stringValue"].as_str(),
                Some(text),
                "case {case:?}: expected attribute {:?} to carry {text:?}, got {envelope}",
                attribute.name
            );
        }
        if let Some(base64) = attribute.binary {
            assert_eq!(
                reported["binaryValue"].as_str(),
                Some(base64),
                "case {case:?}: expected attribute {:?} to carry {base64:?}, got {envelope}",
                attribute.name
            );
        }
    }
}

/// The pattern the pipe of the second test filters by: it admits the records whose body
/// spells a document saying `kind: wanted`, and no others.
const WANTED_PATTERN: &str = r#"{"body": {"kind": ["wanted"]}}"#;

/// RULE (the same rule, over the whole life of one pipe): every delivery a pipe makes
/// carries the envelope of the record it was made for, digests included — including the
/// delivery for a record sent AFTER one the pipe's filter dropped, because a dropped record
/// is consumed and leaves the pipe polling.
#[tokio::test]
async fn a_record_sent_after_a_dropped_one_is_delivered_with_its_own_envelope() {
    // The legs differ along what the pipe must do with the record: deliver it, drop it,
    // deliver one sent after the drop. Each leg's body is its own, so a delivery is
    // attributed to the leg that sent it.
    let legs = [
        Row {
            case: "the first wanted record",
            body: r#"{"kind": "wanted", "leg": "one"}"#,
            attributes: &[Attribute {
                name: "probe",
                data_type: "String",
                binary: None,
                string: Some("attr-one"),
            }],
        },
        Row {
            case: "a record the filter drops",
            body: r#"{"kind": "other", "leg": "two"}"#,
            attributes: &[],
        },
        Row {
            case: "a wanted record sent after the dropped one",
            body: r#"{"kind": "wanted", "leg": "three"}"#,
            attributes: &[Attribute {
                name: "probe",
                data_type: "String",
                binary: None,
                string: Some("attr-three"),
            }],
        },
    ];
    let dropped = legs[1].body;

    let router = emulator();
    let name = "deliveries-across-a-dropped-record";
    let (source_url, source_arn) = given_queue(&router, &format!("{name}-src")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{name}-tgt")).await;
    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{name}"),
        Some(json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": {
                "SqsQueueParameters": { "BatchSize": 10 },
                "FilterCriteria": { "Filters": [{ "Pattern": WANTED_PATTERN }] },
            },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreatePipe answered {} {}",
        created.status, created.body
    );

    for leg in &legs {
        let case = leg.case;
        let published = send(&router, &source_url, leg).await;
        if leg.body == dropped {
            // Nothing arrives for a dropped record, and the quiet window is what makes
            // that absence an answer rather than a race.
            let seen = collect_during(&router, &target_url, QUIET).await;
            assert!(
                seen.is_empty(),
                "case {case:?}: a record the filter drops reaches no target, got {seen:?}"
            );
            continue;
        }
        let delivered = collect_at_least(&router, &target_url, 1, BUDGET).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected the record delivered, got {delivered:?}"
        );
        assert_envelope(case, &delivered[0], &published, leg);
    }
}

/// The digests Amazon SQS published for one sent message.
struct Published {
    body_digest: String,
    /// `None` when the message carried no message attributes, for which Amazon SQS
    /// publishes no attribute digest.
    attribute_digest: Option<String>,
}

/// Send a row's message to the queue at `url`, answering what the send published for it.
async fn send(router: &MultiServiceRouter, url: &str, row: &Row) -> Published {
    let case = row.case;
    let mut request = json!({ "QueueUrl": url, "MessageBody": row.body });
    if !row.attributes.is_empty() {
        let mut attributes = serde_json::Map::new();
        for attribute in row.attributes {
            let mut value = json!({ "DataType": attribute.data_type });
            if let Some(text) = attribute.string {
                value["StringValue"] = json!(text);
            }
            if let Some(base64) = attribute.binary {
                value["BinaryValue"] = json!(base64);
            }
            attributes.insert(attribute.name.to_string(), value);
        }
        request["MessageAttributes"] = Value::Object(attributes);
    }
    let (status, answer) = sqs(router, "SendMessage", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage answered {status} {answer}"
    );
    Published {
        body_digest: answer["MD5OfMessageBody"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no MD5OfMessageBody in {answer}"))
            .to_string(),
        attribute_digest: answer["MD5OfMessageAttributes"]
            .as_str()
            .map(str::to_string),
    }
}

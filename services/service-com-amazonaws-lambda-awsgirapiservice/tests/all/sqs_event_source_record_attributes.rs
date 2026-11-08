// SPDX-License-Identifier: CC0-1.0

//! What the `aws:sqs` record an event source mapping hands its function carries OF the
//! message that was sent, asserted through the function's own run: the handler of a real
//! deployment package forwards every record it is given to a second queue, so everything
//! this suite reads back is a record that really reached a running handler.
//!
//! RULE: a record Lambda hands a function for one message of a mapped queue reports that
//! message as the QUEUE holds it — its `messageId`, `body` and `md5OfBody`, the queue's own
//! ARN as `eventSourceARN`, `aws:sqs` as `eventSource`, the queue's Region, and one
//! `messageAttributes` entry per attribute the message carries with that attribute's
//! `dataType` and its value in the carrier the type names (`stringValue` for a `String` or a
//! `Number`, base64 `binaryValue` for a `Binary`) — where a `String`'s and a `Binary`'s value
//! travels byte for byte, while a `Number` attribute carries a NUMBER rather than the text of
//! one and so is delivered as that number's canonical plain decimal (no exponent, no
//! redundant sign, none of the leading zeros of the integer part and none of the
//! insignificant trailing zeros of the fraction), and `md5OfMessageAttributes` is the
//! documented digest of the attribute set as DELIVERED.
//!
//! The rows differ along what the rule generalises over: the data type the attribute
//! declares (each of the three logical types the Developer Guide publishes — "String, Number,
//! and Binary" — and a custom label on two of them), the carrier the value travels in, and
//! the form the value takes (plain text, text that is not ASCII, text that merely LOOKS like
//! a number, a number written plainly, in exponent form, with leading zeros, with
//! insignificant trailing zeros, negative, and zero itself; bytes that are not valid text and
//! no bytes beyond one).
//!
//! The harness — the router with Lambda and Amazon SQS on one engine, the queue fixture and
//! the function whose package runs — is the shared one in `event_source_mappings.rs`, so this
//! suite drives the same wire as every other over this surface.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use md5::{Digest, Md5};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    await_mapping_state, call_sqs, create_mapping, create_queue, create_running_function,
    router_with_queues, LOOK_AGAIN,
};

/// The `State` a mapping reports once it polls its event source
/// (`EventSourceMappingConfiguration$State`).
const STATE_ENABLED: &str = "Enabled";

/// The Region every request in this suite is signed for, and so the Region the queue it
/// creates lives in: `awsRegion` on a record is the Region of the queue the message came
/// from.
const REGION: &str = "us-east-1";

/// The `eventSource` a record of a queue message carries.
const EVENT_SOURCE: &str = "aws:sqs";

/// The `VisibilityTimeout` the source queue hides a delivered batch for, and the `Timeout` of
/// the function it maps onto: a queue only maps onto a function it outlasts, so the two are
/// the same span.
const ATTEMPT_SECONDS: i64 = 10;

/// How long this suite waits for the forwarded records the clock owes — the mapping settling,
/// its pass polling, the handler running and its own send landing — before the test is a
/// failure.
const BUDGET: Duration = Duration::from_secs(60);

/// The handler every message below is delivered to: it forwards each record it is handed,
/// verbatim, to the queue its environment names, so what this suite asserts is the record a
/// running handler was given.
const FORWARDING_HANDLER: &str = r#"
import json
import os

import boto3

SINK_URL = os.environ["SINK_URL"]
QUEUE_ARN = os.environ["QUEUE_ARN"]


def lambda_handler(event, context):
    client = boto3.client("sqs")
    forwarded = []
    for record in event["Records"]:
        if record["eventSource"] != "aws:sqs":
            raise ValueError("eventSource is " + repr(record["eventSource"]))
        if record["eventSourceARN"] != QUEUE_ARN:
            raise ValueError("eventSourceARN is " + repr(record["eventSourceARN"]))
        client.send_message(QueueUrl=SINK_URL, MessageBody=json.dumps(record))
        forwarded.append(record["messageId"])
    return {"forwarded": forwarded}
"#;

/// Which carrier one attribute's value travels in, and the value itself.
///
/// "For the Number data type, you must use StringValue" — so a `String` and a `Number` are
/// both carried as text and only a `Binary` is carried as bytes
/// (`smithy_query(query = "shape", shape = "MessageAttributeValue")`).
#[derive(Clone, Copy, Debug)]
enum Carried<'a> {
    /// The text a `String` or a `Number` attribute carries.
    Text(&'a str),
    /// The bytes a `Binary` attribute carries, which travel as base64 in both the Amazon SQS
    /// wire document and the JSON record.
    Bytes(&'a [u8]),
}

impl Carried<'_> {
    /// The attribute member a CLIENT writes this value in, under `MessageAttributes`.
    fn sent_member(&self, data_type: &str) -> Value {
        match self {
            Self::Text(text) => json!({ "DataType": data_type, "StringValue": text }),
            Self::Bytes(bytes) => json!({
                "DataType": data_type,
                "BinaryValue": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        }
    }

    /// The `messageAttributes` entry a RECORD must report this value as.
    fn record_entry(&self, data_type: &str) -> Value {
        match self {
            Self::Text(text) => json!({ "dataType": data_type, "stringValue": text }),
            Self::Bytes(bytes) => json!({
                "dataType": data_type,
                "binaryValue": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        }
    }

    /// The transport-type byte the digest encoding stands this value behind: `1` for a value
    /// carried as a string and `2` for one carried as bytes.
    fn transport_type(&self) -> u8 {
        match self {
            Self::Text(_) => 1,
            Self::Bytes(_) => 2,
        }
    }

    /// The bytes of the value itself, as the digest encoding covers them.
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

/// The Amazon SQS digest of a message carrying the ONE attribute `name` names, written out
/// from the Developer Guide's "Calculating the MD5 message digest for message attributes":
/// the name, the `DataType` and the value each prefixed with its length as a 4-byte
/// big-endian integer, with the transport-type byte standing before the value.
fn attribute_digest(name: &str, data_type: &str, value: Carried<'_>) -> String {
    let length_prefixed = |part: &[u8]| {
        let length = u32::try_from(part.len()).expect("a test value shorter than 4 GiB");
        [&length.to_be_bytes()[..], part].concat()
    };
    let encoded = [
        length_prefixed(name.as_bytes()),
        length_prefixed(data_type.as_bytes()),
        vec![value.transport_type()],
        length_prefixed(value.bytes()),
    ]
    .concat();
    hex::encode(Md5::digest(encoded))
}

/// One row: the attribute a message carries, and the entry the record for that message must
/// report it as.
struct Row {
    case: &'static str,
    /// The name the message carries the attribute under. Every row uses its own, so a record
    /// is never matched against another row's expectation.
    name: &'static str,
    /// The `DataType` the attribute declares.
    data_type: &'static str,
    /// The value as the client sends it.
    sent: Carried<'static>,
    /// The value the record must carry, in the same carrier.
    delivered: Carried<'static>,
}

/// The bytes a binary row sends: a sequence that is not valid UTF-8, so a delivery that went
/// through text would not survive it.
const BINARY_BYTES: &[u8] = &[0x00, 0x01, 0xfe, 0xff, 0x80];

/// The catalog. The three logical types are the ones `SendMessage` publishes ("Amazon SQS
/// supports the following logical data types: String, Number, and Binary"), with the custom
/// label the same paragraph allows ("You can also append custom labels"); the Number forms
/// walk the spellings one number can be written in.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a String value",
            name: "text",
            data_type: "String",
            sent: Carried::Text("order-placed"),
            delivered: Carried::Text("order-placed"),
        },
        Row {
            case: "a String value that is not ASCII",
            name: "place",
            data_type: "String",
            sent: Carried::Text("café ⚑ münchen"),
            delivered: Carried::Text("café ⚑ münchen"),
        },
        Row {
            case: "a String carrying a custom label",
            name: "shade",
            data_type: "String.colour",
            sent: Carried::Text("crimson"),
            delivered: Carried::Text("crimson"),
        },
        Row {
            // Only a Number is a number: the same text under a String type is text, and text
            // travels byte for byte.
            case: "a String whose text merely looks like a number",
            name: "written",
            data_type: "String",
            sent: Carried::Text("0042.50"),
            delivered: Carried::Text("0042.50"),
        },
        Row {
            case: "a Number written plainly",
            name: "count",
            data_type: "Number",
            sent: Carried::Text("17"),
            delivered: Carried::Text("17"),
        },
        Row {
            case: "a Number with leading and insignificant trailing zeros",
            name: "amount",
            data_type: "Number",
            sent: Carried::Text("0042.50"),
            delivered: Carried::Text("42.5"),
        },
        Row {
            case: "a Number in exponent form",
            name: "capacity",
            data_type: "Number",
            sent: Carried::Text("1e3"),
            delivered: Carried::Text("1000"),
        },
        Row {
            case: "a negative Number with an insignificant trailing zero",
            name: "adjustment",
            data_type: "Number",
            sent: Carried::Text("-12.500"),
            delivered: Carried::Text("-12.5"),
        },
        Row {
            case: "a Number that is zero written with a fraction",
            name: "balance",
            data_type: "Number",
            sent: Carried::Text("0.00"),
            delivered: Carried::Text("0"),
        },
        Row {
            case: "a Number carrying a custom label",
            name: "ratio",
            data_type: "Number.float",
            sent: Carried::Text("1.2300"),
            delivered: Carried::Text("1.23"),
        },
        Row {
            case: "a Binary value of bytes that are not text",
            name: "payload",
            data_type: "Binary",
            sent: Carried::Bytes(BINARY_BYTES),
            delivered: Carried::Bytes(BINARY_BYTES),
        },
        Row {
            case: "a Binary value of one byte",
            name: "flag",
            data_type: "Binary.flag",
            sent: Carried::Bytes(&[0xff]),
            delivered: Carried::Bytes(&[0xff]),
        },
    ]
}

/// The body a row's message carries: its own case, so a forwarded record is matched to the
/// row it belongs to by what was sent rather than by the order anything arrived in.
fn body_of(row: &Row) -> String {
    format!("record for {}", row.case)
}

/// `SendMessage` of `body` carrying the one attribute `row` names: the `MessageId` Amazon SQS
/// minted for it.
async fn send_row(router: &MultiServiceRouter, url: &str, row: &Row) -> String {
    let case = row.case;
    let (status, sent) = call_sqs(
        router,
        "SendMessage",
        &json!({
            "QueueUrl": url,
            "MessageBody": body_of(row),
            "MessageAttributes": { row.name: row.sent.sent_member(row.data_type) },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );
    // The digest the SEND reports is of the attributes the request carried — "an MD5 digest
    // of the non-URL-encoded message attribute string", which a sender compares against its
    // own digest to "verify that Amazon SQS received the message correctly" — so for a value
    // the queue keeps in another form it differs from the record's own digest below.
    let expected_digest = attribute_digest(row.name, row.data_type, row.sent);
    assert_eq!(
        sent["MD5OfMessageAttributes"].as_str(),
        Some(expected_digest.as_str()),
        "case {case:?}: expected the send's MD5OfMessageAttributes to cover the attributes \
         the request carried ({expected_digest}), got {sent}"
    );
    sent["MessageId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no MessageId in {sent}"))
        .to_string()
}

/// Every record the handler has forwarded to the sink queue so far, keyed by the `body` it
/// reports — read until `wanted` of them have arrived or the budget is out.
async fn forwarded_records(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
) -> BTreeMap<String, Value> {
    let mut records: BTreeMap<String, Value> = BTreeMap::new();
    let deadline = Instant::now() + BUDGET;
    while records.len() < wanted && Instant::now() < deadline {
        let (status, answered) = call_sqs(
            router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                // Long enough that a record counted here is not handed over again while the
                // rest arrive.
                "VisibilityTimeout": 300,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "expected ReceiveMessage on the sink queue to answer 200, got {status} {answered}"
        );
        for message in answered["Messages"].as_array().cloned().unwrap_or_default() {
            let text = message["Body"]
                .as_str()
                .unwrap_or_else(|| panic!("a forwarded message with no Body: {message}"));
            let record: Value = serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("a forwarded record that is not JSON: {text} ({error})"));
            let body = record["body"]
                .as_str()
                .unwrap_or_else(|| panic!("a forwarded record carrying no body: {record}"))
                .to_string();
            records.insert(body, record);
        }
        if records.len() < wanted {
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
    records
}

/// One `attributes` entry of a record, as text.
fn attribute(record: &Value, name: &str) -> String {
    record["attributes"][name]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_delivered_record_reports_the_message_the_queue_holds() {
    let case = "a queue whose messages carry one attribute each";
    let router = router_with_queues();
    let rows = rows();

    let (sink_url, _) = create_queue(&router, case, "esm-record-sink", &json!({})).await;
    let (source_url, source_arn) = create_queue(
        &router,
        case,
        "esm-record-source",
        &json!({ "VisibilityTimeout": ATTEMPT_SECONDS.to_string() }),
    )
    .await;
    create_running_function(
        &router,
        case,
        "esm-record-fn",
        FORWARDING_HANDLER,
        ATTEMPT_SECONDS,
        &json!({ "QUEUE_ARN": source_arn, "SINK_URL": sink_url }),
    )
    .await;
    let created = create_mapping(
        &router,
        &json!({
            "FunctionName": "esm-record-fn",
            "EventSourceArn": source_arn,
            "BatchSize": 10,
            "Enabled": true,
        }),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: CreateEventSourceMapping answered {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    await_mapping_state(&router, case, &uuid, STATE_ENABLED).await;

    // One message per row, each remembering the id Amazon SQS minted for it: a record is
    // about THAT message or the row fails.
    let mut sent_ids: BTreeMap<String, String> = BTreeMap::new();
    for row in &rows {
        sent_ids.insert(body_of(row), send_row(&router, &source_url, row).await);
    }

    let records = forwarded_records(&router, &sink_url, rows.len()).await;
    assert_eq!(
        records.len(),
        rows.len(),
        "case {case:?}: every message sent must reach the handler as a record, got {} of {} \
         ({:?})",
        records.len(),
        rows.len(),
        records.keys().collect::<Vec<_>>()
    );

    for row in &rows {
        let case = row.case;
        let body = body_of(row);
        let record = records
            .get(&body)
            .unwrap_or_else(|| panic!("case {case:?}: no record for the body {body:?}"));

        // The envelope is about the message that was sent, as the queue holds it.
        let expected_id = sent_ids.get(&body).cloned().unwrap_or_default();
        assert_eq!(
            record["messageId"].as_str(),
            Some(expected_id.as_str()),
            "case {case:?}: expected the record to carry the sent message's id \
             {expected_id:?}, got {record}"
        );
        assert!(
            record["receiptHandle"].as_str().is_some_and(|handle| !handle.is_empty()),
            "case {case:?}: expected the record to carry a receipt handle, got {record}"
        );
        assert_eq!(
            (
                record["eventSource"].as_str(),
                record["eventSourceARN"].as_str(),
                record["awsRegion"].as_str(),
            ),
            (
                Some(EVENT_SOURCE),
                Some(source_arn.as_str()),
                Some(REGION),
            ),
            "case {case:?}: expected the record to name its source queue, got {record}"
        );
        let expected_md5 = hex::encode(Md5::digest(body.as_bytes()));
        assert_eq!(
            record["md5OfBody"].as_str(),
            Some(expected_md5.as_str()),
            "case {case:?}: expected md5OfBody {expected_md5:?} for the body {body:?}, got \
             {record}"
        );

        // The attributes the delivery itself accounts for: this is each message's first
        // delivery, and both timestamps are the queue's own millisecond readings.
        assert_eq!(
            attribute(record, "ApproximateReceiveCount"),
            "1",
            "case {case:?}: the record is the message's first delivery, got {record}"
        );
        for name in ["SentTimestamp", "ApproximateFirstReceiveTimestamp"] {
            let reading = attribute(record, name);
            assert!(
                reading.parse::<i64>().is_ok_and(|millis| millis > 0),
                "case {case:?}: expected {name} to be a millisecond reading, got \
                 {reading:?} in {record}"
            );
        }
        assert!(
            !attribute(record, "SenderId").is_empty(),
            "case {case:?}: expected the record to report who sent the message, got {record}"
        );

        // And the attribute itself, in the carrier its type names.
        let expected_entry = row.delivered.record_entry(row.data_type);
        let reported = record["messageAttributes"][row.name].clone();
        assert_eq!(
            reported, expected_entry,
            "case {case:?}: expected the attribute {:?} to be delivered as {expected_entry}, \
             got {reported} in {record}",
            row.name
        );
        assert_eq!(
            record["messageAttributes"]
                .as_object()
                .map(|attributes| attributes.len()),
            Some(1),
            "case {case:?}: the message carried one attribute, so the record reports one, got \
             {record}"
        );
        // The digest is OF the delivered set: a consumer that digests the attributes it was
        // handed gets the value the record reports.
        let expected_digest = attribute_digest(row.name, row.data_type, row.delivered);
        assert_eq!(
            record["md5OfMessageAttributes"].as_str(),
            Some(expected_digest.as_str()),
            "case {case:?}: expected md5OfMessageAttributes to be the digest of the delivered \
             attribute set ({expected_digest}), got {record}"
        );
    }
}

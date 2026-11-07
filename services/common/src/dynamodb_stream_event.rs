// SPDX-License-Identifier: CC0-1.0

//! The ONE rendering of a DynamoDB STREAM RECORD as the JSON envelope a consumer of that
//! stream is handed one in, for every service that reads a table's stream and passes the
//! records on — so two consumers of the same record cannot be handed different documents.
//!
//! A DynamoDB stream record travels to a consumer as a document whose member names are the
//! stream record's own (`eventID`, `eventName`, `eventVersion`, `eventSource`, `awsRegion`,
//! `dynamodb`, `eventSourceARN`), with the change itself under `dynamodb` named as
//! `StreamRecord`'s members are (`ApproximateCreationDateTime`, `Keys`, `NewImage`,
//! `OldImage`, `SequenceNumber`, `SizeBytes`, `StreamViewType`). A member the record does
//! not carry is ABSENT rather than reported empty: `OldImage` is not there for an `INSERT`,
//! and neither image is there for a `KEYS_ONLY` stream.
//!
//! The record is taken as the DynamoDB Streams data plane answered it
//! (`::aws_sdk_dynamodbstreams::types::Record`), so nothing here invents a value: a
//! consumer reads the change the stream published.

use std::collections::BTreeMap;

use ::serde_json::{json, Map, Value};

/// The `eventSource` a DynamoDB stream record envelope reports.
pub const EVENT_SOURCE: &str = "aws:dynamodb";

/// The `eventVersion` of the envelope rendered here — the version of the record FORMAT,
/// which is what a consumer branches on — used for a record the stream reported none for.
pub const EVENT_VERSION: &str = "1.1";

/// One stream record as the envelope a consumer is handed it in: which change it was,
/// where it came from, and the record's own `dynamodb` body — the item's keys and the
/// images the stream's view type captures.
///
/// `source_arn` is the stream ARN the consumer reads, which the envelope names as the
/// source of the record, and `region` is the Region that stream lives in, used for a
/// record that reports none of its own.
///
/// `Err` for a record carrying an attribute type this rendering does not know, so a caller
/// can leave the record where it is rather than pass on an envelope with a value missing.
pub fn record_envelope(
    record: &::aws_sdk_dynamodbstreams::types::Record,
    source_arn: &str,
    region: &str,
) -> Result<Value, String> {
    let mut envelope = Map::new();
    if let Some(id) = record.event_id.clone() {
        envelope.insert("eventID".to_string(), Value::String(id));
    }
    if let Some(name) = record.event_name.as_ref() {
        envelope.insert(
            "eventName".to_string(),
            Value::String(name.as_str().to_string()),
        );
    }
    envelope.insert(
        "eventVersion".to_string(),
        Value::String(
            record
                .event_version
                .clone()
                .unwrap_or_else(|| EVENT_VERSION.to_string()),
        ),
    );
    envelope.insert(
        "eventSource".to_string(),
        Value::String(
            record
                .event_source
                .clone()
                .unwrap_or_else(|| EVENT_SOURCE.to_string()),
        ),
    );
    // The Region the record reports is the stream's own; the source ARN's is the fallback
    // for a record that carries none.
    envelope.insert(
        "awsRegion".to_string(),
        Value::String(
            record
                .aws_region
                .clone()
                .unwrap_or_else(|| region.to_string()),
        ),
    );
    if let Some(body) = record.dynamodb.as_ref() {
        let mut stream_record = Map::new();
        if let Some(at) = body.approximate_creation_date_time.as_ref() {
            // "in Unix time seconds", as a stream record reports the instant a change was
            // made.
            stream_record.insert(
                "ApproximateCreationDateTime".to_string(),
                Value::from(at.secs()),
            );
        }
        if let Some(keys) = body.keys.as_ref() {
            stream_record.insert("Keys".to_string(), attribute_map_json(keys)?);
        }
        if let Some(image) = body.new_image.as_ref() {
            stream_record.insert("NewImage".to_string(), attribute_map_json(image)?);
        }
        if let Some(image) = body.old_image.as_ref() {
            stream_record.insert("OldImage".to_string(), attribute_map_json(image)?);
        }
        if let Some(sequence) = body.sequence_number.clone() {
            stream_record.insert("SequenceNumber".to_string(), Value::String(sequence));
        }
        if let Some(size) = body.size_bytes {
            stream_record.insert("SizeBytes".to_string(), Value::from(size));
        }
        if let Some(view) = body.stream_view_type.as_ref() {
            stream_record.insert(
                "StreamViewType".to_string(),
                Value::String(view.as_str().to_string()),
            );
        }
        envelope.insert("dynamodb".to_string(), Value::Object(stream_record));
    }
    envelope.insert(
        "eventSourceARN".to_string(),
        Value::String(source_arn.to_string()),
    );
    Ok(Value::Object(envelope))
}

/// One item image as the JSON a stream record carries it in: each attribute under its
/// name, as [`attribute_json`] renders it.
fn attribute_map_json(
    image: &::std::collections::HashMap<String, ::aws_sdk_dynamodbstreams::types::AttributeValue>,
) -> Result<Value, String> {
    // Collected into an ordered map so one image always renders the same way, whatever
    // order the hash map happens to iterate in.
    let mut rendered: BTreeMap<String, Value> = BTreeMap::new();
    for (name, value) in image {
        rendered.insert(name.clone(), attribute_json(value)?);
    }
    Ok(json!(rendered))
}

/// One attribute value as the JSON DynamoDB carries one in: a single-member object whose
/// name is the attribute's TYPE (`S`, `N`, `B`, `SS`, `NS`, `BS`, `M`, `L`, `NULL`,
/// `BOOL`) — the same form a `PutItem` request writes it in, so a consumer reads the item
/// back in the shape it wrote it.
///
/// ITERATIVE, over an explicit worklist, for the reason the DynamoDB-to-streams seam gives
/// for its own walk over an image: `M` and `L` nest to a depth the client chose, and a
/// recursive walk over it would overflow the thread stack, which in Rust aborts the
/// process instead of unwinding.
///
/// A nesting value is met twice: once to put its members on the worklist, and once to
/// rebuild it from the members converted since. A map's names are CARRIED from the pass
/// that pushed its members rather than read off the map a second time, so the pairing does
/// not depend on two iterations of one map agreeing.
fn attribute_json(
    root: &::aws_sdk_dynamodbstreams::types::AttributeValue,
) -> Result<Value, String> {
    use ::aws_sdk_dynamodbstreams::types::AttributeValue as Attribute;

    /// What the walk still owes: a value to render, or a container to rebuild from the
    /// members already rendered.
    enum Step<'a> {
        Render(&'a Attribute),
        RebuildMap(Vec<&'a String>),
        RebuildList(usize),
    }

    /// The base64 of one binary attribute, which is how JSON carries bytes — through the
    /// codec this workspace already encodes a wire-visible blob with rather than a second
    /// one.
    fn encoded(bytes: &::aws_smithy_types::Blob) -> Value {
        Value::String(::aws_smithy_types::base64::encode(bytes.as_ref()))
    }

    let mut worklist = vec![Step::Render(root)];
    let mut rendered: Vec<Value> = Vec::new();
    while let Some(step) = worklist.pop() {
        match step {
            Step::Render(Attribute::M(members)) => {
                let mut names = Vec::with_capacity(members.len());
                let mut values = Vec::with_capacity(members.len());
                for (name, nested) in members {
                    names.push(name);
                    values.push(nested);
                }
                worklist.push(Step::RebuildMap(names));
                worklist.extend(values.into_iter().map(Step::Render));
            }
            Step::Render(Attribute::L(members)) => {
                worklist.push(Step::RebuildList(members.len()));
                worklist.extend(members.iter().map(Step::Render));
            }
            Step::Render(Attribute::S(text)) => rendered.push(json!({ "S": text })),
            Step::Render(Attribute::N(number)) => rendered.push(json!({ "N": number })),
            Step::Render(Attribute::B(bytes)) => rendered.push(json!({ "B": encoded(bytes) })),
            Step::Render(Attribute::Ss(members)) => rendered.push(json!({ "SS": members })),
            Step::Render(Attribute::Ns(members)) => rendered.push(json!({ "NS": members })),
            Step::Render(Attribute::Bs(members)) => rendered.push(json!({
                "BS": members.iter().map(encoded).collect::<Vec<Value>>(),
            })),
            Step::Render(Attribute::Null(is_null)) => rendered.push(json!({ "NULL": is_null })),
            Step::Render(Attribute::Bool(flag)) => rendered.push(json!({ "BOOL": flag })),
            // The union is `#[non_exhaustive]`: a member this build of the SDK does not
            // know is a record this rendering cannot carry faithfully, so the caller is
            // told rather than a value going missing from an envelope.
            Step::Render(_) => {
                return Err(
                    "a stream record carries an attribute type this emulator does not render"
                        .to_string(),
                )
            }
            Step::RebuildMap(names) => {
                let nested = take_rendered(&mut rendered, names.len())?;
                rendered.push(json!({
                    "M": names
                        .into_iter()
                        .cloned()
                        .zip(nested.into_iter().rev())
                        .collect::<BTreeMap<String, Value>>(),
                }));
            }
            Step::RebuildList(count) => {
                let mut nested = take_rendered(&mut rendered, count)?;
                nested.reverse();
                rendered.push(json!({ "L": nested }));
            }
        }
    }
    // The walk pushes exactly one value for the root, so this is it.
    rendered
        .pop()
        .ok_or_else(|| "an attribute value rendered to no value".to_string())
}

/// The `count` values a container's members rendered to, taken off the top of the walk's
/// stack. Every `Step::Render` pushes exactly one value, so the top `count` are this
/// container's; a shorter stack would be a defect in that invariant, and the caller is told
/// rather than a truncated container reaching a consumer.
fn take_rendered(rendered: &mut Vec<Value>, count: usize) -> Result<Vec<Value>, String> {
    if rendered.len() < count {
        return Err(
            "a nested attribute value rendered to fewer values than it has members".to_string(),
        );
    }
    let from = rendered.len() - count;
    Ok(rendered.split_off(from))
}

#[cfg(test)]
mod tests {
    use ::aws_sdk_dynamodbstreams::types::{AttributeValue, OperationType, Record, StreamRecord};
    use ::serde_json::json;

    use super::{record_envelope, EVENT_SOURCE, EVENT_VERSION};

    /// The stream ARN the rows below read, in the form a table's stream ARN takes.
    const STREAM_ARN: &str =
        "arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/2026-01-01T00:00:00.000";

    /// One image of a row: each attribute under the name it is written at.
    type Image = Option<Vec<(&'static str, AttributeValue)>>;

    /// One row: the case, which change the record is, and the new and old images the
    /// stream's view type captured for it.
    type Row = (&'static str, OperationType, Image, Image);

    /// RULE: a stream record is rendered as the envelope a consumer reads one in — the
    /// change's name, the stream it came from, and the item's keys and images under
    /// `dynamodb`, each attribute in the type-tagged form DynamoDB writes one in — and a
    /// member the record does not carry is absent rather than empty.
    ///
    /// The rows differ along what the rule generalises over: which change the record is,
    /// which images the stream's view type captures, and the types of the attributes an
    /// image holds (string, number, boolean, binary, a set, and a nested map and list).
    #[test]
    fn a_record_renders_as_the_envelope_a_consumer_reads_it_in() {
        let rows: Vec<Row> = vec![
            (
                "insert-with-a-new-image",
                OperationType::Insert,
                Some(vec![
                    ("pk", AttributeValue::S("one".to_string())),
                    ("count", AttributeValue::N("7".to_string())),
                    ("ok", AttributeValue::Bool(true)),
                ]),
                None,
            ),
            (
                "modify-with-both-images",
                OperationType::Modify,
                Some(vec![("pk", AttributeValue::S("two".to_string()))]),
                Some(vec![("pk", AttributeValue::S("two-old".to_string()))]),
            ),
            (
                "remove-with-an-old-image-only",
                OperationType::Remove,
                None,
                Some(vec![(
                    "blob",
                    AttributeValue::B(::aws_smithy_types::Blob::new(vec![1u8, 2, 3])),
                )]),
            ),
            (
                "keys-only-carries-no-image",
                OperationType::Insert,
                None,
                None,
            ),
            (
                "nested-containers-and-a-set",
                OperationType::Insert,
                Some(vec![
                    (
                        "inner",
                        AttributeValue::M(
                            [("leaf".to_string(), AttributeValue::S("deep".to_string()))]
                                .into_iter()
                                .collect(),
                        ),
                    ),
                    (
                        "items",
                        AttributeValue::L(vec![
                            AttributeValue::N("1".to_string()),
                            AttributeValue::Null(true),
                        ]),
                    ),
                    (
                        "tags",
                        AttributeValue::Ss(vec!["a".to_string(), "b".to_string()]),
                    ),
                ]),
                None,
            ),
        ];

        for (case, event_name, new_image, old_image) in rows {
            let mut body = StreamRecord::builder()
                .sequence_number("100000000000000000001")
                .size_bytes(26)
                .keys("pk", AttributeValue::S("one".to_string()));
            for (name, value) in new_image.clone().unwrap_or_default() {
                body = body.new_image(name, value);
            }
            for (name, value) in old_image.clone().unwrap_or_default() {
                body = body.old_image(name, value);
            }
            let record = Record::builder()
                .event_id("1")
                .event_name(event_name.clone())
                .dynamodb(body.build())
                .build();
            let envelope = record_envelope(&record, STREAM_ARN, "us-east-1")
                .unwrap_or_else(|error| panic!("case {case:?}: the record must render: {error}"));

            assert_eq!(
                envelope["eventName"],
                json!(event_name.as_str()),
                "case {case:?}: the envelope must name the change, got {envelope}"
            );
            assert_eq!(
                envelope["eventSource"],
                json!(EVENT_SOURCE),
                "case {case:?}: expected the source {EVENT_SOURCE}, got {envelope}"
            );
            assert_eq!(
                envelope["eventVersion"],
                json!(EVENT_VERSION),
                "case {case:?}: expected the version {EVENT_VERSION}, got {envelope}"
            );
            assert_eq!(
                envelope["eventSourceARN"],
                json!(STREAM_ARN),
                "case {case:?}: the envelope must name the stream it came from, got {envelope}"
            );
            assert_eq!(
                envelope["dynamodb"]["Keys"],
                json!({ "pk": { "S": "one" } }),
                "case {case:?}: the keys must be type-tagged, got {envelope}"
            );
            for (member, image) in [("NewImage", new_image), ("OldImage", old_image)] {
                match image {
                    None => assert!(
                        envelope["dynamodb"].get(member).is_none(),
                        "case {case:?}: {member} must be absent for a record without one, \
                         got {envelope}"
                    ),
                    Some(attributes) => {
                        for (name, _) in attributes {
                            assert!(
                                envelope["dynamodb"][member].get(name).is_some(),
                                "case {case:?}: {member} must carry {name:?}, got {envelope}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// RULE: each attribute of an image is rendered as the single-member, type-tagged
    /// object DynamoDB carries one in, nesting included.
    #[test]
    fn every_attribute_type_renders_type_tagged() {
        // (case, the attribute, the JSON DynamoDB carries it as)
        let rows: Vec<(&str, AttributeValue, ::serde_json::Value)> = vec![
            ("string", AttributeValue::S("text".to_string()), json!({ "S": "text" })),
            ("number", AttributeValue::N("1.5".to_string()), json!({ "N": "1.5" })),
            (
                "binary",
                AttributeValue::B(::aws_smithy_types::Blob::new(vec![0u8, 1])),
                json!({ "B": "AAE=" }),
            ),
            ("boolean", AttributeValue::Bool(false), json!({ "BOOL": false })),
            ("null", AttributeValue::Null(true), json!({ "NULL": true })),
            (
                "string-set",
                AttributeValue::Ss(vec!["a".to_string()]),
                json!({ "SS": ["a"] }),
            ),
            (
                "number-set",
                AttributeValue::Ns(vec!["2".to_string()]),
                json!({ "NS": ["2"] }),
            ),
            (
                "binary-set",
                AttributeValue::Bs(vec![::aws_smithy_types::Blob::new(vec![255u8])]),
                json!({ "BS": ["/w=="] }),
            ),
            (
                "list-of-lists",
                AttributeValue::L(vec![AttributeValue::L(vec![AttributeValue::S(
                    "nested".to_string(),
                )])]),
                json!({ "L": [{ "L": [{ "S": "nested" }] }] }),
            ),
            (
                "map-in-a-list",
                AttributeValue::L(vec![AttributeValue::M(
                    [
                        ("a".to_string(), AttributeValue::N("1".to_string())),
                        ("b".to_string(), AttributeValue::S("two".to_string())),
                    ]
                    .into_iter()
                    .collect(),
                )]),
                json!({ "L": [{ "M": { "a": { "N": "1" }, "b": { "S": "two" } } }] }),
            ),
        ];

        for (case, attribute, expected) in rows {
            let record = Record::builder()
                .event_name(OperationType::Insert)
                .dynamodb(
                    StreamRecord::builder()
                        .new_image("value", attribute)
                        .build(),
                )
                .build();
            let envelope = record_envelope(&record, STREAM_ARN, "us-east-1")
                .unwrap_or_else(|error| panic!("case {case:?}: the record must render: {error}"));
            let rendered = &envelope["dynamodb"]["NewImage"]["value"];
            assert_eq!(
                rendered, &expected,
                "case {case:?}: expected {expected}, got {rendered}"
            );
        }
    }
}

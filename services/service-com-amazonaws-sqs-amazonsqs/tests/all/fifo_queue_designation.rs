// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for THE ATTRIBUTE THAT DESIGNATES A QUEUE AS FIFO and for the
//! FIFO-only attribute a FIFO queue carries whether or not its create named it, driven over
//! the wire (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness
//! in [`super::support`].
//!
//! Two rules are asserted here.
//!
//! 1. **An attribute that exists only to DESIGNATE what a queue is carries the one value
//!    that designates it; a request supplying any other value names an attribute the
//!    service does not have, and is refused with `InvalidAttributeName` at HTTP 400 and the
//!    sentence `Unknown Attribute <name>.` before anything is written.** `FifoQueue`
//!    "designates a queue as FIFO" and no attribute designates a standard queue — "if you
//!    don't specify the FifoQueue attribute, Amazon SQS creates a standard queue" — so
//!    there is nothing a value other than the designating one could ask for. The refusal is
//!    observable twice over: in the answer, and in the queue that was never created, which
//!    `GetQueueUrl` reports as a queue that does not exist.
//!
//! 2. **An attribute that applies to the kind of queue being created is MATERIALISED with
//!    its documented default, so a read reports that default rather than nothing.** A FIFO
//!    queue whose create enabled no content-based deduplication reads the attribute back as
//!    `false`: "absent" and `false` are not interchangeable in the attribute map, and a
//!    client reading the attribute to decide whether it must supply a
//!    `MessageDeduplicationId` is answered either way. A standard queue does not carry a
//!    FIFO-only attribute at all, so it reports none — which is the other half of the same
//!    rule.
//!
//! The rows span what each rule generalises over rather than one example: for rule 1 every
//! FORM a non-designating value takes (the opposite boolean, the designating word in
//! another case, a numeric spelling, a padded one, no text at all, a word outside the
//! attribute's vocabulary), each beside the designating value that must be accepted; for
//! rule 2 the FIFO attribute sets a create may carry (the designator alone, with the other
//! FIFO-only attributes, with the default supplied explicitly, with the opposite supplied),
//! each read back through both forms of `GetQueueAttributes` and after a later
//! `SetQueueAttributes`.

use serde_json::{json, Value};

use super::support::{
    all_attributes, call, create, create_ok, emulator, error_code, expected_url, ok,
};

/// The refusal class an attribute name a call cannot accept carries, and its status.
const UNKNOWN_ATTRIBUTE: (u16, &str) = (400, "InvalidAttributeName");

/// The refusal class a queue nothing created carries, and its status.
const NO_SUCH_QUEUE: (u16, &str) = (400, "QueueDoesNotExist");

/// The attribute that designates a queue as FIFO, spelled out here rather than imported so
/// a wrong constant in the service cannot make a row pass. Source:
/// `smithy_query(query="shape",shape="com.amazonaws.sqs#QueueAttributeName",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`.
const FIFO_QUEUE: &str = "FifoQueue";

/// The one value that designates a queue as FIFO.
const DESIGNATES: &str = "true";

/// The FIFO-only attribute every FIFO queue carries, from the same enum.
const CONTENT_BASED_DEDUPLICATION: &str = "ContentBasedDeduplication";

/// The value a FIFO queue reports for it when its create enabled nothing.
const NOT_CONTENT_BASED: &str = "false";

/// The one sentence Amazon SQS refuses an attribute name it does not have with, spelled out
/// here rather than imported.
fn unknown_attribute_message(name: &str) -> String {
    format!("Unknown Attribute {name}.")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Every value of the designating attribute that designates NOTHING, spanning the forms a
/// client supplies one in.
const NON_DESIGNATING_VALUES: &[(&str, &str)] = &[
    ("the opposite boolean", "false"),
    ("the designating word upper-cased", "TRUE"),
    ("the designating word capitalised", "True"),
    ("the opposite boolean upper-cased", "FALSE"),
    ("a numeric spelling", "1"),
    ("the opposite numeric spelling", "0"),
    ("a padded designating word", " true"),
    ("the designating word with trailing space", "true "),
    ("no text at all", ""),
    ("a word outside the attribute's vocabulary", "yes"),
];

/// Rule 1: only the designating value is a value of the designating attribute, and a create
/// supplying any other is refused as an attribute the call does not have, with no queue left
/// behind.
#[tokio::test]
async fn the_fifo_designator_takes_only_the_value_that_designates_and_creates_nothing_otherwise() {
    let router = emulator();

    for (index, (case, value)) in NON_DESIGNATING_VALUES.iter().enumerate() {
        // The name is one a standard queue may carry, so the row is judged on the attribute
        // it supplies and never on the name beside it.
        let name = format!("designator-{index}");
        let (status, parsed) = create(
            &router,
            &name,
            &json!({ FIFO_QUEUE.to_string(): value.to_string() }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str(), message_of(&parsed)),
            (
                UNKNOWN_ATTRIBUTE.0,
                UNKNOWN_ATTRIBUTE.1,
                unknown_attribute_message(FIFO_QUEUE)
            ),
            "case {case} ({value:?}): expected the unknown-attribute refusal, got {status} \
             with {parsed}"
        );

        // Nothing was written: the name resolves to no queue, which is the error class a
        // client probes existence with.
        let (status, parsed) = call(&router, "GetQueueUrl", &json!({ "QueueName": name })).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            NO_SUCH_QUEUE,
            "case {case} ({value:?}): a refused create must leave no queue behind, got \
             {status} with {parsed}"
        );
    }

    // The control: the designating value, on the name a FIFO queue must carry, is accepted
    // and the queue it designates resolves and reports itself as FIFO.
    let url = create_ok(
        &router,
        "designator-accepted.fifo",
        &json!({ FIFO_QUEUE.to_string(): DESIGNATES }),
    )
    .await;
    let resolved = ok(
        &router,
        "GetQueueUrl",
        json!({"QueueName": "designator-accepted.fifo"}),
    )
    .await;
    assert_eq!(
        resolved["QueueUrl"].as_str(),
        Some(expected_url("designator-accepted.fifo").as_str()),
        "case the designating value: expected the created queue to resolve, got {resolved}"
    );
    let reported = all_attributes(&router, &url).await;
    assert_eq!(
        reported.get(FIFO_QUEUE).map(String::as_str),
        Some(DESIGNATES),
        "case the designating value: expected the queue to report itself as FIFO, got \
         {reported:?}"
    );
}

/// One row of the FIFO-default table: what the create supplies, and what the queue must
/// then report for the FIFO-only attribute.
struct DefaultRow {
    case: &'static str,
    /// The attributes supplied beside the designator.
    supplied: Value,
    /// The value `ContentBasedDeduplication` must read back as.
    expected: &'static str,
}

/// Rule 2: a FIFO queue reports the documented default of the FIFO-only attribute its
/// create left out, through both forms of the read; a value the create supplied wins, and a
/// later `SetQueueAttributes` is what changes it.
#[tokio::test]
async fn a_fifo_queue_reports_the_default_of_the_fifo_only_attribute_its_create_left_out() {
    let router = emulator();
    let rows = [
        DefaultRow {
            case: "the designator alone",
            supplied: json!({}),
            expected: NOT_CONTENT_BASED,
        },
        DefaultRow {
            case: "the other FIFO-only attributes, not this one",
            supplied: json!({
                "DeduplicationScope": "messageGroup",
                "FifoThroughputLimit": "perMessageGroupId",
            }),
            expected: NOT_CONTENT_BASED,
        },
        DefaultRow {
            case: "the default supplied explicitly",
            supplied: json!({ CONTENT_BASED_DEDUPLICATION.to_string(): NOT_CONTENT_BASED }),
            expected: NOT_CONTENT_BASED,
        },
        DefaultRow {
            case: "content-based deduplication enabled",
            supplied: json!({ CONTENT_BASED_DEDUPLICATION.to_string(): "true" }),
            expected: "true",
        },
        DefaultRow {
            case: "a queue-level configuration beside the designator",
            supplied: json!({"VisibilityTimeout": "120", "DelaySeconds": "5"}),
            expected: NOT_CONTENT_BASED,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let name = format!("fifo-default-{index}.fifo");
        let mut attributes = row.supplied.clone();
        attributes[FIFO_QUEUE] = json!(DESIGNATES);
        let url = create_ok(&router, &name, &attributes).await;

        // Through the read that asks for everything the queue has.
        let reported = all_attributes(&router, &url).await;
        assert_eq!(
            reported.get(CONTENT_BASED_DEDUPLICATION).map(String::as_str),
            Some(row.expected),
            "case {}: expected {CONTENT_BASED_DEDUPLICATION} to read back as {:?}, got \
             {reported:?}",
            row.case,
            row.expected
        );

        // And through the read that selects the attribute by name: the same value, from the
        // same stored state.
        let selected = ok(
            &router,
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": [CONTENT_BASED_DEDUPLICATION]}),
        )
        .await;
        assert_eq!(
            selected["Attributes"][CONTENT_BASED_DEDUPLICATION].as_str(),
            Some(row.expected),
            "case {}: expected the selected read to report {:?}, got {selected}",
            row.case,
            row.expected
        );

        // The materialised default is part of the queue's attribute set, so a re-create
        // naming the same attributes is the same queue rather than a conflict.
        let again = create_ok(&router, &name, &attributes).await;
        assert_eq!(
            again, url,
            "case {}: expected the re-create to answer the same url",
            row.case
        );

        // And the attribute is settable afterwards, which is what makes the default a
        // starting value rather than a fixed one.
        let flipped = if row.expected == "true" {
            NOT_CONTENT_BASED
        } else {
            "true"
        };
        let _ = ok(
            &router,
            "SetQueueAttributes",
            json!({
                "QueueUrl": url,
                "Attributes": { CONTENT_BASED_DEDUPLICATION.to_string(): flipped },
            }),
        )
        .await;
        let reported = all_attributes(&router, &url).await;
        assert_eq!(
            reported.get(CONTENT_BASED_DEDUPLICATION).map(String::as_str),
            Some(flipped),
            "case {}: expected the set value to be reported, got {reported:?}",
            row.case
        );
    }
}

/// The other half of rule 2: a standard queue does not carry a FIFO-only attribute, so it
/// reports none — and a read selecting only that attribute answers with no `Attributes`
/// member at all rather than with an empty map.
#[tokio::test]
async fn a_standard_queue_carries_no_fifo_only_attribute() {
    let router = emulator();
    let rows = [
        ("no attributes supplied", json!({})),
        ("a queue-level configuration", json!({"DelaySeconds": "10"})),
    ];

    for (index, (case, attributes)) in rows.iter().enumerate() {
        let url = create_ok(&router, &format!("standard-{index}"), attributes).await;
        let reported = all_attributes(&router, &url).await;
        assert!(
            !reported.contains_key(CONTENT_BASED_DEDUPLICATION),
            "case {case}: a standard queue has no {CONTENT_BASED_DEDUPLICATION}, got \
             {reported:?}"
        );
        assert!(
            !reported.contains_key(FIFO_QUEUE),
            "case {case}: a standard queue is designated as nothing, got {reported:?}"
        );

        let selected = ok(
            &router,
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": [CONTENT_BASED_DEDUPLICATION]}),
        )
        .await;
        assert!(
            selected.get("Attributes").is_none(),
            "case {case}: a selection the queue has no value for must report no Attributes \
             member, got {selected}"
        );
    }
}

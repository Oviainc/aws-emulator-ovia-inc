// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT A CALL THAT CHANGES AN EXISTING QUEUE ANSWERS WHEN IT SUPPLIES
//! AN ATTRIBUTE ONLY A CREATE MAY SUPPLY, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in [`super::support`].
//!
//! The rule under test: **an attribute the service HAS but accepts only while the queue is
//! being made is refused, on a call that changes an existing queue, as an invalid attribute
//! VALUE — `InvalidAttributeValue` at HTTP 400 (legacy query code `InvalidAttributeValue`
//! with the `Sender` fault) and the sentence `Invalid value for the parameter <Attribute>.
//! Reason: Modifying queue type is not supported.`, the attribute named exactly as the
//! request's attribute map spelled it — whatever value the call supplied for it, and whether
//! or not that value matches what the queue already is. Nothing about the queue changes.**
//!
//! A refusal of a NAME says the attribute does not exist, which is a different, separately
//! modelled class: this attribute does exist, is published with the create that may supply
//! it, and is reported back by a read — so what the call asked for is the change, and the
//! change is what the refusal is about. A client branching on the modelled class would
//! otherwise be told to drop an attribute AWS tells it it may not modify, and a client
//! matching the `Reason:` sentence would lose the diagnosis entirely.
//!
//! The rows span what the rule generalises over rather than the one recorded request: the
//! KIND of queue the call addresses (standard and FIFO), the VALUE supplied (the designating
//! one — which asks a FIFO queue for no change at all — its opposite, other casings, numeric
//! spellings, a padded one, no text, and a word outside the attribute's vocabulary), and
//! whether the attribute arrives alone or beside an attribute the call would otherwise
//! accept. Each row is asserted over the answer AND over the queue it did not change, and
//! the control rows pin that neither the create that may supply the attribute nor the sibling
//! name class moved.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

use super::support::{
    all_attributes, call, call_in_query_mode, create, create_ok, emulator, error_code, ok,
    QUERY_ERROR_HEADER,
};

/// The attribute only a create may supply: "You can provide this attribute only during queue
/// creation. You can't change it for an existing queue" (`FifoQueue`'s own documentation in
/// `smithy_query(query="operation",operation="CreateQueue",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`).
/// Spelled out here rather than imported, so a wrong constant in the service cannot make a
/// row pass.
const CREATE_ONLY_ATTRIBUTE: &str = "FifoQueue";

/// The one value of that attribute that designates a queue as FIFO.
const DESIGNATES: &str = "true";

/// The refusal class an attribute VALUE the call does not accept carries, and its status.
const INVALID_ATTRIBUTE_VALUE: (u16, &str) = (400, "InvalidAttributeValue");

/// The refusal class a name the service does not have for the call carries: the sibling of
/// the class above, kept apart by this suite's control.
const INVALID_ATTRIBUTE_NAME: &str = "InvalidAttributeName";

/// A name the model's `QueueAttributeName` enum declares nowhere, so no call has it.
const UNDECLARED_NAME: &str = "NotAnAttribute";

/// The fault side the legacy query report names for a refusal the caller caused.
const SENDER_FAULT: &str = "Sender";

/// The one sentence Amazon SQS refuses a change to the queue's type with, spelled out here
/// rather than imported: the fixed prefix, the fixed separator and the fixed `Reason:`
/// sentence, with the attribute name the request supplied as the only variable part.
fn not_modifiable_message(attribute: &str) -> String {
    format!("Invalid value for the parameter {attribute}. Reason: Modifying queue type is not supported.")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Every value a call supplies the create-only attribute with, spanning the forms a client
/// writes one in: the value that designates, its opposite, other casings and spellings, and
/// text the attribute has no meaning for.
const SUPPLIED_VALUES: &[(&str, &str)] = &[
    ("the designating value", DESIGNATES),
    ("the opposite boolean", "false"),
    ("the designating word upper-cased", "TRUE"),
    ("the designating word capitalised", "True"),
    ("a numeric spelling", "1"),
    ("the opposite numeric spelling", "0"),
    ("a padded designating word", " true"),
    ("no text at all", ""),
    ("a word outside the attribute's vocabulary", "yes"),
];

/// The attribute maps an update supplies the create-only attribute in: alone, and beside an
/// attribute the call would accept on its own — sorted before and sorted after it, so the row
/// pins the refusal whichever order the names are visited in.
fn maps_for(value: &str) -> Vec<(String, Value)> {
    vec![
        (
            "alone".to_string(),
            json!({ CREATE_ONLY_ATTRIBUTE.to_string(): value.to_string() }),
        ),
        (
            "beside an attribute sorted before it".to_string(),
            json!({
                CREATE_ONLY_ATTRIBUTE.to_string(): value.to_string(),
                "DelaySeconds": "5",
            }),
        ),
        (
            "beside an attribute sorted after it".to_string(),
            json!({
                CREATE_ONLY_ATTRIBUTE.to_string(): value.to_string(),
                "VisibilityTimeout": "77",
            }),
        ),
    ]
}

/// The queue each row addresses: a standard one and a FIFO one, with what the queue reports
/// before any row ran.
async fn queues(
    router: &MultiServiceRouter,
) -> Vec<(&'static str, String, BTreeMap<String, String>)> {
    let standard = create_ok(router, "immutable-standard", &json!({})).await;
    let fifo = create_ok(
        router,
        "immutable-queue.fifo",
        &json!({ CREATE_ONLY_ATTRIBUTE.to_string(): DESIGNATES }),
    )
    .await;
    let before_standard = all_attributes(router, &standard).await;
    let before_fifo = all_attributes(router, &fifo).await;
    vec![
        ("a standard queue", standard, before_standard),
        ("a FIFO queue", fifo, before_fifo),
    ]
}

/// The rule: whatever the value and whichever kind of queue, the update is refused as an
/// invalid attribute value stating that the queue's type cannot be modified, and the queue is
/// left exactly as it was.
#[tokio::test]
async fn an_update_naming_the_create_only_attribute_is_refused_as_an_invalid_attribute_value() {
    let router = emulator();
    for (kind, url, before) in queues(&router).await {
        for (value_case, value) in SUPPLIED_VALUES {
            for (map_case, attributes) in maps_for(value) {
                let case = format!("{kind} with {value_case} ({value:?}) {map_case}");
                let (status, parsed) = call(
                    &router,
                    "SetQueueAttributes",
                    &json!({"QueueUrl": url, "Attributes": attributes}),
                )
                .await;
                assert_eq!(
                    (status, error_code(&parsed).as_str(), message_of(&parsed)),
                    (
                        INVALID_ATTRIBUTE_VALUE.0,
                        INVALID_ATTRIBUTE_VALUE.1,
                        not_modifiable_message(CREATE_ONLY_ATTRIBUTE)
                    ),
                    "case {case}: expected HTTP {} {} stating {:?}, got {status} with {parsed}",
                    INVALID_ATTRIBUTE_VALUE.0,
                    INVALID_ATTRIBUTE_VALUE.1,
                    not_modifiable_message(CREATE_ONLY_ATTRIBUTE)
                );

                // The refusal is taken before anything is written: the attribute beside it is
                // not stored, the queue's type is what its create made it, and the record's
                // own timestamps did not move.
                let after = all_attributes(&router, &url).await;
                assert_eq!(
                    after, before,
                    "case {case}: a refused update must leave the queue exactly as it was, \
                     got {after:?}"
                );
            }
        }
    }
}

/// The same refusal as a query-compatible SDK sees it: the legacy code of the VALUE class
/// beside the caller's fault, and the same class in the body's discriminator.
#[tokio::test]
async fn the_refusal_reports_the_value_class_to_a_query_compatible_client() {
    let router = emulator();
    for (kind, url, _) in queues(&router).await {
        for (value_case, value) in [("the designating value", DESIGNATES), ("its opposite", "false")]
        {
            let case = format!("{kind} with {value_case} ({value:?})");
            let (status, headers, parsed) = call_in_query_mode(
                &router,
                "SetQueueAttributes",
                &json!({
                    "QueueUrl": url,
                    "Attributes": { CREATE_ONLY_ATTRIBUTE.to_string(): value.to_string() },
                }),
            )
            .await;
            let report = headers
                .get(QUERY_ERROR_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string);
            let expected = format!("{};{SENDER_FAULT}", INVALID_ATTRIBUTE_VALUE.1);
            assert_eq!(
                (status, error_code(&parsed).as_str(), report.clone()),
                (
                    INVALID_ATTRIBUTE_VALUE.0,
                    INVALID_ATTRIBUTE_VALUE.1,
                    Some(expected.clone())
                ),
                "case {case}: expected HTTP {} with the {} class and {QUERY_ERROR_HEADER} \
                 {expected:?}, got {status} with {parsed} and {report:?}",
                INVALID_ATTRIBUTE_VALUE.0,
                INVALID_ATTRIBUTE_VALUE.1
            );
        }
    }
}

/// The controls, which is what keeps the rule from being "refuse everything": the create that
/// MAY supply the attribute still does, the queue it designates still reports itself as FIFO,
/// another attribute of the same update path is still accepted on the queue the rows refused,
/// and a name the service really does not have still draws the sibling NAME class.
#[tokio::test]
async fn the_attribute_is_still_accepted_at_create_and_the_name_class_is_unmoved() {
    let router = emulator();

    // The create may supply it, and the queue reads back as the type it designated.
    let url = create_ok(
        &router,
        "immutable-created.fifo",
        &json!({ CREATE_ONLY_ATTRIBUTE.to_string(): DESIGNATES }),
    )
    .await;
    let reported = all_attributes(&router, &url).await;
    assert_eq!(
        reported.get(CREATE_ONLY_ATTRIBUTE).map(String::as_str),
        Some(DESIGNATES),
        "case the create that may supply the attribute: expected the queue to report itself as \
         FIFO, got {reported:?}"
    );

    // A create naming a value that designates nothing is still a NAME refusal — the attribute
    // the request named is one no create has — so the value class belongs to the update alone.
    let (status, parsed) = create(
        &router,
        "immutable-not-designated",
        &json!({ CREATE_ONLY_ATTRIBUTE.to_string(): "false" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, INVALID_ATTRIBUTE_NAME),
        "case a create supplying a value that designates nothing: expected the name class, got \
         {status} with {parsed}"
    );

    // The update path itself still works on the queue the rows refused.
    let _ = ok(
        &router,
        "SetQueueAttributes",
        json!({"QueueUrl": url, "Attributes": {"DelaySeconds": "5"}}),
    )
    .await;
    let reported = all_attributes(&router, &url).await;
    assert_eq!(
        reported.get("DelaySeconds").map(String::as_str),
        Some("5"),
        "case an attribute the update accepts: expected the set value to be reported, got \
         {reported:?}"
    );

    // And a name the service does not have is still refused as a name, not as a value.
    let (status, parsed) = call(
        &router,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": { UNDECLARED_NAME.to_string(): "1" }}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str(), message_of(&parsed)),
        (
            400,
            INVALID_ATTRIBUTE_NAME,
            format!("Unknown Attribute {UNDECLARED_NAME}.")
        ),
        "case an undeclared attribute name: expected the name class, got {status} with {parsed}"
    );
}

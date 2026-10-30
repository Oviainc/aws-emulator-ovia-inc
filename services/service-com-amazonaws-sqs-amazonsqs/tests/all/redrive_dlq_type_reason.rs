// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the queue-TYPE rule of a dead-letter configuration and the refusal
//! that states it, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: **a `RedrivePolicy` whose `deadLetterTargetArn` names a queue of the
//! other queue type than the queue the policy is written on is refused
//! `InvalidParameterValue` at HTTP 400, with the refused parameter's template carrying the
//! policy document exactly as the request supplied it and the one `Reason` sentence Amazon
//! SQS states that rule in; the attribute is not stored, so a later read reports no value for
//! it. A pair of queues of ONE type is accepted and reads back.**
//!
//! The `Reason` clause is the only part of an `InvalidParameterValue` that says WHICH
//! constraint a document broke, so a client that branches on it is told about the queue types
//! only where that clause is the sentence the service answers this rule with — and not the
//! model's prose ABOUT the constraint, which states the rule for a reader rather than for a
//! matcher.
//!
//! The table spans what the rule generalises over rather than the one recorded request: the
//! DIRECTION of the mismatch (standard source driving to a FIFO dead-letter queue and the
//! reverse) with both same-type pairings as its controls, the SPELLING of the policy document
//! the refusal has to echo back (compact, the members in either order, whitespace around
//! every member, written over several lines), the `maxReceiveCount` it carries (as a number
//! and as a string of digits), the REGION the two queues live in, and both paths that write a
//! queue attribute (`CreateQueue` and `SetQueueAttributes`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    as_map, call_in, create_in, create_ok_in, emulator, error_code, expected_arn_in,
    fifo_attributes, ok_in, REGION,
};

/// The `Reason` sentence Amazon SQS states the dead-letter queue-type rule in, spelled out
/// here rather than imported from the crate under test, so a wrong constant cannot make a row
/// pass. Source: the recorded 400 response of this refusal, whose text is wholly fixed — and
/// which ends in TWO periods.
const TYPE_REASON: &str = "Dead-letter queue must be same type of queue as the source..";

/// A fragment of the model's PROSE about the same constraint
/// (`smithy_query(query="doc", shape="SetQueueAttributesRequest", member="Attributes",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`).
/// It explains the rule to a reader; no refusal may be worded with it.
const MODEL_PROSE_FRAGMENT: &str = "must also be a FIFO queue";

/// The attribute carrying the dead-letter configuration of a source queue.
const REDRIVE_POLICY: &str = "RedrivePolicy";

/// The error code every refused parameter VALUE of this service carries.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The regions the rows are driven in: the harness's own plus one on another continent, so no
/// row can be satisfied by a rule keyed to one region label.
const REGIONS: &[&str] = &[REGION, "eu-central-1"];

/// The standard dead-letter queue every row may name.
const STANDARD_TARGET: &str = "type-reason-dlq";

/// The FIFO dead-letter queue every row may name; a FIFO queue's name carries the `.fifo`
/// suffix.
const FIFO_TARGET: &str = "type-reason-dlq.fifo";

/// The refused-parameter-value template this service answers a refused VALUE in, spelled out
/// rather than imported: the document the request supplied, the parameter it was supplied
/// for, and the one rule that value broke.
fn refusal(supplied: &str, reason: &str) -> String {
    format!("Value {supplied} for parameter {REDRIVE_POLICY} is invalid. Reason: {reason}")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The compact document, as a client that serializes a map writes it.
fn compact(target_arn: &str, count: &Value) -> String {
    json!({"deadLetterTargetArn": target_arn, "maxReceiveCount": count}).to_string()
}

/// The same two members in the other order.
fn reordered(target_arn: &str, count: &Value) -> String {
    format!(r#"{{"maxReceiveCount":{count},"deadLetterTargetArn":"{target_arn}"}}"#)
}

/// The same document with whitespace around every member.
fn spaced(target_arn: &str, count: &Value) -> String {
    format!(r#"{{ "deadLetterTargetArn" : "{target_arn}" , "maxReceiveCount" : {count} }}"#)
}

/// The same document written over several lines, as a configuration file carries it.
fn over_lines(target_arn: &str, count: &Value) -> String {
    serde_json::to_string_pretty(
        &json!({"deadLetterTargetArn": target_arn, "maxReceiveCount": count}),
    )
    .expect("the document serializes")
}

/// How one row writes its policy document out, given the target's ARN and the count.
type Spelling = fn(&str, &Value) -> String;

/// One spelling of the document: what it is, the `maxReceiveCount` it carries as the row
/// spells it, and how it writes the two members out.
struct DocumentSpelling {
    case: &'static str,
    count: Value,
    spell: Spelling,
}

/// One row of the table: the pair of queue types, and the document the write submits.
struct Row {
    /// What the row is about, named in every assertion it makes.
    case: String,
    /// Whether the queue the policy is written ON is a FIFO queue.
    source_fifo: bool,
    /// Whether the dead-letter queue the policy NAMES is a FIFO queue.
    target_fifo: bool,
    /// The `maxReceiveCount` the document carries, as the row spells it.
    count: Value,
    /// How the row writes the document out.
    spell: Spelling,
}

impl Row {
    /// A redrive crosses no queue type, so a pair of different types is the refused one.
    fn refused(&self) -> bool {
        self.source_fifo != self.target_fifo
    }

    /// The document this row submits, naming the dead-letter queue of its target type in
    /// `region`.
    fn document(&self, region: &str) -> String {
        (self.spell)(&target_arn(region, self.target_fifo), &self.count)
    }

    /// The attributes the row's own source queue is created with: what makes a queue a FIFO
    /// queue, or nothing at all.
    fn source_attributes(&self) -> Value {
        if self.source_fifo {
            fifo_attributes(&[])
        } else {
            json!({})
        }
    }

    /// The document an ACCEPTED row must read back as: the service's own rendering of the two
    /// members, with the count as the number its documentation calls it ("the number of times
    /// a message is delivered to the source queue before being moved to the dead-letter
    /// queue").
    fn stored(&self, region: &str) -> String {
        let count = self
            .count
            .as_i64()
            .or_else(|| {
                self.count
                    .as_str()
                    .and_then(|digits| digits.trim().parse::<i64>().ok())
            })
            .unwrap_or_default();
        json!({
            "deadLetterTargetArn": target_arn(region, self.target_fifo),
            "maxReceiveCount": count,
        })
        .to_string()
    }
}

/// The ARN of the dead-letter queue of the given type in `region`.
fn target_arn(region: &str, fifo: bool) -> String {
    expected_arn_in(region, if fifo { FIFO_TARGET } else { STANDARD_TARGET })
}

/// The name a row's own source queue carries, with the `.fifo` suffix a FIFO queue's name
/// needs.
fn source_name(prefix: &str, index: usize, fifo: bool) -> String {
    let suffix = if fifo { ".fifo" } else { "" };
    format!("{prefix}-{index}{suffix}")
}

/// Create both dead-letter queues the rows may name, in `region`.
async fn create_targets(router: &MultiServiceRouter, region: &str) {
    let _ = create_ok_in(router, region, STANDARD_TARGET, &json!({})).await;
    let _ = create_ok_in(router, region, FIFO_TARGET, &fifo_attributes(&[])).await;
}

/// What a read of just the `RedrivePolicy` attribute of the queue at `url` answers.
async fn redrive_policy_read(router: &MultiServiceRouter, region: &str, url: &str) -> Value {
    ok_in(
        router,
        region,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": [REDRIVE_POLICY]}),
    )
    .await
}

/// The `RedrivePolicy` such a read reports, or `None` where the queue carries no value for
/// it. A queue with no value for every name the request selected reports NO `Attributes`
/// member at all, which is what [`assert_reports_no_policy`] pins.
fn policy_of(read: &Value) -> Option<String> {
    read.get("Attributes")
        .and_then(|attributes| as_map(attributes).get(REDRIVE_POLICY).cloned())
}

/// Assert that the queue at `url` reports no dead-letter configuration whatsoever: the read
/// selected the one attribute the queue has no value for, so its answer carries no
/// `Attributes` member — not an empty map, which a client would read as a configuration it
/// cannot see into.
async fn assert_reports_no_policy(
    router: &MultiServiceRouter,
    region: &str,
    url: &str,
    case: &str,
) {
    let read = redrive_policy_read(router, region, url).await;
    assert!(
        read.get("Attributes").is_none(),
        "case {case} in {region}: expected no Attributes member at all, got {read}"
    );
}

/// Every pairing of the two queue types, each spelled in every form a client might write the
/// document in: the two mismatched directions the rule refuses and the two same-type pairings
/// it admits as their controls.
fn rows() -> Vec<Row> {
    // Every form a client might write ONE document in, and both spellings of the count: as a
    // number and as a string of digits. The documented default is among the values
    // ("maxReceiveCount – ... Default: 10", the `RedrivePolicy` attribute's own
    // documentation).
    let spelling = |case: &'static str, count: Value, spell: Spelling| DocumentSpelling {
        case,
        count,
        spell,
    };
    let spellings = [
        spelling("a compact document", json!(3), compact),
        spelling("a count of one delivery", json!(1), compact),
        spelling("a count spelled as a string of digits", json!("3"), compact),
        spelling("the documented default count", json!(10), compact),
        spelling("the members in the other order", json!(3), reordered),
        spelling("whitespace around every member", json!(3), spaced),
        spelling("a document written over several lines", json!(3), over_lines),
    ];
    let pairs = [
        ("a standard source driving to a FIFO dead-letter queue", false, true),
        ("a FIFO source driving to a standard dead-letter queue", true, false),
        ("a standard source driving to a standard dead-letter queue", false, false),
        ("a FIFO source driving to a FIFO dead-letter queue", true, true),
    ];
    let mut rows = Vec::new();
    for (pair, source_fifo, target_fifo) in pairs {
        for spelling in &spellings {
            rows.push(Row {
                case: format!("{pair}, {}", spelling.case),
                source_fifo,
                target_fifo,
                count: spelling.count.clone(),
                spell: spelling.spell,
            });
        }
    }
    rows
}

/// `SetQueueAttributes` refuses a type-mismatched `RedrivePolicy` with the queue-type rule's
/// own sentence and the document echoed back verbatim, and stores nothing; a same-type pair is
/// stored and reads back.
#[tokio::test]
async fn set_queue_attributes_refuses_a_type_mismatched_dead_letter_target() {
    for region in REGIONS {
        let router = emulator();
        create_targets(&router, region).await;

        for (index, row) in rows().into_iter().enumerate() {
            let case = &row.case;
            let document = row.document(region);
            let url = create_ok_in(
                &router,
                region,
                &source_name("type-source", index, row.source_fifo),
                &row.source_attributes(),
            )
            .await;
            // Nothing is configured yet, so a read of that one attribute reports nothing.
            assert_reports_no_policy(&router, region, &url, case).await;

            let (status, parsed) = call_in(
                &router,
                region,
                "SetQueueAttributes",
                &json!({"QueueUrl": url, "Attributes": {REDRIVE_POLICY: document}}),
            )
            .await;
            let read = redrive_policy_read(&router, region, &url).await;
            let stored = policy_of(&read);

            if row.refused() {
                assert_eq!(
                    (status, error_code(&parsed).as_str(), message_of(&parsed)),
                    (
                        400,
                        INVALID_PARAMETER_VALUE,
                        refusal(&document, TYPE_REASON)
                    ),
                    "case {case} in {region}: expected the queue-type rule's own sentence with \
                     the document echoed back, got {status} with {parsed}"
                );
                assert!(
                    !message_of(&parsed).contains(MODEL_PROSE_FRAGMENT),
                    "case {case}: the refusal must not be worded as the model's prose, got {:?}",
                    message_of(&parsed)
                );
                assert_eq!(
                    stored, None,
                    "case {case} in {region}: a refused policy must leave nothing stored, got \
                     {stored:?}"
                );
                assert_reports_no_policy(&router, region, &url, case).await;
            } else {
                assert_eq!(
                    status, 200,
                    "case {case} in {region}: expected the same-type pair accepted, got {status} \
                     with {parsed}"
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(row.stored(region).as_str()),
                    "case {case} in {region}: expected the accepted policy reported with its \
                     target intact, got {stored:?}"
                );
            }
        }
    }
}

/// `CreateQueue` judges the same pair the same way and answers the same sentence, and a
/// refused create leaves no queue behind at all.
#[tokio::test]
async fn create_queue_refuses_a_type_mismatched_dead_letter_target() {
    for region in REGIONS {
        let router = emulator();
        create_targets(&router, region).await;

        for (index, row) in rows().into_iter().enumerate() {
            let case = &row.case;
            let document = row.document(region);
            let name = source_name("type-created", index, row.source_fifo);
            let mut attributes = row.source_attributes();
            attributes[REDRIVE_POLICY] = json!(document);

            let (status, parsed) = create_in(&router, region, &name, &attributes).await;

            if row.refused() {
                assert_eq!(
                    (status, error_code(&parsed).as_str(), message_of(&parsed)),
                    (
                        400,
                        INVALID_PARAMETER_VALUE,
                        refusal(&document, TYPE_REASON)
                    ),
                    "case {case} in {region}: expected the queue-type rule's own sentence with \
                     the document echoed back, got {status} with {parsed}"
                );
                let (status, parsed) =
                    call_in(&router, region, "GetQueueUrl", &json!({"QueueName": name})).await;
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "QueueDoesNotExist"),
                    "case {case} in {region}: a refused create must leave no queue behind, got \
                     {status} with {parsed}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case} in {region}: expected the same-type pair accepted, got {status} \
                     with {parsed}"
                );
                let url = parsed["QueueUrl"].as_str().unwrap_or_default().to_string();
                let stored = policy_of(&redrive_policy_read(&router, region, &url).await);
                assert_eq!(
                    stored.as_deref(),
                    Some(row.stored(region).as_str()),
                    "case {case} in {region}: expected the created queue to report the policy it \
                     took, got {stored:?}"
                );
            }
        }
    }
}

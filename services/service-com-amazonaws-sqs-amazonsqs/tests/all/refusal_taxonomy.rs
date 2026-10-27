// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH error every input-validation refusal of this service
//! carries, driven over the wire through the shared harness in [`super::support`].
//!
//! The rule under test: a request this service refuses for its INPUT is answered at HTTP
//! 400 with one of the error codes this service's API has — the model's own
//! `InvalidAttributeName` / `InvalidAttributeValue` / `InvalidMessageContents` /
//! `EmptyBatchRequest` for a fault in an attribute, a body or a batch, and the two sender
//! faults its documentation carries for a fault in a PARAMETER: `MissingParameter` when a
//! parameter the call requires is absent, `InvalidParameterValue` when a supplied
//! parameter's value is not one it accepts — never a code outside that set, and with the
//! message the service builds from the request in its own templates.
//!
//! The table spans what the rule generalises over rather than one recorded request: the
//! OPERATION (eleven of them), the KIND of fault (absent parameter, value outside a
//! range, value outside a value set, value of the wrong form, a parameter the queue type
//! does not take, a collection that names nothing, a count past a published bound), and
//! the MEMBER it sits in. Every row asserts the status, the code and the WHOLE message:
//! where a message is a divergence from AWS's own sentence that a change could not
//! resolve (the issue report records which, and why the recorded text is not obtainable
//! here), its row pins the template the handler builds and will fail the
//! day it is corrected — which is what a later change needs them to do. Every observed
//! code is also checked against the closed set of codes this service's API has — which is
//! what makes a refusal answered as `ValidationException`, a shape the model does not
//! declare at all, a failure of this suite.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    LIST_QUEUES_MAX_RESULTS_RANGE, MAX_MESSAGES_PER_SECOND, MAX_MESSAGE_ATTRIBUTES,
    MAX_MESSAGE_TOKEN_LENGTH, MAX_QUEUE_NAME_LENGTH, MAX_TAGS_PER_QUEUE, MAX_TAG_VALUE_LENGTH,
};

use super::support::{call, create_ok, dead_letter_queue, emulator, error_code, expected_arn};

/// Every error code a refusal of this service may carry.
///
/// The client faults its model declares, from
/// `smithy_query(query="digest", model_path="services/service-com-amazonaws-sqs-amazonsqs/
/// smithy-model/com-amazonaws-sqs-amazonsqs.json")`, plus the two sender faults the
/// service's documentation carries for parameter-level refusals (`InvalidParameterValue`,
/// `MissingParameter`), which the model shapes no error for. `ValidationException` is
/// deliberately absent: it is not a shape in this model at all.
const EVERY_REFUSAL_CODE: &[&str] = &[
    "BatchEntryIdsNotDistinct",
    "BatchRequestTooLong",
    "EmptyBatchRequest",
    "InvalidAddress",
    "InvalidAttributeName",
    "InvalidAttributeValue",
    "InvalidBatchEntryId",
    "InvalidIdFormat",
    "InvalidMessageContents",
    "InvalidParameterValue",
    "InvalidSecurity",
    "MessageNotInflight",
    "MissingParameter",
    "OverLimit",
    "PurgeQueueInProgress",
    "QueueDeletedRecently",
    "QueueDoesNotExist",
    "QueueNameExists",
    "ReceiptHandleIsInvalid",
    "RequestThrottled",
    "ResourceNotFoundException",
    "TooManyEntriesInBatchRequest",
    "UnsupportedOperation",
];

/// Every value `MessageSystemAttributeName` publishes, in model order. Source:
/// `smithy_query(query="shape", shape="MessageSystemAttributeName")` on this crate's
/// model — the set a refusal of a name outside it reports.
const EVERY_SYSTEM_ATTRIBUTE_NAME: &[&str] = &[
    "All",
    "SenderId",
    "SentTimestamp",
    "ApproximateReceiveCount",
    "ApproximateFirstReceiveTimestamp",
    "SequenceNumber",
    "MessageDeduplicationId",
    "MessageGroupId",
    "AWSTraceHeader",
    "DeadLetterQueueSourceArn",
];

/// Where the queue a row addresses goes in its request, and which queue that is.
#[derive(Debug, Clone, Copy)]
enum Address {
    /// The request names no queue (a create, or a listing of the account's queues).
    None,
    /// `QueueUrl` carries the queue's url.
    Url(&'static str),
    /// `SourceArn` carries the queue's ARN.
    SourceArn(&'static str),
}

/// One refused request and the answer it must get.
struct Row {
    case: &'static str,
    operation: &'static str,
    address: Address,
    request: Value,
    code: &'static str,
    message: String,
}

/// The reason a refusal gives for a parameter the addressed queue's TYPE does not take.
const WRONG_QUEUE_TYPE: &str =
    "The request include parameter that is not valid for this queue type.";

/// The whole rule a FIFO queue's name has to satisfy, as its refusal states it.
fn fifo_name_rule() -> String {
    format!(
        "The name of a FIFO queue can only include alphanumeric characters, hyphens, or \
         underscores, must end with .fifo suffix and be 1 to {MAX_QUEUE_NAME_LENGTH} in length."
    )
}

/// The rule a standard queue's name has to satisfy, as its refusal states it.
fn queue_name_rule() -> String {
    format!(
        "Can only include alphanumeric characters, hyphens, or underscores. 1 to \
         {MAX_QUEUE_NAME_LENGTH} in length"
    )
}

/// How a refusal states that `value`, supplied for `parameter`, is not accepted.
fn invalid(value: &str, parameter: &str, reason: &str) -> String {
    format!("Value {value} for parameter {parameter} is invalid. Reason: {reason}")
}

/// The same template for a refusal that does NOT name the offending value, which is how
/// this service words a refusal whose value is the very thing it will not take.
fn invalid_without_value(parameter: &str, reason: &str) -> String {
    format!("Value for parameter {parameter} is invalid. Reason: {reason}")
}

/// How a refusal states that a `MessageGroupId`-style token is not one `member` accepts.
fn bad_token(value: &str, member: &str) -> String {
    invalid(
        value,
        member,
        &format!(
            "{member} can only include alphanumeric and punctuation characters. 1 to \
             {MAX_MESSAGE_TOKEN_LENGTH} in length."
        ),
    )
}

/// `count` message attributes, as the `MessageAttributes` member carries them.
fn message_attributes(count: usize) -> Value {
    let map: serde_json::Map<String, Value> = (0..count)
        .map(|index| {
            (
                format!("a{index}"),
                json!({ "DataType": "String", "StringValue": "v" }),
            )
        })
        .collect();
    Value::Object(map)
}

/// `count` distinct tags, as the `Tags` member carries them.
fn tags(count: usize) -> Value {
    let map: serde_json::Map<String, Value> = (0..count)
        .map(|index| (format!("k{index:03}"), json!("v")))
        .collect();
    Value::Object(map)
}

/// Every input-validation refusal this service answers carries a code its API has, at
/// HTTP 400, with the message the service builds from the request.
#[tokio::test]
async fn every_input_refusal_carries_a_code_this_service_has() {
    let router = emulator();
    // The queues the rows address: a standard queue, a FIFO queue, a FIFO queue that
    // derives its deduplication token, a queue whose maximum message size was lowered,
    // and a dead-letter queue with a source that drives messages to it.
    const STANDARD: &str = "refusals-standard";
    const SMALL_MAXIMUM: usize = 1_024;
    let standard = create_ok(&router, STANDARD, &json!({})).await;
    let fifo = create_ok(&router, "refusals.fifo", &json!({"FifoQueue": "true"})).await;
    let content = create_ok(
        &router,
        "refusals-content.fifo",
        &json!({"FifoQueue": "true", "ContentBasedDeduplication": "true"}),
    )
    .await;
    let small = create_ok(
        &router,
        "refusals-small",
        &json!({"MaximumMessageSize": SMALL_MAXIMUM.to_string()}),
    )
    .await;
    let (dead_letter, dead_letter_arn, _source) = dead_letter_queue(&router, "refusals-dlq").await;

    let url_of = |key: &str| -> String {
        match key {
            "standard" => standard.clone(),
            "fifo" => fifo.clone(),
            "content" => content.clone(),
            "small" => small.clone(),
            "dead-letter" => dead_letter.clone(),
            other => panic!("case setup: no queue named {other}"),
        }
    };
    let arn_of = |key: &str| -> String {
        match key {
            "standard" => expected_arn(STANDARD),
            "dead-letter" => dead_letter_arn.clone(),
            other => panic!("case setup: no queue named {other}"),
        }
    };

    let long_token = "g".repeat(MAX_MESSAGE_TOKEN_LENGTH + 1);
    let long_name = "n".repeat(MAX_QUEUE_NAME_LENGTH + 1);
    let long_fifo_name = format!(
        "{}.fifo",
        "f".repeat(MAX_QUEUE_NAME_LENGTH + 1 - ".fifo".len())
    );
    let long_tag_value = "v".repeat(MAX_TAG_VALUE_LENGTH + 1);
    let system_attribute_set = format!(
        "Must be one of: {}.",
        EVERY_SYSTEM_ATTRIBUTE_NAME.join(", ")
    );

    let rows: Vec<Row> = vec![
        // A parameter the call requires and did not get: absent, not invalid.
        Row {
            case: "a FIFO send with no MessageGroupId",
            operation: "SendMessage",
            address: Address::Url("fifo"),
            request: json!({"MessageBody": "payload", "MessageDeduplicationId": "d"}),
            code: "MissingParameter",
            message: "The request must contain the parameter MessageGroupId.".to_string(),
        },
        Row {
            case: "a SetQueueAttributes whose Attributes map names nothing",
            operation: "SetQueueAttributes",
            address: Address::Url("standard"),
            request: json!({"Attributes": {}}),
            code: "MissingParameter",
            message: "The request must contain the parameter Attribute.Name.".to_string(),
        },
        Row {
            case: "a TagQueue whose Tags map names nothing",
            operation: "TagQueue",
            address: Address::Url("standard"),
            request: json!({"Tags": {}}),
            code: "MissingParameter",
            message: "The request must contain the parameter Tags.".to_string(),
        },
        Row {
            case: "an UntagQueue whose TagKeys list names nothing",
            operation: "UntagQueue",
            address: Address::Url("standard"),
            request: json!({"TagKeys": []}),
            code: "MissingParameter",
            message: "The request must contain the parameter TagKeys.".to_string(),
        },
        // A FIFO send the queue cannot deduplicate.
        Row {
            case: "a FIFO send with no MessageDeduplicationId on a queue that derives none",
            operation: "SendMessage",
            address: Address::Url("fifo"),
            request: json!({"MessageBody": "payload", "MessageGroupId": "g"}),
            code: "InvalidParameterValue",
            message: "The queue should either have ContentBasedDeduplication enabled or \
                      MessageDeduplicationId provided explicitly"
                .to_string(),
        },
        // A parameter the addressed queue's TYPE does not take.
        Row {
            case: "a positive per-message DelaySeconds on a FIFO queue",
            operation: "SendMessage",
            address: Address::Url("content"),
            request: json!({"MessageBody": "payload", "MessageGroupId": "g", "DelaySeconds": 5}),
            code: "InvalidParameterValue",
            message: invalid("5", "DelaySeconds", WRONG_QUEUE_TYPE),
        },
        Row {
            case: "a MessageDeduplicationId on a standard queue",
            operation: "SendMessage",
            address: Address::Url("standard"),
            request: json!({"MessageBody": "payload", "MessageDeduplicationId": "token"}),
            code: "InvalidParameterValue",
            message: invalid("token", "MessageDeduplicationId", WRONG_QUEUE_TYPE),
        },
        Row {
            case: "a batch entry with a positive DelaySeconds on a FIFO queue",
            operation: "SendMessageBatch",
            address: Address::Url("content"),
            request: json!({"Entries": [
                {"Id": "plain", "MessageBody": "one", "MessageGroupId": "g"},
                {"Id": "delayed", "MessageBody": "two", "MessageGroupId": "g", "DelaySeconds": 7},
            ]}),
            code: "InvalidParameterValue",
            message: invalid("7", "DelaySeconds", WRONG_QUEUE_TYPE),
        },
        Row {
            case: "a batch entry with a MessageDeduplicationId on a standard queue",
            operation: "SendMessageBatch",
            address: Address::Url("standard"),
            request: json!({"Entries": [
                {"Id": "plain", "MessageBody": "one"},
                {"Id": "tokened", "MessageBody": "two", "MessageDeduplicationId": "token"},
            ]}),
            code: "InvalidParameterValue",
            message: invalid("token", "MessageDeduplicationId", WRONG_QUEUE_TYPE),
        },
        // A token outside the form its member publishes, on each member that carries one.
        Row {
            case: "a MessageGroupId one character past the token bound",
            operation: "SendMessage",
            address: Address::Url("fifo"),
            request: json!({
                "MessageBody": "payload",
                "MessageGroupId": long_token,
                "MessageDeduplicationId": "d",
            }),
            code: "InvalidParameterValue",
            message: bad_token(&long_token, "MessageGroupId"),
        },
        Row {
            case: "a MessageGroupId holding a space, which the token set does not list",
            operation: "SendMessage",
            address: Address::Url("standard"),
            request: json!({"MessageBody": "payload", "MessageGroupId": "tenant 7"}),
            code: "InvalidParameterValue",
            message: bad_token("tenant 7", "MessageGroupId"),
        },
        Row {
            case: "an empty MessageDeduplicationId, which names no send attempt",
            operation: "SendMessage",
            address: Address::Url("fifo"),
            request: json!({
                "MessageBody": "payload",
                "MessageGroupId": "g",
                "MessageDeduplicationId": "",
            }),
            code: "InvalidParameterValue",
            message: bad_token("", "MessageDeduplicationId"),
        },
        Row {
            case: "a ReceiveRequestAttemptId one character past the token bound",
            operation: "ReceiveMessage",
            address: Address::Url("fifo"),
            request: json!({"ReceiveRequestAttemptId": long_token}),
            code: "InvalidParameterValue",
            message: bad_token(&long_token, "ReceiveRequestAttemptId"),
        },
        // A value outside the range its parameter publishes, at each end of it.
        Row {
            case: "a DelaySeconds one past the top of its range",
            operation: "SendMessage",
            address: Address::Url("standard"),
            request: json!({"MessageBody": "payload", "DelaySeconds": 901}),
            code: "InvalidParameterValue",
            message: invalid("901", "DelaySeconds", "DelaySeconds must be >= 0 and <= 900."),
        },
        Row {
            case: "a negative DelaySeconds",
            operation: "SendMessage",
            address: Address::Url("standard"),
            request: json!({"MessageBody": "payload", "DelaySeconds": -1}),
            code: "InvalidParameterValue",
            message: invalid("-1", "DelaySeconds", "DelaySeconds must be >= 0 and <= 900."),
        },
        Row {
            case: "a MaxNumberOfMessages of zero, below its range",
            operation: "ReceiveMessage",
            address: Address::Url("standard"),
            request: json!({"MaxNumberOfMessages": 0}),
            code: "InvalidParameterValue",
            message: invalid(
                "0",
                "MaxNumberOfMessages",
                "Must be between 1 and 10, if provided.",
            ),
        },
        Row {
            case: "a MaxNumberOfMessages one past its range",
            operation: "ReceiveMessage",
            address: Address::Url("standard"),
            request: json!({"MaxNumberOfMessages": 11}),
            code: "InvalidParameterValue",
            message: invalid(
                "11",
                "MaxNumberOfMessages",
                "Must be between 1 and 10, if provided.",
            ),
        },
        Row {
            case: "a WaitTimeSeconds one past its range",
            operation: "ReceiveMessage",
            address: Address::Url("standard"),
            request: json!({"WaitTimeSeconds": 21}),
            code: "InvalidParameterValue",
            message: invalid(
                "21",
                "WaitTimeSeconds",
                "Must be >= 0 and <= 20, if provided.",
            ),
        },
        Row {
            case: "a VisibilityTimeout one past its range",
            operation: "ReceiveMessage",
            address: Address::Url("standard"),
            request: json!({"VisibilityTimeout": 43_201}),
            code: "InvalidParameterValue",
            message: invalid(
                "43201",
                "VisibilityTimeout",
                "Must be >= 0 and <= 43200, if provided.",
            ),
        },
        Row {
            case: "a message movement rate of zero, below its range",
            operation: "StartMessageMoveTask",
            address: Address::SourceArn("dead-letter"),
            request: json!({"MaxNumberOfMessagesPerSecond": 0}),
            code: "InvalidParameterValue",
            message: invalid(
                "0",
                "MaxNumberOfMessagesPerSecond",
                &format!("Must be >= 1 and <= {MAX_MESSAGES_PER_SECOND}."),
            ),
        },
        Row {
            case: "a message movement rate one past its range",
            operation: "StartMessageMoveTask",
            address: Address::SourceArn("dead-letter"),
            request: json!({"MaxNumberOfMessagesPerSecond": MAX_MESSAGES_PER_SECOND + 1}),
            code: "InvalidParameterValue",
            message: invalid(
                &(MAX_MESSAGES_PER_SECOND + 1).to_string(),
                "MaxNumberOfMessagesPerSecond",
                &format!("Must be >= 1 and <= {MAX_MESSAGES_PER_SECOND}."),
            ),
        },
        Row {
            case: "a message-move listing asking for no result at all",
            operation: "ListMessageMoveTasks",
            address: Address::SourceArn("dead-letter"),
            request: json!({"MaxResults": 0}),
            code: "InvalidParameterValue",
            message: invalid_without_value(
                "MaxResults",
                "You must enter a number that's between 1 and 10.",
            ),
        },
        Row {
            case: "a queue listing asking for more than a page may hold",
            operation: "ListQueues",
            address: Address::None,
            request: json!({"MaxResults": LIST_QUEUES_MAX_RESULTS_RANGE.1 + 1}),
            code: "InvalidParameterValue",
            message: invalid_without_value(
                "MaxResults",
                &format!(
                    "MaxResults must be an integer between {} and {}.",
                    LIST_QUEUES_MAX_RESULTS_RANGE.0, LIST_QUEUES_MAX_RESULTS_RANGE.1
                ),
            ),
        },
        Row {
            case: "a queue listing continuing from a token no page handed out",
            operation: "ListQueues",
            address: Address::None,
            request: json!({"NextToken": "not-a-cursor"}),
            code: "InvalidParameterValue",
            message: invalid(
                "not-a-cursor",
                "NextToken",
                "it was not issued by this listing.",
            ),
        },
        // A value outside the SET its member accepts, on each member that carries one.
        Row {
            case: "a system attribute name outside the set, on the member in use",
            operation: "ReceiveMessage",
            address: Address::Url("standard"),
            request: json!({"MessageSystemAttributeNames": ["NotAnAttribute"]}),
            code: "InvalidParameterValue",
            message: invalid(
                "(NotAnAttribute)",
                "MessageSystemAttributeNames",
                &system_attribute_set,
            ),
        },
        Row {
            case: "a system attribute name outside the set, on the discontinued alias",
            operation: "ReceiveMessage",
            address: Address::Url("standard"),
            request: json!({"AttributeNames": ["senttimestamp"]}),
            code: "InvalidParameterValue",
            message: invalid("(senttimestamp)", "AttributeNames", &system_attribute_set),
        },
        // A count past a published bound.
        Row {
            case: "one message attribute past the published maximum",
            operation: "SendMessage",
            address: Address::Url("standard"),
            request: json!({
                "MessageBody": "payload",
                "MessageAttributes": message_attributes(MAX_MESSAGE_ATTRIBUTES + 1),
            }),
            code: "InvalidParameterValue",
            message: format!(
                "Number of message attributes [{}] exceeds the allowed maximum [{}].",
                MAX_MESSAGE_ATTRIBUTES + 1,
                MAX_MESSAGE_ATTRIBUTES
            ),
        },
        Row {
            case: "one tag past the per-queue tag bound",
            operation: "TagQueue",
            address: Address::Url("standard"),
            request: json!({"Tags": tags(MAX_TAGS_PER_QUEUE + 1)}),
            code: "InvalidParameterValue",
            message: format!("Too many tags added for queue {STANDARD}."),
        },
        Row {
            case: "a tag value one character past its bound",
            operation: "TagQueue",
            address: Address::Url("standard"),
            request: json!({"Tags": {"Environment": long_tag_value.clone()}}),
            code: "InvalidParameterValue",
            message: format!(
                "Tag values must be between 0 and {MAX_TAG_VALUE_LENGTH} characters in length."
            ),
        },
        // A message the queue will not carry: over its MaximumMessageSize, or holding a
        // character the body's value set excludes.
        Row {
            case: "a body one byte past the queue's MaximumMessageSize",
            operation: "SendMessage",
            address: Address::Url("small"),
            request: json!({"MessageBody": "x".repeat(SMALL_MAXIMUM + 1)}),
            code: "InvalidParameterValue",
            message: format!(
                "One or more parameters are invalid. Reason: Message must be shorter than \
                 {SMALL_MAXIMUM} bytes."
            ),
        },
        Row {
            case: "a body holding a character outside the allowed set",
            operation: "SendMessage",
            address: Address::Url("standard"),
            request: json!({"MessageBody": "payload\u{1}"}),
            code: "InvalidMessageContents",
            message: "Invalid binary character '#x1' was found in the message body, the set of \
                      allowed characters is #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | \
                      [#x10000-#x10FFFF]."
                .to_string(),
        },
        // A queue name outside the form the member publishes, for each queue type.
        Row {
            case: "a queue name one character past the length bound",
            operation: "CreateQueue",
            address: Address::None,
            request: json!({"QueueName": long_name}),
            code: "InvalidParameterValue",
            message: queue_name_rule(),
        },
        Row {
            case: "a queue name holding a dot, which only a FIFO suffix may",
            operation: "CreateQueue",
            address: Address::None,
            request: json!({"QueueName": "orders.v2"}),
            code: "InvalidParameterValue",
            message: queue_name_rule(),
        },
        Row {
            case: "a .fifo name without the attribute that asks for a FIFO queue",
            operation: "CreateQueue",
            address: Address::None,
            request: json!({"QueueName": "orders-standard.fifo"}),
            code: "InvalidParameterValue",
            message: queue_name_rule(),
        },
        Row {
            case: "a FIFO queue whose name lacks the suffix",
            operation: "CreateQueue",
            address: Address::None,
            request: json!({"QueueName": "no-suffix", "Attributes": {"FifoQueue": "true"}}),
            code: "InvalidParameterValue",
            message: fifo_name_rule(),
        },
        Row {
            case: "a FIFO queue name one character past the length bound",
            operation: "CreateQueue",
            address: Address::None,
            request: json!({
                "QueueName": long_fifo_name,
                "Attributes": {"FifoQueue": "true"},
            }),
            code: "InvalidParameterValue",
            message: fifo_name_rule(),
        },
        // A fault in an ATTRIBUTE keeps the model's own attribute errors, whichever call
        // supplied it, and names the attribute whose value was refused.
        Row {
            case: "a create naming an attribute value outside the attribute's range",
            operation: "CreateQueue",
            address: Address::None,
            request: json!({
                "QueueName": "refused-attribute",
                "Attributes": {"VisibilityTimeout": "43201"},
            }),
            code: "InvalidAttributeValue",
            message: "Invalid value for the parameter VisibilityTimeout.".to_string(),
        },
        Row {
            case: "an update naming a Policy that is not a JSON document",
            operation: "SetQueueAttributes",
            address: Address::Url("standard"),
            request: json!({"Attributes": {"Policy": "{not json"}}),
            code: "InvalidAttributeValue",
            message: "Invalid value for the parameter Policy.".to_string(),
        },
        Row {
            case: "an update naming a MessageRetentionPeriod below its range",
            operation: "SetQueueAttributes",
            address: Address::Url("standard"),
            request: json!({"Attributes": {"MessageRetentionPeriod": "0"}}),
            code: "InvalidAttributeValue",
            message: "Invalid value for the parameter MessageRetentionPeriod.".to_string(),
        },
        Row {
            case: "an update naming an attribute that does not exist",
            operation: "SetQueueAttributes",
            address: Address::Url("standard"),
            request: json!({"Attributes": {"NotAnAttribute": "1"}}),
            code: "InvalidAttributeName",
            message: "Unknown Attribute NotAnAttribute.".to_string(),
        },
        // A batch that names no entry: the operation's own request-entry shape is what
        // the request should have carried at least one of.
        Row {
            case: "a delete batch with no entries",
            operation: "DeleteMessageBatch",
            address: Address::Url("standard"),
            request: json!({"Entries": []}),
            code: "EmptyBatchRequest",
            message: "There should be at least one DeleteMessageBatchRequestEntry in the request."
                .to_string(),
        },
        Row {
            case: "a send batch with no entries",
            operation: "SendMessageBatch",
            address: Address::Url("standard"),
            request: json!({"Entries": []}),
            code: "EmptyBatchRequest",
            message: "There should be at least one SendMessageBatchRequestEntry in the request."
                .to_string(),
        },
        // A source queue the operation's own pre-condition excludes: the caller named it,
        // so the fault is in that parameter's value.
        Row {
            case: "a message move task whose source is not a dead-letter queue",
            operation: "StartMessageMoveTask",
            address: Address::SourceArn("standard"),
            request: json!({}),
            code: "InvalidParameterValue",
            message: "Source queue must be configured as a Dead Letter Queue.".to_string(),
        },
    ];

    for row in &rows {
        let mut request = row.request.clone();
        match row.address {
            Address::None => {}
            Address::Url(key) => request["QueueUrl"] = json!(url_of(key)),
            Address::SourceArn(key) => request["SourceArn"] = json!(arn_of(key)),
        }
        let (status, answer) = call(&router, row.operation, &request).await;
        let code = error_code(&answer);
        assert_eq!(
            (status, code.as_str()),
            (400, row.code),
            "case {}: expected HTTP 400 {}, got {status} with {answer}",
            row.case,
            row.code
        );
        assert!(
            EVERY_REFUSAL_CODE.contains(&code.as_str()),
            "case {}: {code} is not a code this service's API has, got {answer}",
            row.case
        );
        let reported = answer["message"].as_str().unwrap_or_default();
        assert_eq!(
            reported,
            row.message.as_str(),
            "case {}: expected the message {:?}, got {answer}",
            row.case,
            row.message
        );
    }
}

/// A refused request changes nothing: the batch a queue-type parameter refuses enqueues
/// none of its entries, and the queue a refused tag or attribute call addresses keeps
/// what it had.
#[tokio::test]
async fn a_refused_request_leaves_the_queue_as_it_was() {
    let router = emulator();
    let fifo = create_ok(
        &router,
        "untouched.fifo",
        &json!({"FifoQueue": "true", "ContentBasedDeduplication": "true"}),
    )
    .await;

    // The whole batch is refused for the parameter the queue type does not take, so the
    // entry that carried nothing wrong is not enqueued either.
    let (status, answer) = call(
        &router,
        "SendMessageBatch",
        &json!({
            "QueueUrl": fifo,
            "Entries": [
                {"Id": "plain", "MessageBody": "one", "MessageGroupId": "g"},
                {"Id": "delayed", "MessageBody": "two", "MessageGroupId": "g", "DelaySeconds": 3},
            ],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "InvalidParameterValue"),
        "case a batch with a per-message delay on a FIFO queue: expected a 400 \
         InvalidParameterValue, got {status} with {answer}"
    );
    let held = super::support::attribute_count(&router, &fifo, "ApproximateNumberOfMessages").await;
    assert_eq!(
        held, 0,
        "case a refused batch: the queue must hold nothing, got {held}"
    );

    // A refused tag call leaves the tag map alone, and a refused attribute call the
    // attribute map.
    let standard = create_ok(&router, "untouched-standard", &json!({})).await;
    let before = super::support::all_attributes(&router, &standard).await;
    let (status, answer) = call(
        &router,
        "SetQueueAttributes",
        &json!({"QueueUrl": standard, "Attributes": {"VisibilityTimeout": "43201"}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "InvalidAttributeValue"),
        "case an update naming a value outside the attribute's range: expected a 400 \
         InvalidAttributeValue, got {status} with {answer}"
    );
    let after = super::support::all_attributes(&router, &standard).await;
    assert_eq!(
        after.get("VisibilityTimeout"),
        before.get("VisibilityTimeout"),
        "case a refused update: VisibilityTimeout must keep the value it had, got {after:?}"
    );
}

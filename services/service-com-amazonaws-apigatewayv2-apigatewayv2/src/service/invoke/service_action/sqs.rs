// SPDX-License-Identifier: CC0-1.0

//! The `SQS-SendMessage` action: the message a route sends to a queue.
//!
//! The parameters are the members of `SendMessage`'s own input
//! (`smithy_query(query="operation", operation="SendMessage")` on the Amazon SQS model),
//! which is what the action takes. `QueueUrl` and `MessageBody` are the members the model
//! marks required, so an integration mapping neither names no send at all and the request is
//! answered as an integration that produced no response — never as a message nothing
//! enqueued.
//!
//! The queue URL is a parameter of the in-process call and NOT a destination this process
//! connects to: the client is the emulator's own transport, addressed at the Amazon SQS
//! endpoint of the api's Region, so the send is served by the queue this emulator holds.
//!
//! What the send then does is Amazon SQS's own: whether the calling role may send to that
//! queue, what the queue's own configuration does with the message, and what a consumer
//! receives.

use serde_json::{json, Map, Value};
use smithy_server_runtime::OperationContext;

use super::credentials::SigningIdentity;
use super::parameters::Mapped;
use super::Unperformed;

/// The `IntegrationSubtype` that names this action.
pub const SEND_MESSAGE_SUBTYPE: &str = "SQS-SendMessage";

/// Every parameter an integration may map onto the send: the members of
/// `SendMessageRequest` this emulator carries into the call.
///
/// The two attribute MAPS the operation also declares (`MessageAttributes`,
/// `MessageSystemAttributes`) are not among them, so an integration mapping one is reported
/// rather than sent without it ([`Mapped::beyond`]).
pub const SEND_MESSAGE_PARAMETERS: &[&str] = &[
    "DelaySeconds",
    "MessageBody",
    "MessageDeduplicationId",
    "MessageGroupId",
    "QueueUrl",
];

/// The `arnNamespace` Amazon SQS publishes its endpoint and signs under, which is the
/// endpoint label the in-process client is addressed at.
const SQS_ENDPOINT: &str = "sqs";

/// Send the mapped message, and answer the document Amazon SQS returned.
pub async fn send_message(
    ctx: &OperationContext<'_>,
    region: &str,
    identity: Option<&SigningIdentity>,
    mapped: &Mapped,
) -> Result<Value, Unperformed> {
    let (Some(queue_url), Some(body)) = (mapped.get("QueueUrl"), mapped.get("MessageBody")) else {
        return Err(Unperformed::NotPerformed(format!(
            "the integration maps no QueueUrl and MessageBody, which {SEND_MESSAGE_SUBTYPE} \
             requires"
        )));
    };
    let client = action_client!(aws_sdk_sqs, ctx.callback(), SQS_ENDPOINT, region, identity);
    let mut call = client.send_message().queue_url(queue_url).message_body(body);
    if let Some(delay) = mapped.get("DelaySeconds") {
        // `DelaySeconds` is an integer member, so a mapped value that is not one is not a
        // delay the action can be called with.
        let seconds = delay.parse::<i32>().map_err(|error| {
            Unperformed::NotPerformed(format!(
                "the integration maps a DelaySeconds this action cannot be called with: \
                 {error}"
            ))
        })?;
        call = call.delay_seconds(seconds);
    }
    if let Some(deduplication_id) = mapped.get("MessageDeduplicationId") {
        call = call.message_deduplication_id(deduplication_id);
    }
    if let Some(group_id) = mapped.get("MessageGroupId") {
        call = call.message_group_id(group_id);
    }
    let sent = call
        .send()
        .await
        .map_err(|error| super::unperformed(SEND_MESSAGE_SUBTYPE, &error))?;
    let mut answer = Map::new();
    if let Some(digest) = sent.md5_of_message_body() {
        answer.insert("MD5OfMessageBody".to_string(), json!(digest));
    }
    if let Some(message_id) = sent.message_id() {
        answer.insert("MessageId".to_string(), json!(message_id));
    }
    if let Some(sequence_number) = sent.sequence_number() {
        answer.insert("SequenceNumber".to_string(), json!(sequence_number));
    }
    Ok(Value::Object(answer))
}

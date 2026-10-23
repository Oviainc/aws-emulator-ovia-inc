// SPDX-License-Identifier: CC0-1.0

//! The `p` member Amazon Bedrock Runtime puts in every event-stream frame it answers
//! with, which the shared seam in `service_common::event_stream` cannot write.
//!
//! Bedrock Runtime's event-stream responses carry, beside the members the Smithy
//! model declares for the event, one extra JSON member `p`: a run of characters that
//! carries no information. It is padding — it varies the encoded size of a frame so
//! the size of a response does not leak the content it carries — and it is on the
//! wire of every `ConverseStream` and `InvokeModelWithResponseStream` frame, which is
//! why a client's frames decode with it present. The Smithy model does not declare it
//! (it is not a modelled member of any event shape), so it cannot be written through
//! the event's schema: [`frame_padded_event`] serializes the event through the
//! service's own protocol, exactly as a non-padded frame is serialized, and then adds
//! the member to the encoded payload.
//!
//! Nothing here is derived from the request or from a recording: the padding is a
//! fixed run of characters this emulator writes, because padding is by definition
//! information-free.

use aws_smithy_eventstream::frame::{read_message_from, write_message_to};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::event_stream::Message;
use service_common::event_stream::frame_event;
use smithy_server_runtime::ServerHttpProtocol;

/// The JSON member the padding travels in.
pub const PADDING_MEMBER: &str = "p";

/// The padding this emulator writes: an information-free run of characters. AWS
/// chooses a length per frame; one fixed run is the same contract to a client,
/// which must ignore the member either way.
pub const PADDING: &str = "abcdefghijklmnopqrstuvwxyz";

/// Serialize one modelled event structure as a single `vnd.amazon.eventstream`
/// frame carrying the [`PADDING_MEMBER`], through the service's own protocol.
///
/// The frame headers and the JSON encoding are the protocol's: the event is framed
/// by `service_common::event_stream::frame_event`, then the one frame is decoded,
/// the padding member added to its JSON object payload, and the frame written back
/// with its headers unchanged.
pub fn frame_padded_event(
    protocol: &dyn ServerHttpProtocol,
    event_type: &str,
    event: &dyn SerializableStruct,
    schema: &Schema,
) -> Result<Vec<u8>, ProtocolError> {
    let framed = frame_event(protocol, event_type, false, event, schema)?;
    let mut buffer = ::bytes::Bytes::from(framed);
    let message = read_message_from(&mut buffer).map_err(|error| {
        ProtocolError::Malformed(format!("the {event_type} frame could not be read: {error}"))
    })?;

    let mut payload: serde_json::Value = serde_json::from_slice(message.payload())
        .map_err(|error| {
            ProtocolError::Malformed(format!(
                "the {event_type} payload is not a JSON object: {error}"
            ))
        })?;
    match payload.as_object_mut() {
        Some(object) => {
            object.insert(
                PADDING_MEMBER.to_string(),
                serde_json::Value::String(PADDING.to_string()),
            );
        }
        None => {
            return Err(ProtocolError::Malformed(format!(
                "the {event_type} payload is not a JSON object"
            )))
        }
    }
    let padded = serde_json::to_vec(&payload).map_err(|error| {
        ProtocolError::Malformed(format!(
            "the padded {event_type} payload could not be encoded: {error}"
        ))
    })?;

    let mut rewritten = Message::new(padded);
    for header in message.headers() {
        rewritten = rewritten.add_header(header.clone());
    }
    let mut out = Vec::new();
    write_message_to(&rewritten, &mut out).map_err(|error| {
        ProtocolError::Malformed(format!(
            "the padded {event_type} frame could not be encoded: {error}"
        ))
    })?;
    Ok(out)
}

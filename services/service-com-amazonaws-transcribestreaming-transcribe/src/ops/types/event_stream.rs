// SPDX-License-Identifier: CC0-1.0

//! `application/vnd.amazon.eventstream` wire support for this service's four
//! `@streaming` operations, plus the seam that lets the editable `service.rs` route
//! a request to one of them.
//!
//! `src/ops/SKIPPED_STUBS.md` records why this file exists: the op-stub generator
//! declines an operation with a `@streaming` event-stream input member because it
//! emits no wire framing for one, and the generated `ErasedOperation` shim of an
//! operation whose OUTPUT is a `@streaming` union declines it too (its
//! `serialize_as_event_stream<P>` needs a `Sized` protocol, which the object-safe
//! `run` cannot hand it). Everything a `Start*` operation needs that the generated
//! path cannot give it lives here:
//!
//! * [`decode_event_stream`] — split a request body into its event frames, reading
//!   the `:event-type` header the AWS SDKs set on each one.
//! * [`frame_event`] — serialize one modeled event structure as ONE
//!   `vnd.amazon.eventstream` frame, through the service's own protocol (so the
//!   frame headers and the JSON payload are the protocol's, not this file's).
//! * [`event_stream_response`] / [`error_response`] — a 200 whose modeled response
//!   headers come from the output shape and whose body is the concatenated frames,
//!   and a modeled error rendered in the service's own envelope.
//! * [`EventStreamOperation`] / [`EventStreamRoute`] — the object-safe adapter
//!   `Service::override_operation` hands back.

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_runtime_api::http::{Request, Response, StatusCode};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{envelope, validate};
use aws_smithy_schema_server::{missing_member_message, DeserializableStruct, ProtocolError};
use aws_smithy_types::body::SdkBody;
use bytes::Buf;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, OperationError, ServerHttpProtocol, Service,
};

/// The content type of an event-stream body, in both directions.
pub const EVENT_STREAM_CONTENT_TYPE: &str = "application/vnd.amazon.eventstream";

/// What this emulator answers the RECOGNITION itself with.
///
/// Every `Start*` operation here runs the control plane for real — it validates the
/// request against the model, stores the session and its negotiated settings,
/// accounts for the audio that arrived, and reports all of it back — but no speech
/// recogniser exists in this process, so there is no transcript to report. Rather
/// than invent words, each result stream carries the modeled
/// `ServiceUnavailableException` member of its `@streaming` union, framed as an
/// in-band exception, with this message: the model's own channel for a stream that
/// yields no result.
pub const NO_RECOGNISER: &str = "This emulator does not perform speech recognition, so no \
     transcript is produced for the streamed audio. The streaming session, its negotiated \
     settings and its status are real and are reported by this service's session APIs.";

/// One decoded input frame: which event-stream union member it carries, and its
/// payload bytes.
///
/// The payload of an event whose structure has an `@eventPayload` member (every
/// `AudioEvent` here) is that member's raw value; for any other event it is the
/// event structure encoded in the protocol's body format.
#[derive(Debug, Clone)]
pub struct DecodedEvent {
    pub event_type: String,
    pub payload: Vec<u8>,
}

/// Why an event-stream request body could not be used.
///
/// The two cases are answered with different modeled errors, so they are carried
/// apart rather than as one sentence: a malformed body is the caller's mistake
/// (`BadRequestException`), while a body past this emulator's capacity is a limit
/// (`LimitExceededException`).
#[derive(Debug, Clone)]
pub enum EventStreamError {
    /// The body is not a usable event-stream message stream.
    Malformed(String),
    /// The body carries more frames than [`crate::state::MAX_EVENTS_PER_STREAM`].
    TooManyEvents(String),
}

/// Split an event-stream request body into its frames, in arrival order.
///
/// Bounded at [`crate::state::MAX_EVENTS_PER_STREAM`] frames: the frame count is
/// caller-chosen work, so it is refused before it is done rather than after.
pub fn decode_event_stream(body: &[u8]) -> Result<Vec<DecodedEvent>, EventStreamError> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut events = Vec::new();
    while buffer.has_remaining() {
        if events.len() >= crate::state::MAX_EVENTS_PER_STREAM {
            return Err(EventStreamError::TooManyEvents(format!(
                "the stream carries more than the {} events this emulator accepts in one \
                 request; send the audio in fewer, larger events",
                crate::state::MAX_EVENTS_PER_STREAM
            )));
        }
        let before = buffer.remaining();
        let message = read_message_from(&mut buffer).map_err(|e| {
            EventStreamError::Malformed(format!(
                "the request body is not a valid application/vnd.amazon.eventstream \
                 message stream: {e}"
            ))
        })?;
        if buffer.remaining() >= before {
            return Err(EventStreamError::Malformed(
                "the request body carries a zero-length event-stream frame".to_string(),
            ));
        }
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|h| h.name().as_str() == name)
                .and_then(|h| match h.value() {
                    aws_smithy_types::event_stream::HeaderValue::String(s) => {
                        Some(s.as_str().to_string())
                    }
                    _ => None,
                })
        };
        // `:message-type` is `event` for a union member and `exception` for an
        // in-band error; a client sends only events.
        let message_type = header(":message-type").unwrap_or_default();
        if message_type != "event" {
            return Err(EventStreamError::Malformed(format!(
                "event-stream frame has :message-type '{message_type}', but only 'event' \
                 frames may be sent on an input stream"
            )));
        }
        let event_type = header(":event-type").ok_or_else(|| {
            EventStreamError::Malformed(
                "event-stream frame carries no :event-type header naming the event".to_string(),
            )
        })?;
        events.push(DecodedEvent {
            event_type,
            payload: message.payload().to_vec(),
        });
    }
    Ok(events)
}

/// Deserialize one decoded frame's JSON payload into the modeled event structure
/// `T`, then check it against the model's constraints.
///
/// Driven through the service's own protocol, like every other helper here: the
/// payload is handed to `deserialize_input` as a body, and `T`'s schema binds no
/// HTTP member, so the protocol's body codec is what parses it.
pub fn event_payload<T>(
    protocol: &dyn ServerHttpProtocol,
    payload: &[u8],
    schema: &Schema,
) -> Result<T, String>
where
    T: DeserializableStruct + SerializableStruct,
{
    let body = Request::new(SdkBody::from(payload.to_vec()));
    let mut deserializer = protocol
        .deserialize_input(&body, schema)
        .map_err(|e| format!("the event payload could not be read: {e}"))?;
    let value = T::deserialize(&mut *deserializer).map_err(render_serde_error)?;
    drop(deserializer);
    match envelope(&validate(&value, schema)) {
        Some(message) => Err(message),
        None => Ok(value),
    }
}

/// Read the `@httpHeader`-bound half of a `Start*` request into `T` and validate it
/// against the model.
///
/// The event-stream payload member is excluded from `T`'s schema (see
/// `super::streaming_types`), so the body is never parsed here — it is handed to
/// [`decode_event_stream`] instead. `Err` carries the refusal sentence, which the
/// caller answers with `BadRequestException`: the model documents that error for
/// exactly this class ("`MediaEncoding` or `LanguageCode` used unsupported
/// values").
pub fn settings_from_headers<T>(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
    schema: &Schema,
) -> Result<T, String>
where
    T: DeserializableStruct + SerializableStruct,
{
    // The protocol's own header binder, driven over a copy of the request whose
    // body is empty: `T`'s schema binds every member to a header, so an empty body
    // keeps the JSON codec from being pointed at event-stream frames.
    let mut headers_only = Request::new(SdkBody::empty());
    headers_only
        .set_uri(req.uri())
        .map_err(|e| format!("the request URI could not be read: {e}"))?;
    headers_only
        .set_method(req.method())
        .map_err(|e| format!("the request method could not be read: {e}"))?;
    for (name, value) in req.headers().iter() {
        headers_only
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }

    let mut deserializer = protocol
        .deserialize_input(&headers_only, schema)
        .map_err(|e| format!("the request headers could not be read: {e}"))?;
    let value = T::deserialize(&mut *deserializer).map_err(render_serde_error)?;
    drop(deserializer);
    match envelope(&validate(&value, schema)) {
        Some(message) => Err(message),
        None => Ok(value),
    }
}

/// Word a deserialization failure the way the framework's own boundary does, so an
/// absent required member reads the same here as on a non-streaming operation.
fn render_serde_error(error: SerdeError) -> String {
    match error {
        SerdeError::MissingMember { member_name } => missing_member_message(&member_name),
        other => other.to_string(),
    }
}

/// Serialize one modeled event structure as a single `vnd.amazon.eventstream`
/// frame, through the service's own protocol.
///
/// `event_type` is the `@streaming` union's member name, which the frame carries in
/// its `:event-type` header; `is_exception` marks an in-band error member, which
/// travels as `:message-type: exception` instead.
pub fn frame_event(
    protocol: &dyn ServerHttpProtocol,
    event_type: &str,
    is_exception: bool,
    event: &dyn SerializableStruct,
    schema: &Schema,
) -> Result<Vec<u8>, ProtocolError> {
    let framed = protocol.serialize_event_stream_message(event_type, is_exception, event, schema)?;
    Ok(framed
        .into_body()
        .bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_default())
}

/// A 200 carrying `frames` as its event-stream body and the operation's modeled
/// response headers.
///
/// The headers are serialized from the output shape through the protocol, so every
/// header name, and the rendering of every value, is the model's.
pub fn event_stream_response(
    protocol: &dyn ServerHttpProtocol,
    operation: &str,
    headers: &dyn SerializableStruct,
    headers_schema: &Schema,
    frames: Vec<u8>,
) -> Result<Response, ProtocolError> {
    let rendered = protocol.serialize_output(headers, headers_schema, operation, 200)?;
    let mut response = Response::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        SdkBody::from(frames),
    );
    for (name, value) in rendered.headers().iter() {
        if name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        response
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    response
        .headers_mut()
        .insert("content-type", EVENT_STREAM_CONTENT_TYPE);
    Ok(response)
}

/// Render a modeled error in this service's own envelope, with the status the model
/// gives it.
pub fn error_response(protocol: &dyn ServerHttpProtocol, error: &dyn OperationError) -> Response {
    let status = error.http_status();
    protocol
        .serialize_error(
            error.as_serializable(),
            error.variant_schema(),
            error.shape_name(),
            status,
        )
        .unwrap_or_else(|_| {
            // Unreachable for a JSON envelope, but an error path must not panic.
            let mut response = Response::new(
                StatusCode::try_from(500u16).expect("500 is a valid status"),
                SdkBody::from(r#"{"__type":"InternalFailureException"}"#),
            );
            response
                .headers_mut()
                .insert("content-type", "application/json");
            response
        })
}

/// One hand-authored event-stream operation: the whole request-to-response step,
/// because the generated `deserialize -> validate -> handle -> serialize` sequence
/// cannot carry an event-stream body.
pub trait EventStreamOperation: Send + Sync {
    /// Serve the request. `svc` carries this service's wire protocol, which every
    /// helper above is driven through.
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response>;
}

/// The object-safe wrapper `Service::override_operation` returns for an
/// [`EventStreamOperation`].
///
/// It exists because the generated `impl ErasedOperation` for a `@streaming`-output
/// operation declines the request, and a type can carry only one impl of that
/// trait: this wrapper is a distinct type, so the operation's own implementation is
/// reachable without touching generated code.
pub struct EventStreamRoute<T>(pub T);

impl<T: EventStreamOperation + 'static> ErasedOperation for EventStreamRoute<T> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        self.0.serve(svc, req, ctx)
    }

    // No override: `None` defers to the auth schemes the model gives the operation
    // the request's routing key names, which is this same operation.
}

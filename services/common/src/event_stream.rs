// SPDX-License-Identifier: CC0-1.0

//! `application/vnd.amazon.eventstream` wire support for an operation whose OUTPUT
//! is a `@streaming` union, plus the seam that lets an editable `service.rs` route a
//! request to one.
//!
//! # Why this lives here
//!
//! The scaffold's op-stub generator declines an operation with a `@streaming`
//! event-stream member (each service's `src/ops/SKIPPED_STUBS.md` records it), and the
//! generated `ErasedOperation` shim of such an operation declines the request too: its
//! `serialize_as_event_stream<P>` needs a `Sized` protocol, which the object-safe `run`
//! cannot hand it. So every service with a streaming operation needs the SAME pieces,
//! none of which names a service:
//!
//! * [`deserialize_request`] — read a request into the modelled input shape through the
//!   service's own protocol, then check it against the model's constraints.
//! * [`frame_event`] — serialize one modelled event structure as ONE
//!   `vnd.amazon.eventstream` frame, through the service's own protocol (so the frame
//!   headers and the payload encoding are the protocol's, not this file's).
//! * [`event_stream_response`] / [`event_stream_only_response`] / [`error_response`] —
//!   a 200 whose modelled response headers come from the output shape and whose body is
//!   the concatenated frames, and a modelled error in the service's own envelope.
//! * [`EventStreamOperation`] / [`EventStreamRoute`] — the object-safe adapter
//!   `Service::override_operation` hands back.
//!
//! # Services still carrying their own copy
//!
//! Two crates inlined these helpers before there was anywhere shared to put them:
//!
//! * `services/service-com-amazonaws-transcribestreaming-transcribe/src/ops/types/event_stream.rs`
//!   — the full set, near-identical to this module (it also owns an event-stream INPUT
//!   decoder, which stays service-local because its frame budget is); and
//! * `services/service-com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2/src/ops/invoke_endpoint_with_bidirectional_stream.rs`
//!   — a partial copy of the framing inside the one operation that needs it.
//!
//! Both are to be MIGRATED onto this module: delete the local copy, `use
//! service_common::event_stream::…`. That migration is NOT in this change because it
//! rewrites the wire path of three Transcribe Streaming operations and one SageMaker
//! Runtime one, whose correctness this change's issue does not own and whose own suites
//! are the only thing that could pass judgement on it; doing it here would put an
//! unreviewed rewrite of two other services inside a Bedrock Runtime fix. Nothing new
//! should inline the framing again — call this module.

use aws_smithy_runtime_api::http::{Request, Response, StatusCode};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{envelope, validate};
use aws_smithy_schema_server::{missing_member_message, DeserializableStruct, ProtocolError};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, OperationError, ServerHttpProtocol, Service,
};

/// The content type of an event-stream body, in both directions.
pub const EVENT_STREAM_CONTENT_TYPE: &str = "application/vnd.amazon.eventstream";

/// Read `req` into the modelled input shape `T` and check it against the model.
///
/// Driven through the service's own protocol, so the HTTP bindings the model
/// declares — `@httpLabel`, `@httpHeader`, `@httpQuery`, `@httpPayload` — and the
/// body codec are the protocol's. `Err` carries the refusal sentence, which the
/// caller answers with the modelled error for a bad request; an absent required
/// member is worded exactly as the framework's own boundary words it.
pub fn deserialize_request<T>(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
    schema: &Schema,
) -> Result<T, String>
where
    T: DeserializableStruct + SerializableStruct,
{
    let mut deserializer = protocol
        .deserialize_input(req, schema)
        .map_err(|error| format!("the request could not be read: {error}"))?;
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

/// Serialize one modelled event structure as a single `vnd.amazon.eventstream`
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

/// A 200 carrying `frames` as its event-stream body and the operation's modelled
/// response headers.
///
/// The headers are serialized from the output shape through the protocol, so every
/// header name, and the rendering of every value, is the model's. The body's own
/// content type is the event-stream one, which replaces whatever the protocol's
/// non-streaming rendering would have set.
pub fn event_stream_response(
    protocol: &dyn ServerHttpProtocol,
    operation: &str,
    headers: &dyn SerializableStruct,
    headers_schema: &Schema,
    frames: Vec<u8>,
) -> Result<Response, ProtocolError> {
    let rendered = protocol.serialize_output(headers, headers_schema, operation, 200)?;
    let mut response = event_stream_only_response(frames);
    for (name, value) in rendered.headers().iter() {
        if name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        response
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    Ok(response)
}

/// A 200 carrying `frames` as its event-stream body, for an output shape whose ONLY
/// member is the `@streaming` payload — so there are no modelled response headers to
/// serialize and [`event_stream_response`] would have nothing to read them from.
pub fn event_stream_only_response(frames: Vec<u8>) -> Response {
    let mut response = Response::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        SdkBody::from(frames),
    );
    response
        .headers_mut()
        .insert("content-type", EVENT_STREAM_CONTENT_TYPE);
    response
}

/// Render a modelled error in the serving service's own envelope, with the status the
/// model gives it.
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
                SdkBody::from(r#"{"__type":"InternalServerException"}"#),
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

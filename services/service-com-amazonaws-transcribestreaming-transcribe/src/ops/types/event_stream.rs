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
//!
//! ## The two transports of one operation
//!
//! Each `Start*` operation is reachable over two transports. The modeled one is
//! HTTP/2: the `@http` method and URI the model binds, with every input member on
//! its `@httpHeader`. The WebSocket one is the SAME operation at the same URI plus
//! the `-websocket` suffix ([`WEBSOCKET_PATH_SUFFIX`]), with each input member moved
//! out of its `x-amzn-transcribe-*` header into the query parameter of that name
//! without the prefix. [`settings_from_query`] reads that spelling back onto the
//! same modeled shape [`settings_from_headers`] fills, so one set of model
//! constraints validates both transports; [`is_websocket_transport_of`] is how an
//! operation tells which transport a request arrived on.

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

/// What this emulator answers a WebSocket upgrade of a `Start*` operation with.
///
/// The WebSocket transport is a protocol UPGRADE: the service answers `101
/// Switching Protocols` and the connection then carries `vnd.amazon.eventstream`
/// frames in both directions. This emulator's front door serves one BUFFERED
/// request per call — `EmulatorService::handle_request` takes an
/// `http::Request<Bytes>` and returns an `http::Response<Bytes>`
/// (`vendor/server/src/lib.rs`), the router strips the hop-by-hop `connection` and
/// `upgrade` response headers a handshake needs
/// (`vendor/smithy-server-runtime/src/multi_service_router.rs`), an operation is
/// handed the request's `http::request::Parts` by shared reference so it can never
/// take hyper's upgrade handle (`vendor/smithy-server-runtime/src/operation.rs`),
/// and the vendored listener refuses a WebSocket upgrade outright
/// (`vendor/server/src/listeners.rs`). So no socket can be switched to the WebSocket
/// protocol from inside a service, and claiming `101` for a stream that cannot then
/// carry a frame would be worse than refusing it. The upgrade's parameters are
/// validated against the model exactly as the HTTP/2 transport's headers are, and
/// the stream itself is refused with the modeled `ServiceUnavailableException` —
/// the model's own channel for a stream this service cannot carry — naming the
/// transport that IS served.
pub const NO_WEBSOCKET_TRANSPORT: &str = "This emulator cannot serve the WebSocket transport of \
     this operation: its HTTP front door answers one buffered request per call and cannot switch \
     a connection to the WebSocket protocol, so no 101 Switching Protocols handshake is possible. \
     The request's parameters were read and validated against the model. Stream the audio over \
     the modeled HTTP/2 transport of the same operation instead.";

/// The suffix AWS appends to a streaming operation's modeled URI to address the
/// WebSocket spelling of that SAME operation (`/stream-transcription` ->
/// `/stream-transcription-websocket`).
pub const WEBSOCKET_PATH_SUFFIX: &str = "-websocket";

/// The first URI path segments this service's WebSocket data plane serves: the
/// modeled URI of each streaming operation AWS documents a WebSocket transport for,
/// plus [`WEBSOCKET_PATH_SUFFIX`].
///
/// The Smithy model does not describe the WebSocket binding — it binds only the
/// HTTP/2 `@http` URI of each operation — so this is the set AWS documents
/// ("Using Amazon Transcribe streaming with WebSockets": `stream-transcription-websocket`
/// for `StartStreamTranscription` and `medical-stream-transcription-websocket` for
/// `StartMedicalStreamTranscription`), and nothing wider: a path AWS does not
/// publish must stay unclaimed rather than be invented here.
pub const WEBSOCKET_PATH_SEGMENTS: &[&str] = &[
    "stream-transcription-websocket",
    "medical-stream-transcription-websocket",
];

/// The `@httpHeader` prefix every `Start*` input member carries, and the prefix the
/// WebSocket spelling of that member drops: `MediaSampleRateHertz` is
/// `x-amzn-transcribe-sample-rate` as a header and `sample-rate` as a query
/// parameter.
const TRANSCRIBE_HEADER_PREFIX: &str = "x-amzn-transcribe-";

/// The prefix of the query parameters AWS reserves for itself, which carry the
/// request's signature rather than any member of the operation's input: the SigV4
/// query-signing set (`X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`,
/// `X-Amz-Expires`, `X-Amz-SignedHeaders`, `X-Amz-Signature`,
/// `X-Amz-Security-Token`). The whole prefix is skipped rather than those names one
/// by one, because no input member of this service spells its query parameter with
/// it; the emulator — never a service — verifies what they carry.
const AWS_RESERVED_QUERY_PREFIX: &str = "x-amz-";

/// The path of a request URI, without its query string.
pub fn request_path(uri: &str) -> &str {
    let path = uri.split('?').next().unwrap_or_default();
    // An absolute-form URI (`https://host/path`) carries the authority before the
    // path; the origin form a server usually sees does not.
    match path.find("://") {
        Some(scheme_end) => match path[scheme_end + 3..].find('/') {
            Some(authority_end) => &path[scheme_end + 3 + authority_end..],
            None => "/",
        },
        None => path,
    }
}

/// Whether `uri` addresses the WebSocket transport of the operation the model binds
/// to `modeled_path`.
pub fn is_websocket_transport_of(uri: &str, modeled_path: &str) -> bool {
    request_path(uri).strip_suffix(WEBSOCKET_PATH_SUFFIX) == Some(modeled_path)
}

/// Read the `@httpHeader`-bound half of a `Start*` request out of the QUERY STRING
/// of a WebSocket upgrade, into the same `T` [`settings_from_headers`] fills from
/// headers.
///
/// Each parameter is re-presented as the header the model binds its member to —
/// [`TRANSCRIBE_HEADER_PREFIX`] plus the parameter name — and the request is then
/// read by the protocol's own header binder and checked against the model's
/// constraints, so the WebSocket transport accepts and refuses exactly what the
/// HTTP/2 transport does. The SigV4 query-signing parameters are skipped: they name
/// no member of the operation.
///
/// The query string is split into decoded pairs by the FRAMEWORK's own
/// `parse_form_urlencoded` — the decoder its `query` protocols use — so this
/// transport cannot disagree with the rest of the emulator about percent-decoding.
///
/// `Err` carries the refusal sentence the caller answers with
/// `BadRequestException`, as on the header transport.
pub fn settings_from_query<T>(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
    schema: &Schema,
) -> Result<T, String>
where
    T: DeserializableStruct + SerializableStruct,
{
    let mut as_headers = Request::new(SdkBody::empty());
    as_headers
        .set_uri(req.uri())
        .map_err(|e| format!("the request URI could not be read: {e}"))?;
    as_headers
        .set_method(req.method())
        .map_err(|e| format!("the request method could not be read: {e}"))?;
    let query = req.uri().split_once('?').map(|(_, q)| q).unwrap_or_default();
    for (name, value) in aws_smithy_schema_server::parse_form_urlencoded(query.as_bytes()) {
        let name = name.to_ascii_lowercase();
        if name.starts_with(AWS_RESERVED_QUERY_PREFIX) {
            continue;
        }
        let header = format!("{TRANSCRIBE_HEADER_PREFIX}{name}");
        if as_headers.headers_mut().try_append(header, value).is_err() {
            // Either the parameter's name or its value carries bytes HTTP cannot
            // represent (a control character, say), so the parameter cannot be read
            // as the member it names.
            return Err(format!(
                "the request parameter '{name}' could not be read: its name or its value is \
                 not valid HTTP"
            ));
        }
    }
    settings_from_headers(protocol, &as_headers, schema)
}

/// The refusal for a request that names `NumberOfChannels` without asking for
/// channel identification, `None` when the pair is consistent.
///
/// The model states the rule on the member itself: "If you include
/// `NumberOfChannels` in your request, you must also include
/// `EnableChannelIdentification`"
/// (smithy_query(query="doc", shape="StartStreamTranscriptionRequest", member="NumberOfChannels")).
/// It is a rule ACROSS two members, which no constraint trait can express, so the
/// operations check it here — both of the operations that bind the pair, on both of
/// their transports.
pub fn channel_identification_refusal(
    enable_channel_identification: bool,
    number_of_channels: Option<i32>,
) -> Option<String> {
    let channels = number_of_channels?;
    (!enable_channel_identification).then(|| {
        format!(
            "NumberOfChannels was set to {channels} without EnableChannelIdentification: if you \
             include NumberOfChannels in your request, you must also include \
             EnableChannelIdentification"
        )
    })
}

/// The refusal for a request that names `PartialResultsStability` without enabling
/// partial-results stabilization, `None` when the pair is consistent.
///
/// The model states the rule on the member itself: `PartialResultsStability` is the
/// "level of stability to use when you enable partial results stabilization
/// (`EnablePartialResultsStabilization`)"
/// (smithy_query(query="doc", shape="StartStreamTranscriptionRequest", member="PartialResultsStability")),
/// so a stability level without that switch selects a level for something that is
/// off. Like [`channel_identification_refusal`], it spans two members and so cannot
/// be a constraint trait.
pub fn partial_results_stability_refusal(
    enable_partial_results_stabilization: bool,
    partial_results_stability: Option<&str>,
) -> Option<String> {
    let stability = partial_results_stability?;
    (!enable_partial_results_stabilization).then(|| {
        format!(
            "PartialResultsStability was set to '{stability}' without \
             EnablePartialResultsStabilization: a stability level applies only when you enable \
             partial results stabilization"
        )
    })
}

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

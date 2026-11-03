// SPDX-License-Identifier: CC0-1.0

//! @op InvokeEndpointWithBidirectionalStream
//!
//! Invokes a model endpoint with bidirectional streaming: the request carries a
//! `vnd.amazon.event-stream` sequence of `RequestStreamEvent` members and the
//! response answers with a sequence of `ResponseStreamEvent` members.
//!
//! This operation is HAND-AUTHORED rather than scaffolded. The op-stub generator
//! declined it ("a `@streaming` event-stream input member, which needs wire
//! framing the generator does not emit", `src/ops/SKIPPED_STUBS.md`), and the
//! `#[smithy_service]` expansion also declines any operation whose OUTPUT binds a
//! `@streaming` union — the generated `ErasedOperation` shim for
//! [`InvokeEndpointWithBidirectionalStream`] therefore answers the framework's
//! "modeled but not served" signal and is never the thing that serves a request.
//! What serves it is [`BidirectionalStreamEntry`], the hand-written
//! [`ErasedOperation`] that `crate::service::EmulatedService::override_operation`
//! returns — the router consults that seam BEFORE the generated routing table.
//! The table entry still matters: the router reads the MODEL's auth schemes off
//! it, so this operation is SigV4-signed exactly as the model says.
//!
//! ## What the emulator computes
//!
//! Everything in the response comes from this service's own store
//! ([`crate::state`]) and from the frames the request carried:
//!
//! * the endpoint is resolved out of the registry — an unregistered
//!   `EndpointName` cannot succeed;
//! * `X-Amzn-Invoked-Production-Variant` is the variant actually selected: the
//!   `TargetVariant` header when it names one of THAT endpoint's production
//!   variants, otherwise that endpoint's default variant;
//! * the response frames are the endpoint's declared
//!   [`ModelBehaviour`](crate::state::ModelBehaviour) applied to the accumulated
//!   request frames.
//!
//! `ModelInvocationPath` and `ModelQueryString` are read, validated against their
//! modelled `@length`/`@pattern`, and recorded on the transcript entry, but they
//! do NOT affect the answer: on the real service they select the path and query
//! the invocation is forwarded to INSIDE the model container, and this emulator
//! runs no container to forward them to. A test must not read per-path or
//! per-query model routing off this operation; what it can assert is that the
//! values were received, which the transcript shows.
//!
//! The emulator runs no model artifact — there is none to run — so it never
//! invents an inference result: each endpoint's answering rule is declared in the
//! registry and the handler applies it.
//!
//! ## What is observable afterwards
//!
//! Every invocation that reached endpoint resolution is appended to the store's
//! bounded transcript, which is the only way a caller can read back what was
//! received and produced (the service models no operation that reports it). A
//! request refused EARLIER — an unroutable URI, a body that is not an event
//! stream, a value outside a modelled constraint, a malformed or over-cap stream,
//! an unregistered `EndpointName`, a `TargetVariant` the endpoint does not have —
//! never becomes a transcript entry, so an absent entry does not mean no request
//! arrived. Those are covered by the structured audit record this operation emits
//! for EVERY request it serves (`tracing::info!`: the signing name, the
//! operation, the presented access-key id, the method and path, and the status it
//! answered with — never a payload, a header value or a credential secret).
//!
//! ## Modeled contract
//!
//! * **Route:** `POST /endpoints/{EndpointName}/invocations-bidirectional-stream`
//! * **Input:** `EndpointName` (`@httpLabel`, required, maxLen 63, pattern
//!   `^[a-zA-Z0-9](-*[a-zA-Z0-9])*`), `Body` (`@httpPayload`, required,
//!   `@streaming` union `RequestStreamEvent`), and the headers `TargetVariant`
//!   (`X-Amzn-SageMaker-Target-Variant`), `ModelInvocationPath`
//!   (`X-Amzn-SageMaker-Model-Invocation-Path`), `ModelQueryString`
//!   (`X-Amzn-SageMaker-Model-Query-String`), each with its own `@length` and
//!   `@pattern`.
//! * **Output:** `Body` (`@httpPayload`, required, `@streaming` union
//!   `ResponseStreamEvent` with members `PayloadPart`, `ModelStreamError`,
//!   `InternalStreamFailure`) and `InvokedProductionVariant`
//!   (`X-Amzn-Invoked-Production-Variant`).
//! * **Errors:** `InputValidationError` (400), `InternalServerError` (500),
//!   `InternalStreamFailure` (500), `ModelError` (424), `ModelStreamError` (400),
//!   `ServiceUnavailableError` (503).
//!
//! `ModelStreamError` and `InternalStreamFailure` are declared BOTH as top-level
//! errors and as members of `ResponseStreamEvent`. A failure the model reports
//! once the response stream is open cannot change a status already sent, so this
//! handler delivers those two IN-STREAM: inside the response body, after the 200
//! and after the `PayloadPart` frames that precede them.
//! `ServiceUnavailableError` throttling is out of this slice's scope and is left
//! to its own issue; nothing here fabricates one.
//!
//! The response body is BUFFERED, not progressively flushed: the router hands
//! this operation a buffered request and takes a buffered response, so the whole
//! frame sequence — including a terminating in-stream error member — is encoded
//! and then delivered as one body. The frame ORDER a client decodes is the real
//! one; the TIMING is not, so a test must not read time-to-first-byte or
//! partial-read behaviour off this emulator. Progressive delivery would need a
//! streaming response body from the framework and is left to its own issue.
//!
//! ## What is bounded, and why
//!
//! Both the work and the memory of one invocation are sized by the caller: it
//! chooses how many frames the body holds, how large each one is, and (through
//! the addressed endpoint's `chunk_size`) how many response frames those bytes
//! are re-cut into — and each response frame costs a prelude, its headers and two
//! CRCs on top of its payload, so a small body can amplify. Four caps hold that
//! down, every one applied BEFORE the thing it bounds is allocated:
//!
//! * [`MAX_REQUEST_FRAMES`] and [`MAX_REQUEST_PAYLOAD_BYTES`] on the request
//!   stream, which are what the CALLER chose, refused with the modelled
//!   `InputValidationError`;
//! * [`MAX_RESPONSE_FRAMES`] on the answer, checked from the frame COUNT before
//!   a single frame is built. That count is the product of a REGISTRY setting
//!   (the endpoint's `chunk_size`, or the length of its script) and a payload the
//!   caller was already allowed to send, so it is attributed as
//!   `InternalServerError` — a misconfigured endpoint, not bad input. Both caps
//!   here are EMULATOR limits: AWS returns neither error for these causes;
//! * the transcript's own [`crate::state::MAX_RECORDED_INVOCATIONS`],
//!   [`crate::state::MAX_RECORDED_FRAMES_PER_INVOCATION`],
//!   [`crate::state::MAX_RECORDED_INVOCATION_BYTES`] and
//!   [`crate::state::MAX_RECORDED_HEADER_BYTES`], so a store that grows per
//!   request grows with neither the request count nor any size a caller chooses.
//!
//! The decode and encode loops yield to the runtime every [`FRAMES_PER_YIELD`]
//! frames, so a large invocation is cancellable part-way rather than a single
//! uninterruptible block of synchronous work inside an async future.
//!
//! For the full modeled documentation of any shape:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2/smithy-model/com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2.json")
//! ```

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    Endpoint, Invocation, InvocationOutcome, ModelBehaviour, RequestFrame, ResponseFrame,
    StreamFailureKind,
};
use aws_smithy_eventstream::frame::{write_message_to, DecodedFrame, MessageFrameDecoder};
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints;
use aws_smithy_schema_server::json::JsonCodec;
use aws_smithy_schema_server::{Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, ServerHttpProtocol, Service,
};
use std::sync::Arc;
use std::time::SystemTime;

/// The operation's wire name, as the model spells it.
const OPERATION_NAME: &str = "InvokeEndpointWithBidirectionalStream";

/// `@httpHeader` names, from the model.
const HEADER_TARGET_VARIANT: &str = "X-Amzn-SageMaker-Target-Variant";
const HEADER_MODEL_INVOCATION_PATH: &str = "X-Amzn-SageMaker-Model-Invocation-Path";
const HEADER_MODEL_QUERY_STRING: &str = "X-Amzn-SageMaker-Model-Query-String";
const HEADER_INVOKED_PRODUCTION_VARIANT: &str = "X-Amzn-Invoked-Production-Variant";

/// The media type an event-stream payload travels as.
const EVENT_STREAM_CONTENT_TYPE: &str = "application/vnd.amazon.eventstream";

/// The only member of `RequestStreamEvent`, and the data-carrying member of
/// `ResponseStreamEvent`.
const MEMBER_PAYLOAD_PART: &str = "PayloadPart";
/// The two error members of `ResponseStreamEvent`.
const MEMBER_MODEL_STREAM_ERROR: &str = "ModelStreamError";
const MEMBER_INTERNAL_STREAM_FAILURE: &str = "InternalStreamFailure";

/// `CompletionState` values, from the model's `^(PARTIAL)$|^(COMPLETE)$`.
const COMPLETION_STATE_PARTIAL: &str = "PARTIAL";
const COMPLETION_STATE_COMPLETE: &str = "COMPLETE";

/// The `:content-type` an `@eventPayload` blob frame declares.
const FRAME_CONTENT_TYPE_BINARY: &str = "application/octet-stream";
/// The `:content-type` an in-stream error frame declares — its members are
/// rendered with this service's own restJson1 body codec.
const FRAME_CONTENT_TYPE_JSON: &str = "application/json";

/// Event-stream prelude headers.
const FRAME_HEADER_MESSAGE_TYPE: &str = ":message-type";
const FRAME_HEADER_EVENT_TYPE: &str = ":event-type";
const FRAME_HEADER_EXCEPTION_TYPE: &str = ":exception-type";
const FRAME_HEADER_CONTENT_TYPE: &str = ":content-type";
const FRAME_MESSAGE_TYPE_EVENT: &str = "event";
const FRAME_MESSAGE_TYPE_EXCEPTION: &str = "exception";

/// `@eventHeader` names on `RequestPayloadPart` / `ResponsePayloadPart`.
const FRAME_HEADER_DATA_TYPE: &str = "DataType";
const FRAME_HEADER_COMPLETION_STATE: &str = "CompletionState";
const FRAME_HEADER_P: &str = "P";

/// The most `RequestStreamEvent` members one invocation may carry.
///
/// A caller chooses both how many frames a body holds and how small each one is,
/// so decoding is work a caller sizes. The transport caps a request body, but a
/// capped body still holds hundreds of thousands of one-byte frames, and each one
/// becomes a decoded value and a response frame. This cap is what turns that into
/// bounded work; a stream past it is refused with the modelled
/// `InputValidationError` rather than served.
pub const MAX_REQUEST_FRAMES: usize = 1024;

/// The most `@eventPayload` bytes one invocation may carry across all its frames.
///
/// Together with [`MAX_RESPONSE_FRAMES`] this bounds the response the emulator
/// buffers: at most `MAX_REQUEST_PAYLOAD_BYTES` of payload plus
/// `MAX_RESPONSE_FRAMES` frame headers, so a caller cannot choose a product that
/// grows without limit.
pub const MAX_REQUEST_PAYLOAD_BYTES: usize = 1024 * 1024;

/// The most `ResponseStreamEvent` members one invocation may answer with.
///
/// Each frame costs a prelude, its headers and two CRCs on top of its payload, so
/// a small body re-cut at a small `chunk_size` AMPLIFIES: 1 MiB at a chunk size of
/// 1 would be a million frames. The count is therefore checked against this cap
/// BEFORE a single frame is built or a byte is allocated.
pub const MAX_RESPONSE_FRAMES: usize = 4096;

/// How many frames are processed between yields to the runtime.
///
/// Decoding and encoding an event stream is synchronous CPU work inside an async
/// future, and a future with no await point cannot be cancelled or timed out
/// part-way. Yielding every so many frames gives a cancellation somewhere to
/// land, so a large invocation can actually be stopped.
const FRAMES_PER_YIELD: usize = 64;

/// The `InvokeEndpointWithBidirectionalStream` operation.
///
/// Owns the shared engine (the constructor seam). The generated
/// [`ErasedOperation`] shim on this type declines, because the operation's output
/// binds a `@streaming` union; [`BidirectionalStreamEntry`] is what the service
/// returns from `override_operation`, and it delegates to [`Self::serve`] here.
pub struct InvokeEndpointWithBidirectionalStream<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> InvokeEndpointWithBidirectionalStream<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is
    /// a construction-time dependency, so engine access is guaranteed by the time
    /// any request runs. Called by the generated `ServiceDefinition::new(engine)`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The hand-written [`ErasedOperation`] that actually serves the request.
///
/// It exists because the generated shim cannot: driving a `@streaming` union
/// output needs the frame writer below, which the generated surface declines.
/// `crate::service::EmulatedService` holds one and returns it from
/// `override_operation` for `POST
/// /endpoints/{EndpointName}/invocations-bidirectional-stream`.
pub struct BidirectionalStreamEntry<Eng> {
    op: InvokeEndpointWithBidirectionalStream<Eng>,
}

impl<Eng> BidirectionalStreamEntry<Eng> {
    /// Construct the entry from the shared engine, the same seam every operation
    /// uses.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            op: InvokeEndpointWithBidirectionalStream::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for BidirectionalStreamEntry<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The author's `override_protocol` wins over the model-derived one,
            // the same resolution the centralized sequence applies.
            let protocol = svc
                .override_protocol()
                .unwrap_or_else(|| svc.definition().protocol());

            // Telemetry, recorded by this operation because it does not run
            // through the centralized sequence that records it for every other
            // one. Everything is gated on `recording_active()`, read once.
            let recording = emulator_telemetry::recording_active();
            let telemetry = recording.then(|| {
                (
                    std::time::Instant::now(),
                    smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers)
                        .into_metadata(),
                )
            });

            // The access-key id the request presented, read by the transport's own
            // parser. An AKID is the public half of a credential, never a secret,
            // and it is the attribution both the audit record below and the
            // transcript entry carry.
            let caller_akid = engine::extract_akid(&ctx.parts().headers);

            let response = self.op.serve(protocol, req, &caller_akid).await;

            // The structured audit record for a state-changing operation. This
            // operation does not run through the centralized sequence, so it is
            // the one place a per-request record for it can be emitted: who
            // called, what they called, and how it ended. No payload, no header
            // value and no credential secret is recorded.
            // Every request-derived value goes through the same sanitiser, so a
            // caller cannot forge or split a line. Neither a payload byte nor a
            // header value is ever recorded.
            tracing::info!(
                service = ctx.identity().signing_name(),
                operation = OPERATION_NAME,
                caller_akid = %printable(&caller_akid),
                method = %printable(req.method()),
                uri = %printable(request_path(req.uri())),
                status = response.status().as_u16(),
                "served an InvokeEndpointWithBidirectionalStream invocation"
            );

            if let Some((started, request_metadata)) = telemetry {
                let status = response.status().as_u16();
                let failure = (status >= 400).then(|| {
                    emulator_telemetry::OperationFailure::classify(status, OPERATION_NAME)
                });
                let handle = emulator_telemetry::OperationHandle::folded(
                    ctx.identity().signing_name_static(),
                    OPERATION_NAME,
                );
                emulator_telemetry::record_generated_operation_with_handle(
                    &handle,
                    started.elapsed(),
                    failure,
                    request_metadata,
                );
            }

            response
        })
    }
}

/// The `EndpointName` label of a request this operation serves, or `None` when
/// the request is not `POST
/// /endpoints/{EndpointName}/invocations-bidirectional-stream`.
///
/// This is the route match `crate::service::EmulatedService::override_operation`
/// applies, kept beside the handler so the two cannot disagree about which
/// requests this operation claims.
pub fn endpoint_name_label(req: &RtRequest) -> Option<String> {
    // An HTTP method is a case-sensitive token, so the real service would not
    // route a lowercase `post` here either.
    if req.method() != "POST" {
        return None;
    }
    let mut segments = request_path(req.uri()).split('/');
    // A path always starts with `/`, so the first split segment is empty.
    if segments.next() != Some("") {
        return None;
    }
    if segments.next() != Some("endpoints") {
        return None;
    }
    let label = segments.next()?;
    if segments.next() != Some("invocations-bidirectional-stream") || segments.next().is_some() {
        return None;
    }
    if label.is_empty() {
        return None;
    }
    // A `@httpLabel` reaches the server percent-encoded.
    Some(smithy_server_runtime::percent_decode(label))
}

/// A request-derived value as a log record may carry it: anything outside
/// printable ASCII and the space replaced, so a caller cannot forge or split a
/// line in the audit trail. An allowlist rather than `is_control`, which passes
/// U+2028/U+2029 — line terminators to some log readers — straight through.
pub(crate) fn printable(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c == ' ' || c.is_ascii_graphic() {
                c
            } else {
                ' '
            }
        })
        .collect()
}

/// The path component of a request URI, without any scheme/authority prefix,
/// query string or fragment.
///
/// The vendored protocol layer has the same helper
/// (`aws_smithy_schema_server::rest_xml::request_path`), but it is `pub(crate)`
/// there and `vendor/` is read-only from a service crate, so it cannot be reached
/// or promoted from here.
///
/// The query and the fragment are stripped FIRST, because `://` can appear inside
/// a query VALUE (`?q=a://b`) and looking for the scheme over the whole URI would
/// then find it there and mis-read the path.
fn request_path(uri: &str) -> &str {
    let no_fragment = uri.split('#').next().unwrap_or(uri);
    let no_query = no_fragment.split('?').next().unwrap_or(no_fragment);
    match no_query.find("://") {
        None => no_query,
        Some(scheme_end) => match no_query[scheme_end + 3..].find('/') {
            None => "/",
            Some(path_start) => &no_query[scheme_end + 3 + path_start..],
        },
    }
}

/// A modelled failure this operation answers with as a TOP-LEVEL HTTP error,
/// before any response frame has been flushed.
enum Failure {
    /// `InputValidationError`, HTTP 400.
    InputValidation(InputValidationError),
    /// `InternalServerError`, HTTP 500.
    InternalServer(InternalServerError),
    /// `ModelError`, HTTP 424.
    Model(ModelError),
}

impl Failure {
    /// This failure's wire shape name, HTTP status and serializable body — all
    /// three from the model.
    fn parts(&self) -> (&'static str, u16, &dyn SerializableStruct, &'static Schema) {
        match self {
            Self::InputValidation(e) => (
                "InputValidationError",
                400,
                e as &dyn SerializableStruct,
                &SCHEMA_INPUT_VALIDATION_ERROR,
            ),
            Self::InternalServer(e) => (
                "InternalServerError",
                500,
                e as &dyn SerializableStruct,
                &SCHEMA_INTERNAL_SERVER_ERROR,
            ),
            Self::Model(e) => ("ModelError", 424, e as &dyn SerializableStruct, &SCHEMA_MODEL_ERROR),
        }
    }

    /// Render the failure in this service's own protocol envelope.
    fn render(&self, protocol: &dyn ServerHttpProtocol) -> RtResponse {
        let (code, status, body, schema) = self.parts();
        protocol
            .serialize_error(body, schema, code, status)
            .unwrap_or_else(|_| {
                // The envelope could not be written, which is a server fault
                // rather than anything the caller can fix. Answer the modelled
                // server error's status with an empty body rather than panicking.
                RtResponse::new(
                    StatusCode::try_from(500u16).expect("500 is a valid status"),
                    SdkBody::empty(),
                )
            })
    }
}

/// `InputValidationError` carrying the clause the modelled constraints produced.
///
/// `ErrorCode` is the error shape's own name: the model declares the member but
/// AWS publishes no code vocabulary for it, so the one value this emulator can
/// state from the model is the shape it is answering with.
fn input_validation(violations: &[Violation]) -> Failure {
    input_validation_message(
        constraints::envelope(violations).unwrap_or_else(|| "invalid request".to_string()),
    )
}

/// `InputValidationError` carrying `message`.
fn input_validation_message(message: impl Into<String>) -> Failure {
    Failure::InputValidation(InputValidationError {
        message: Some(message.into()),
        error_code: Some("InputValidationError".to_string()),
    })
}

/// `InternalServerError` carrying `message`.
fn internal_server_error(message: impl Into<String>) -> Failure {
    Failure::InternalServer(InternalServerError {
        message: Some(message.into()),
        error_code: Some("InternalServerError".to_string()),
    })
}

impl<Eng: Send + Sync + 'static> InvokeEndpointWithBidirectionalStream<Eng> {
    /// Serve one invocation, rendering a modelled failure in this service's own
    /// envelope.
    async fn serve(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &RtRequest,
        caller_akid: &str,
    ) -> RtResponse {
        match self.invoke(req, caller_akid).await {
            Ok(response) => response,
            Err(failure) => failure.render(protocol),
        }
    }

    /// The invocation itself: validate, decode the request stream, resolve the
    /// endpoint and its variant out of the registry, apply that endpoint's
    /// declared behaviour, record the transcript, and frame the answer.
    async fn invoke(&self, req: &RtRequest, caller_akid: &str) -> Result<RtResponse, Failure> {
        // The shared store, reached through the engine held on `self`. NEVER a
        // fresh engine: that would hand this request a private store.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            internal_server_error(
                "this service is mounted on an engine that holds no state for it",
            )
        })?;

        let endpoint_name = endpoint_name_label(req).ok_or_else(|| {
            input_validation_message(
                "the request URI is not POST \
                 /endpoints/{EndpointName}/invocations-bidirectional-stream",
            )
        })?;

        let input = InvokeEndpointWithBidirectionalStreamInput {
            endpoint_name,
            target_variant: header(req, HEADER_TARGET_VARIANT),
            model_invocation_path: header(req, HEADER_MODEL_INVOCATION_PATH),
            model_query_string: header(req, HEADER_MODEL_QUERY_STRING),
        };

        // The model's own `@length`/`@pattern` traits, enforced by the framework's
        // validating walk over the built value. The bounds are the model's and are
        // never restated here.
        let violations = constraints::validate(
            &input,
            INVOKE_ENDPOINT_WITH_BIDIRECTIONAL_STREAM_INPUT_SCHEMA,
        );
        if !violations.is_empty() {
            return Err(input_validation(&violations));
        }

        // The `@httpPayload` is a `@streaming` union, which travels only as
        // `vnd.amazon.event-stream`; a body declaring any other media type is not
        // this operation's payload, however it happens to decode.
        require_event_stream_content_type(req)?;

        let body = req.body().bytes().ok_or_else(|| {
            internal_server_error("the request body did not reach this operation as bytes")
        })?;
        let request_frames = decode_request_frames(body).await?;

        // The endpoint registry decides whether this invocation can happen at all.
        let endpoint: Endpoint = state
            .lock()
            .endpoints
            .get(&input.endpoint_name)
            .cloned()
            .ok_or_else(|| {
                input_validation_message(format!(
                    "Endpoint {} not found",
                    input.endpoint_name
                ))
            })?;

        let variant = select_variant(&endpoint, input.target_variant.as_deref())?;

        // The size of the answer is checked BEFORE it is built, so an
        // amplification a caller could ask for is refused rather than allocated.
        let answer = answer_of(&endpoint.behaviour, &request_frames)?;

        let mut invocation = Invocation {
            caller_akid: caller_akid.to_string(),
            endpoint_name: endpoint.name.clone(),
            target_variant: input.target_variant.clone(),
            invoked_production_variant: variant.clone(),
            model_invocation_path: input.model_invocation_path.clone(),
            model_query_string: input.model_query_string.clone(),
            // Bounded projections: the frames' shape and true lengths, with the
            // frame count, the payload bytes and the event-header strings all cut
            // to the transcript's per-entry budgets. The true counts travel beside
            // them, so a truncated entry never misreports what happened. The
            // response total counts `PayloadPart` members; a terminating in-stream
            // error member is reported by `terminal`, not counted here.
            request_frames: crate::state::retain_request_frames(&request_frames),
            request_frames_total: request_frames.len(),
            response_frames: crate::state::retain_response_frames(&answer.frames),
            response_frames_total: answer.frames.len(),
            terminal: InvocationOutcome::Completed,
            started_at: SystemTime::now(),
        };

        // A refused invocation is still an invocation: the frames were received,
        // so the transcript records it before the failure is returned.
        if let Some(model_error) = answer.model_error.as_ref() {
            invocation.terminal = InvocationOutcome::ModelError {
                original_status_code: model_error.original_status_code.unwrap_or_default(),
                error_code: model_error.error_code.clone().unwrap_or_default(),
            };
            state.lock().record_invocation(invocation);
            return Err(Failure::Model(model_error.clone()));
        }

        invocation.terminal = match &answer.stream_error {
            None => InvocationOutcome::Completed,
            Some(StreamFailureKind::ModelStreamError {
                message,
                error_code,
            }) => InvocationOutcome::ModelStreamError {
                message: message.clone(),
                error_code: error_code.clone(),
            },
            Some(StreamFailureKind::InternalStreamFailure { message }) => {
                InvocationOutcome::InternalStreamFailure {
                    message: message.clone(),
                }
            }
        };

        // Recorded on EVERY branch from here on, and recorded exactly ONCE, with
        // the outcome that branch produced. Framing runs first so the outcome is
        // known before the entry exists — the entry is never written and then
        // corrected, which under concurrency could only be identified as "the
        // last one" and so could stamp another caller's entry.
        let body = match encode_response_stream(&answer).await {
            Ok(body) => body,
            Err(failure) => {
                invocation.terminal = InvocationOutcome::ResponseFramingFailed;
                state.lock().record_invocation(invocation);
                return Err(failure);
            }
        };
        state.lock().record_invocation(invocation);

        let mut response = RtResponse::new(
            StatusCode::try_from(200u16).expect("200 is a valid status"),
            SdkBody::from(body),
        );
        response
            .headers_mut()
            .insert("content-type", EVENT_STREAM_CONTENT_TYPE);
        response
            .headers_mut()
            .try_insert(HEADER_INVOKED_PRODUCTION_VARIANT, variant)
            .map_err(|e| {
                internal_server_error(format!(
                    "the selected production variant is not a valid HTTP header value: {e}"
                ))
            })?;
        Ok(response)
    }
}

/// One `@httpHeader`-bound member's value, when the request carried it.
fn header(req: &RtRequest, name: &str) -> Option<String> {
    req.headers().get(name).map(str::to_string)
}

/// Refuse a request whose body does not declare the event-stream media type.
///
/// The operation's `@httpPayload` targets a `@streaming` union, which travels
/// only as [`EVENT_STREAM_CONTENT_TYPE`], so a body declaring anything else — or
/// nothing at all — is not this operation's payload. Media-type parameters (a
/// `; charset=…`) are allowed and the type itself is matched case-insensitively,
/// as HTTP defines it.
fn require_event_stream_content_type(req: &RtRequest) -> Result<(), Failure> {
    let declared = req.headers().get("content-type").ok_or_else(|| {
        input_validation_message(format!(
            "the request declares no Content-Type; this operation's Body is an event stream \
             and travels as {EVENT_STREAM_CONTENT_TYPE}"
        ))
    })?;
    let media_type = declared
        .split(';')
        .next()
        .unwrap_or(declared)
        .trim()
        .to_ascii_lowercase();
    if media_type != EVENT_STREAM_CONTENT_TYPE {
        return Err(input_validation_message(format!(
            "the request declares Content-Type {media_type}; this operation's Body is an event \
             stream and travels as {EVENT_STREAM_CONTENT_TYPE}"
        )));
    }
    Ok(())
}

/// The variant an invocation is routed to: the `TargetVariant` when it names one
/// of THIS endpoint's production variants, otherwise the endpoint's default
/// variant. A `TargetVariant` the endpoint does not have is refused — it names no
/// variant that could serve the request.
fn select_variant(endpoint: &Endpoint, target_variant: Option<&str>) -> Result<String, Failure> {
    match target_variant {
        None => Ok(endpoint.default_variant.clone()),
        Some(requested)
            if endpoint
                .production_variants
                .iter()
                .any(|variant| variant == requested) =>
        {
            Ok(requested.to_string())
        }
        Some(requested) => {
            // The body names only what was rejected: a validation error is no
            // place to publish an endpoint's variant inventory. The inventory
            // goes to the diagnostic record instead.
            tracing::debug!(
                endpoint = %endpoint.name,
                variants = %endpoint.production_variants.join(", "),
                "refused a TargetVariant the endpoint does not have"
            );
            Err(input_validation_message(format!(
                "Endpoint {} has no production variant named {requested}",
                endpoint.name
            )))
        }
    }
}

/// What an endpoint answers with: the `PayloadPart` frames, and either an
/// in-stream failure that terminates them or a top-level `ModelError` that
/// replaces the whole response.
struct Answer {
    frames: Vec<ResponseFrame>,
    stream_error: Option<StreamFailureKind>,
    model_error: Option<ModelError>,
}

/// Apply an endpoint's declared behaviour to the frames the request carried,
/// refusing an answer that would exceed [`MAX_RESPONSE_FRAMES`] before any frame
/// is built.
fn answer_of(
    behaviour: &ModelBehaviour,
    request_frames: &[RequestFrame],
) -> Result<Answer, Failure> {
    match behaviour {
        ModelBehaviour::Echo {
            chunk_size,
            data_type,
        } => Ok(Answer {
            frames: echo_frames(request_frames, *chunk_size, data_type, None)?,
            stream_error: None,
            model_error: None,
        }),
        ModelBehaviour::Scripted { frames } => {
            // A script comes from the registry rather than from a caller, so an
            // over-long one is a misconfigured endpoint, not bad input.
            if frames.len() > MAX_RESPONSE_FRAMES {
                // The numbers are registry configuration, so they go to the
                // diagnostic record rather than into a body a caller reads — the
                // same split `select_variant` makes for a variant inventory.
                tracing::warn!(
                    scripted_frames = frames.len(),
                    max_response_frames = MAX_RESPONSE_FRAMES,
                    "an endpoint is configured with more scripted response frames than one \
                     invocation may emit"
                );
                return Err(internal_server_error(
                    "this endpoint's response-framing configuration exceeds the number of \
                     frames one invocation may emit",
                ));
            }
            Ok(Answer {
                frames: frames.clone(),
                stream_error: None,
                model_error: None,
            })
        }
        ModelBehaviour::FailWithModelError {
            original_status_code,
            original_message,
            log_stream_arn,
            error_code,
        } => Ok(Answer {
            frames: Vec::new(),
            stream_error: None,
            model_error: Some(ModelError {
                message: Some(format!(
                    "Received client error ({original_status_code}) from model with message \
                     \"{original_message}\"."
                )),
                original_status_code: Some(*original_status_code),
                original_message: Some(original_message.clone()),
                log_stream_arn: Some(log_stream_arn.clone()),
                error_code: Some(error_code.clone()),
            }),
        }),
        ModelBehaviour::FailMidStream {
            chunk_size,
            data_type,
            frames_before_failure,
            stream_error,
        } => {
            // Only `frames_before_failure` frames ever reach the caller, so only
            // that many are built — and the response-frame cap is therefore
            // evaluated against what is actually emitted, not against an echo
            // that is discarded. Without this, a large payload at a small
            // `chunk_size` would answer the cap's 500 instead of the mid-stream
            // failure the endpoint declares.
            let mut frames = echo_frames(
                request_frames,
                *chunk_size,
                data_type,
                Some(*frames_before_failure),
            )?;
            frames.truncate(*frames_before_failure);
            // The stream never completes, so no frame that reaches the caller may
            // claim it did.
            for frame in &mut frames {
                frame.completion_state = Some(COMPLETION_STATE_PARTIAL.to_string());
            }
            Ok(Answer {
                frames,
                stream_error: Some(stream_error.clone()),
                model_error: None,
            })
        }
    }
}

/// The echo rule: every request frame's `Bytes`, concatenated, re-cut into chunks
/// of at most `chunk_size` bytes, each frame carrying `data_type`, all `PARTIAL`
/// but the last, which is `COMPLETE`.
///
/// The frame boundaries are the endpoint's (`chunk_size`), not the caller's, so
/// the response framing is a function of the registry and not a replay of the
/// request framing.
///
/// The frame COUNT is computed and checked against [`MAX_RESPONSE_FRAMES`] before
/// anything is allocated, because a small `chunk_size` amplifies a caller-chosen
/// payload into many frames, each of which costs a prelude, its headers and two
/// CRCs. `emit_at_most` bounds how many frames the caller will actually be sent
/// (a mid-stream failure cuts the answer short), so the cap is evaluated against
/// what is emitted rather than against an answer that is discarded.
fn echo_frames(
    request_frames: &[RequestFrame],
    chunk_size: usize,
    data_type: &str,
    emit_at_most: Option<usize>,
) -> Result<Vec<ResponseFrame>, Failure> {
    let chunk_size = chunk_size.max(1);
    let total: usize = request_frames.iter().map(|frame| frame.bytes.len()).sum();
    let total = match emit_at_most {
        None => total,
        Some(frames) => total.min(frames.saturating_mul(chunk_size)),
    };
    let frame_count = total.div_ceil(chunk_size).max(1);
    if frame_count > MAX_RESPONSE_FRAMES {
        // The frame count is the product of a REGISTRY setting (`chunk_size`) and
        // a payload the caller was allowed to send, so the fault is the
        // endpoint's configuration, not the request: attributed as a server error,
        // the same way an over-long scripted answer is. AWS returns no 4xx for
        // this cause, and this cap is an emulator limit, so blaming the client
        // would exercise a client branch that production never reaches. The
        // numbers stay in the diagnostic record, not in the body.
        tracing::warn!(
            chunk_size,
            request_bytes = total,
            frame_count,
            max_response_frames = MAX_RESPONSE_FRAMES,
            "an endpoint's chunk size would need more response frames than one invocation \
             may emit"
        );
        return Err(internal_server_error(
            "this endpoint's response-framing configuration exceeds the number of frames one \
             invocation may emit",
        ));
    }

    let payload: Vec<u8> = request_frames
        .iter()
        .flat_map(|frame| frame.bytes.iter().copied())
        .take(total)
        .collect();
    let mut frames: Vec<ResponseFrame> = payload
        .chunks(chunk_size)
        .map(|chunk| {
            ResponseFrame::new(
                chunk.to_vec(),
                Some(data_type.to_string()),
                Some(COMPLETION_STATE_PARTIAL.to_string()),
                None,
            )
        })
        .collect();
    // An empty answer is still one frame: a response stream that carries no
    // member at all would never tell the caller it had completed.
    if frames.is_empty() {
        frames.push(ResponseFrame::new(
            Vec::new(),
            Some(data_type.to_string()),
            None,
            None,
        ));
    }
    if let Some(last) = frames.last_mut() {
        last.completion_state = Some(COMPLETION_STATE_COMPLETE.to_string());
    }
    Ok(frames)
}

/// Decode the request's `@httpPayload` event stream into `RequestStreamEvent`
/// members, in wire order.
///
/// Every frame in the body is consumed, in order, until a frame whose
/// `CompletionState` is `COMPLETE` — after which the body must be exhausted, or
/// the stream is refused — or the body ends.
///
/// A body that ends without any frame declaring `COMPLETE` is taken as complete
/// at end-of-body rather than refused: `RequestPayloadPart$CompletionState` is an
/// OPTIONAL member, so the model permits a stream in which no frame declares one
/// at all, and refusing that would refuse a request the model allows.
///
/// Bounded: a stream with more than [`MAX_REQUEST_FRAMES`] members, or carrying
/// more than [`MAX_REQUEST_PAYLOAD_BYTES`] of payload, is refused as soon as it
/// passes the cap rather than decoded to the end. The loop yields every
/// [`FRAMES_PER_YIELD`] frames so the work can be cancelled part-way.
async fn decode_request_frames(body: &[u8]) -> Result<Vec<RequestFrame>, Failure> {
    let mut decoder = MessageFrameDecoder::new();
    let mut remaining: &[u8] = body;
    let mut frames: Vec<RequestFrame> = Vec::new();
    let mut payload_bytes: usize = 0;

    while !remaining.is_empty() {
        if frames.len() % FRAMES_PER_YIELD == FRAMES_PER_YIELD - 1 {
            tokio::task::yield_now().await;
        }
        let message = match decoder.decode_frame(&mut remaining) {
            Ok(DecodedFrame::Complete(message)) => message,
            Ok(DecodedFrame::Incomplete) => {
                return Err(input_validation_message(
                    "the request event stream ends in the middle of a frame",
                ));
            }
            Err(e) => {
                return Err(input_validation_message(format!(
                    "the request body is not a valid {EVENT_STREAM_CONTENT_TYPE} frame \
                     sequence: {e}"
                )));
            }
        };
        // The payload cap is read off the message's own length, BEFORE the blob
        // is copied out of it.
        payload_bytes = payload_bytes.saturating_add(message.payload().len());
        if payload_bytes > MAX_REQUEST_PAYLOAD_BYTES {
            return Err(input_validation_message(format!(
                "the request event stream carries more than the {MAX_REQUEST_PAYLOAD_BYTES} \
                 payload bytes one invocation may carry"
            )));
        }
        if frames.len() == MAX_REQUEST_FRAMES {
            return Err(input_validation_message(format!(
                "the request event stream carries more than the {MAX_REQUEST_FRAMES} frames \
                 one invocation may carry"
            )));
        }

        let frame = request_frame_of(&message)?;
        let complete = frame.completion_state.as_deref() == Some(COMPLETION_STATE_COMPLETE);
        frames.push(frame);
        if complete {
            // The caller said it had nothing more to send, so anything still in
            // the body is not part of this stream. Refused rather than silently
            // discarded: dropping it would accept a mis-framed stream, and would
            // leave the transcript's own frame total describing only the prefix.
            if !remaining.is_empty() {
                return Err(input_validation_message(
                    "the request event stream carries bytes after the frame whose \
                     CompletionState is COMPLETE",
                ));
            }
            break;
        }
    }

    if frames.is_empty() {
        // `Body` is `@required`, and a stream carrying no member carries no value
        // for it. Worded by the framework's own refusal machinery.
        return Err(input_validation(&[Violation {
            path: constraints::path::member("Body"),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }]));
    }
    Ok(frames)
}

/// One decoded event-stream message as a `RequestStreamEvent` member.
///
/// Every prelude header the protocol requires must be present AND string-valued:
/// a frame that omits `:message-type`, or sends a header the model names with the
/// wrong value type, is refused rather than read as if the member had not been
/// sent. Reading a mistyped header as absent would silently change the stream's
/// meaning — a `CompletionState` sent as an integer would never terminate the
/// decode loop.
fn request_frame_of(message: &Message) -> Result<RequestFrame, Failure> {
    let message_type = string_header(message, FRAME_HEADER_MESSAGE_TYPE)?.ok_or_else(|| {
        input_validation_message(format!(
            "a request event-stream frame carries no {FRAME_HEADER_MESSAGE_TYPE} header; every \
             message of an event stream declares one"
        ))
    })?;
    if message_type != FRAME_MESSAGE_TYPE_EVENT {
        return Err(input_validation_message(format!(
            "a request event-stream frame declares {FRAME_HEADER_MESSAGE_TYPE} \
             {message_type}, but a RequestStreamEvent frame is a \
             {FRAME_MESSAGE_TYPE_EVENT}"
        )));
    }
    let event_type = string_header(message, FRAME_HEADER_EVENT_TYPE)?.ok_or_else(|| {
        input_validation_message(format!(
            "a request event-stream frame carries no {FRAME_HEADER_EVENT_TYPE} header, so it \
             names no member of RequestStreamEvent"
        ))
    })?;
    if event_type != MEMBER_PAYLOAD_PART {
        return Err(input_validation_message(format!(
            "RequestStreamEvent has no member named {event_type}; its only member is \
             {MEMBER_PAYLOAD_PART}"
        )));
    }

    // The `@eventHeader` members, held exactly as the frame carried them, and the
    // `@eventPayload` blob.
    let part = RequestPayloadPart {
        bytes: Some(message.payload().to_vec()),
        data_type: string_header(message, FRAME_HEADER_DATA_TYPE)?,
        completion_state: string_header(message, FRAME_HEADER_COMPLETION_STATE)?,
        p: string_header(message, FRAME_HEADER_P)?,
    };
    // The model's `@pattern` on `DataType` and `CompletionState`, enforced by the
    // same validating walk the request's HTTP-bound members go through.
    let violations = constraints::validate(&part, REQUEST_PAYLOAD_PART_SCHEMA);
    if !violations.is_empty() {
        return Err(input_validation(&violations));
    }

    Ok(RequestFrame::new(
        part.bytes.unwrap_or_default(),
        part.data_type,
        part.completion_state,
        part.p,
    ))
}

/// A string-valued event-stream header: `Ok(None)` when the frame carries none
/// under `name`, `Err` when it carries one that is not string-valued.
///
/// The two cases are distinguished deliberately. Folding "present with the wrong
/// value type" into "absent" would accept a frame AWS refuses and would change
/// what the stream means.
fn string_header(message: &Message, name: &str) -> Result<Option<String>, Failure> {
    let Some(header) = message
        .headers()
        .iter()
        .find(|header| header.name().as_str() == name)
    else {
        return Ok(None);
    };
    match header.value() {
        HeaderValue::String(value) => Ok(Some(value.as_str().to_string())),
        other => Err(input_validation_message(format!(
            "the event-stream header {name} must be a string, but this frame sends it as \
             {}",
            header_value_kind(other)
        ))),
    }
}

/// The name of a non-string event-stream header value's type, for a refusal that
/// says what was wrong without echoing the value.
fn header_value_kind(value: &HeaderValue) -> &'static str {
    match value {
        HeaderValue::Bool(_) => "a boolean",
        HeaderValue::Byte(_) => "a byte",
        HeaderValue::Int16(_) => "an int16",
        HeaderValue::Int32(_) => "an int32",
        HeaderValue::Int64(_) => "an int64",
        HeaderValue::ByteArray(_) => "a byte array",
        HeaderValue::String(_) => "a string",
        HeaderValue::Timestamp(_) => "a timestamp",
        HeaderValue::Uuid(_) => "a uuid",
        _ => "a value of an unknown type",
    }
}

/// Frame the whole response stream: one `PayloadPart` message per frame, then the
/// in-stream failure member when the endpoint declared one.
///
/// For the echo-derived behaviours the buffer this builds is bounded by the SUM
/// of [`MAX_RESPONSE_FRAMES`] and [`MAX_REQUEST_PAYLOAD_BYTES`] rather than their
/// product, because every frame's payload is re-cut from a request payload that
/// is already capped. A `Scripted` answer is bounded in frame COUNT only: its
/// bytes come from the registry, are already resident in the store, and are
/// chosen by whoever registered the endpoint rather than by a caller. The loop
/// yields every [`FRAMES_PER_YIELD`] frames so the work can be cancelled
/// part-way.
async fn encode_response_stream(answer: &Answer) -> Result<Vec<u8>, Failure> {
    let mut body: Vec<u8> = Vec::new();
    for (index, frame) in answer.frames.iter().enumerate() {
        if index % FRAMES_PER_YIELD == FRAMES_PER_YIELD - 1 {
            tokio::task::yield_now().await;
        }
        write_frame(&mut body, &payload_part_message(frame))?;
    }
    if let Some(stream_error) = answer.stream_error.as_ref() {
        write_frame(&mut body, &stream_error_message(stream_error)?)?;
    }
    Ok(body)
}

/// Append one framed message to the response body.
fn write_frame(body: &mut Vec<u8>, message: &Message) -> Result<(), Failure> {
    write_message_to(message, body).map_err(|e| {
        internal_server_error(format!("a response event-stream frame could not be written: {e}"))
    })
}

/// A `ResponseStreamEvent$PayloadPart` message: the `@eventHeader` members as
/// event-stream headers and the `@eventPayload` blob as the message payload.
fn payload_part_message(frame: &ResponseFrame) -> Message {
    let mut message = Message::new(frame.bytes.clone())
        .add_header(Header::new(
            FRAME_HEADER_MESSAGE_TYPE,
            HeaderValue::String(FRAME_MESSAGE_TYPE_EVENT.into()),
        ))
        .add_header(Header::new(
            FRAME_HEADER_EVENT_TYPE,
            HeaderValue::String(MEMBER_PAYLOAD_PART.into()),
        ))
        .add_header(Header::new(
            FRAME_HEADER_CONTENT_TYPE,
            HeaderValue::String(FRAME_CONTENT_TYPE_BINARY.into()),
        ));
    for (name, value) in [
        (FRAME_HEADER_DATA_TYPE, frame.data_type.as_deref()),
        (
            FRAME_HEADER_COMPLETION_STATE,
            frame.completion_state.as_deref(),
        ),
        (FRAME_HEADER_P, frame.p.as_deref()),
    ] {
        if let Some(value) = value {
            message = message.add_header(Header::new(
                name,
                HeaderValue::String(value.to_string().into()),
            ));
        }
    }
    message
}

/// The in-stream error member that terminates a failed response stream, framed as
/// an event-stream `exception` whose payload is the modelled error's own body.
fn stream_error_message(kind: &StreamFailureKind) -> Result<Message, Failure> {
    let (member, payload) = match kind {
        StreamFailureKind::ModelStreamError {
            message,
            error_code,
        } => (
            MEMBER_MODEL_STREAM_ERROR,
            render_json(
                &ModelStreamError {
                    message: Some(message.clone()),
                    error_code: Some(error_code.clone()),
                },
                &SCHEMA_MODEL_STREAM_ERROR,
            )?,
        ),
        StreamFailureKind::InternalStreamFailure { message } => (
            MEMBER_INTERNAL_STREAM_FAILURE,
            render_json(
                &InternalStreamFailure {
                    message: Some(message.clone()),
                },
                &SCHEMA_INTERNAL_STREAM_FAILURE,
            )?,
        ),
    };
    Ok(Message::new(payload)
        .add_header(Header::new(
            FRAME_HEADER_MESSAGE_TYPE,
            HeaderValue::String(FRAME_MESSAGE_TYPE_EXCEPTION.into()),
        ))
        .add_header(Header::new(
            FRAME_HEADER_EXCEPTION_TYPE,
            HeaderValue::String(member.to_string().into()),
        ))
        .add_header(Header::new(
            FRAME_HEADER_CONTENT_TYPE,
            HeaderValue::String(FRAME_CONTENT_TYPE_JSON.into()),
        )))
}

/// Render a modelled shape with this service's own restJson1 body codec, so an
/// in-stream error member's member names are the model's.
fn render_json(value: &dyn SerializableStruct, schema: &Schema) -> Result<Vec<u8>, Failure> {
    let mut serializer = JsonCodec::rest_json().create_serializer();
    serializer.write_struct(schema, value).map_err(|e| {
        internal_server_error(format!("an in-stream error member could not be written: {e}"))
    })?;
    Ok(serializer.finish())
}

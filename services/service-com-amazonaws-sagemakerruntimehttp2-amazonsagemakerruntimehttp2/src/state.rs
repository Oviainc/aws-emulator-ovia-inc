// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2 service.
//!
//! `SageMaker Runtime HTTP2` is a DATA plane: its model binds exactly one
//! operation, `InvokeEndpointWithBidirectionalStream`, and declares no
//! control-plane operation that could create an endpoint. The emulator therefore
//! owns the endpoint registry itself: [`Endpoint`] records live here, keyed by
//! `EndpointName`, and every invocation resolves its endpoint, its production
//! variants and the model behaviour it answers with out of this store. Invoking a
//! name the registry does not hold cannot succeed.
//!
//! The registry is SEEDED at construction (see the [`Default`] impl) because this
//! service has no operation that could register one, and is writable through
//! [`ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State::register_endpoint`]
//! so a test — or a later control-plane wiring — can add its own.
//!
//! The emulator does NOT run a machine-learning model: there is no model artifact
//! to execute. What each endpoint carries instead is a DECLARED
//! [`ModelBehaviour`], held in this store, and the response frame sequence is
//! computed from the request frames under that declared behaviour. Nothing about
//! a response is invented at request time — it is a function of the frames
//! received and the behaviour this registry holds.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// Per-service state for com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2. A handler reaches it through [`service_state`].
///
/// `BTreeMap` rather than `HashMap` for the registry: the emulator reports
/// variants and endpoint names in a deterministic order, which a `HashMap`
/// cannot promise.
#[derive(Debug)]
pub struct ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State {
    /// Every endpoint this emulator can serve, keyed by `EndpointName`.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Ordered transcript of the most recent invocations, newest last.
    ///
    /// The one place a caller can observe what the emulator received and what it
    /// produced, because the service models no operation that reports it. It
    /// covers the invocations that reached endpoint resolution: a request refused
    /// earlier (an unroutable URI, a value outside a modelled constraint, a
    /// malformed or over-cap event stream, an unregistered `EndpointName`, a
    /// `TargetVariant` the endpoint does not have) never becomes an entry, so an
    /// absent entry does not mean no request arrived — the handler's audit record
    /// covers those.
    ///
    /// BOUNDED, in every dimension a caller chooses. It is appended to on every
    /// served invocation, so it is a store that grows per request; left unbounded
    /// it would retain caller-chosen bytes for the life of the process. Four caps
    /// hold it down, all applied before anything is retained:
    ///
    /// * at most [`MAX_RECORDED_INVOCATIONS`] entries, the oldest evicted —
    ///   enforced by [`Self::record_invocation`], the one way an entry is added;
    /// * at most [`MAX_RECORDED_FRAMES_PER_INVOCATION`] frames per entry per
    ///   direction, the rest not visited at all (each entry still reports the
    ///   true `request_frames_total` / `response_frames_total`);
    /// * at most [`MAX_RECORDED_INVOCATION_BYTES`] payload bytes per entry per
    ///   direction, each affected frame still reporting its true `byte_len`;
    /// * at most [`MAX_RECORDED_HEADER_BYTES`] per retained event-header string,
    ///   which is what bounds `P` — the one event header the model constrains
    ///   neither in length nor by pattern.
    ///
    /// The last three are enforced by [`retain_request_frames`] /
    /// [`retain_response_frames`]. So one entry retains at most
    /// `2 * (MAX_RECORDED_INVOCATION_BYTES + MAX_RECORDED_FRAMES_PER_INVOCATION *
    /// 3 * MAX_RECORDED_HEADER_BYTES)` of caller-chosen bytes, plus
    /// `ModelInvocationPath` and `ModelQueryString`, which the MODEL bounds at 100
    /// and 2048 characters and the handler validates against those bounds before
    /// building the entry. Multiply by [`MAX_RECORDED_INVOCATIONS`] for the whole
    /// transcript: a fixed ceiling, whatever arrives.
    pub invocations: Vec<Invocation>,
}

/// The most invocations the transcript retains.
///
/// A diagnostic window, not a log: the transcript exists so a caller can see the
/// frames of the invocations it just made, which needs the recent ones, not all
/// of them. The service models no operation that could delete an entry, so
/// without this cap the only bound would be the process's memory.
pub const MAX_RECORDED_INVOCATIONS: usize = 256;

/// The most payload bytes one transcript entry retains, counted separately over
/// its request frames and its response frames.
///
/// The frames' SHAPE — how many there were, their order, their event headers and
/// each one's true length — is what the transcript is for; the whole payload is
/// not, and a caller chooses how big that is. Bytes past this budget are dropped
/// and each affected frame still reports its full `byte_len`.
pub const MAX_RECORDED_INVOCATION_BYTES: usize = 64 * 1024;

/// The most frames one transcript entry retains, counted separately over its
/// request frames and its response frames.
///
/// A caller chooses how many frames its stream holds, and each retained frame
/// costs its event headers even when its payload bytes are all dropped, so the
/// COUNT needs its own cap: frames past this one are not visited at all. The
/// entry still reports the true totals.
pub const MAX_RECORDED_FRAMES_PER_INVOCATION: usize = 64;

/// The most bytes one retained event-header string keeps.
///
/// `DataType` and `CompletionState` are already short by their modelled
/// `@pattern`, but `RequestPayloadPart$P` carries neither a `@length` nor a
/// `@pattern`, so its length is entirely the caller's. Truncation is on a UTF-8
/// character boundary.
pub const MAX_RECORDED_HEADER_BYTES: usize = 256;

/// One registered inference endpoint.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// The `EndpointName` an invocation addresses this endpoint by.
    pub name: String,
    /// Every production variant behind this endpoint, in registration order.
    pub production_variants: Vec<String>,
    /// The variant an invocation that names no `TargetVariant` is routed to.
    /// Always one of [`Self::production_variants`].
    pub default_variant: String,
    /// How this endpoint's model answers. Declared, never inferred.
    pub behaviour: ModelBehaviour,
    /// When this endpoint entered the registry.
    pub created_at: SystemTime,
}

/// How a registered endpoint's model answers an invocation.
///
/// The emulator runs no model artifact; each endpoint declares its answer rule
/// here and the handler applies it to the frames the request carried.
#[derive(Debug, Clone)]
pub enum ModelBehaviour {
    /// Answer with the accumulated request bytes, re-framed into chunks of at
    /// most `chunk_size` bytes, every frame carrying `data_type` as its
    /// `DataType`. The frame boundaries are the endpoint's, not the caller's.
    Echo {
        /// Maximum `Bytes` length of one response frame. Must be non-zero; a
        /// zero is read as one byte per frame.
        chunk_size: usize,
        /// The `DataType` event header every response frame carries.
        data_type: String,
    },
    /// Answer with exactly these frames, as held in the registry. The
    /// state-driven answer: nothing of the request reaches the response.
    Scripted {
        /// The frames to emit, in order.
        frames: Vec<ResponseFrame>,
    },
    /// Refuse the invocation with the modelled `ModelError` (HTTP 424), built
    /// from these recorded values.
    FailWithModelError {
        /// `ModelError$OriginalStatusCode`.
        original_status_code: i32,
        /// `ModelError$OriginalMessage`.
        original_message: String,
        /// `ModelError$LogStreamArn`.
        log_stream_arn: String,
        /// `ModelError$ErrorCode`.
        error_code: String,
    },
    /// Flush `frames_before_failure` leading `PayloadPart` frames — the first
    /// frames the [`ModelBehaviour::Echo`] rule would produce for the same
    /// `chunk_size`/`data_type`, all still `PARTIAL` because the stream never
    /// completes — and then deliver `stream_error` as a member of the RESPONSE
    /// event stream, after the 200 and after those frames.
    ///
    /// `chunk_size` and `data_type` carry the same meaning as on
    /// [`ModelBehaviour::Echo`], so a mid-stream failure frames its leading
    /// frames by exactly the rule a successful invocation would.
    FailMidStream {
        /// See [`ModelBehaviour::Echo::chunk_size`].
        chunk_size: usize,
        /// See [`ModelBehaviour::Echo::data_type`].
        data_type: String,
        /// How many leading `PayloadPart` frames reach the caller before the
        /// failure. Clamped to the number of frames the echo rule produces.
        frames_before_failure: usize,
        /// The in-stream failure that terminates the response stream.
        stream_error: StreamFailureKind,
    },
}

/// Which member of `ResponseStreamEvent` terminates a failed response stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamFailureKind {
    /// `ResponseStreamEvent$ModelStreamError`.
    ModelStreamError {
        /// `ModelStreamError$Message`.
        message: String,
        /// `ModelStreamError$ErrorCode`.
        error_code: String,
    },
    /// `ResponseStreamEvent$InternalStreamFailure`.
    InternalStreamFailure {
        /// `InternalStreamFailure$Message`.
        message: String,
    },
}

/// One `RequestStreamEvent$PayloadPart` the emulator decoded off the wire.
///
/// `bytes` is the frame's `@eventPayload`; the three optional strings are its
/// `@eventHeader` members, held exactly as the frame carried them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestFrame {
    /// `RequestPayloadPart$Bytes`. In a TRANSCRIPT entry this may be a prefix —
    /// see [`MAX_RECORDED_INVOCATION_BYTES`]; [`Self::byte_len`] is always the
    /// frame's full length.
    pub bytes: Vec<u8>,
    /// The frame's full `@eventPayload` length, whether or not every byte of it
    /// is retained.
    pub byte_len: usize,
    /// `RequestPayloadPart$DataType`.
    pub data_type: Option<String>,
    /// `RequestPayloadPart$CompletionState`.
    pub completion_state: Option<String>,
    /// `RequestPayloadPart$P`.
    pub p: Option<String>,
}

impl RequestFrame {
    /// A frame carrying `bytes`, whose `byte_len` is that payload's length.
    pub fn new(
        bytes: Vec<u8>,
        data_type: Option<String>,
        completion_state: Option<String>,
        p: Option<String>,
    ) -> Self {
        Self {
            byte_len: bytes.len(),
            bytes,
            data_type,
            completion_state,
            p,
        }
    }
}

/// One `ResponseStreamEvent$PayloadPart` the emulator emitted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseFrame {
    /// `ResponsePayloadPart$Bytes`. In a TRANSCRIPT entry this may be a prefix —
    /// see [`MAX_RECORDED_INVOCATION_BYTES`]; [`Self::byte_len`] is always the
    /// frame's full length.
    pub bytes: Vec<u8>,
    /// The frame's full `@eventPayload` length, whether or not every byte of it
    /// is retained.
    pub byte_len: usize,
    /// `ResponsePayloadPart$DataType`.
    pub data_type: Option<String>,
    /// `ResponsePayloadPart$CompletionState`.
    pub completion_state: Option<String>,
    /// `ResponsePayloadPart$P`.
    pub p: Option<String>,
}

impl ResponseFrame {
    /// A frame carrying `bytes`, whose `byte_len` is that payload's length. The
    /// constructor a registry entry's scripted frames are built with, so a
    /// scripted frame can never disagree with its own length.
    pub fn new(
        bytes: Vec<u8>,
        data_type: Option<String>,
        completion_state: Option<String>,
        p: Option<String>,
    ) -> Self {
        Self {
            byte_len: bytes.len(),
            bytes,
            data_type,
            completion_state,
            p,
        }
    }
}

/// The request frames a transcript entry retains: the first
/// [`MAX_RECORDED_FRAMES_PER_INVOCATION`] frames in order, each with its event
/// headers (truncated to [`MAX_RECORDED_HEADER_BYTES`]) and its true `byte_len`,
/// and payload bytes truncated so the whole entry keeps at most
/// [`MAX_RECORDED_INVOCATION_BYTES`] of them.
///
/// Frames past the count cap are never visited, so nothing is cloned for them.
pub fn retain_request_frames(frames: &[RequestFrame]) -> Vec<RequestFrame> {
    let mut budget = MAX_RECORDED_INVOCATION_BYTES;
    let mut retained = Vec::with_capacity(frames.len().min(MAX_RECORDED_FRAMES_PER_INVOCATION));
    for frame in frames.iter().take(MAX_RECORDED_FRAMES_PER_INVOCATION) {
        let keep = frame.bytes.len().min(budget);
        budget -= keep;
        retained.push(RequestFrame {
            bytes: frame.bytes[..keep].to_vec(),
            byte_len: frame.byte_len,
            data_type: retain_header(frame.data_type.as_deref()),
            completion_state: retain_header(frame.completion_state.as_deref()),
            p: retain_header(frame.p.as_deref()),
        });
    }
    retained
}

/// [`retain_request_frames`] for the frames the emulator emitted.
pub fn retain_response_frames(frames: &[ResponseFrame]) -> Vec<ResponseFrame> {
    let mut budget = MAX_RECORDED_INVOCATION_BYTES;
    let mut retained = Vec::with_capacity(frames.len().min(MAX_RECORDED_FRAMES_PER_INVOCATION));
    for frame in frames.iter().take(MAX_RECORDED_FRAMES_PER_INVOCATION) {
        let keep = frame.bytes.len().min(budget);
        budget -= keep;
        retained.push(ResponseFrame {
            bytes: frame.bytes[..keep].to_vec(),
            byte_len: frame.byte_len,
            data_type: retain_header(frame.data_type.as_deref()),
            completion_state: retain_header(frame.completion_state.as_deref()),
            p: retain_header(frame.p.as_deref()),
        });
    }
    retained
}

/// One event-header string as a transcript entry retains it: at most
/// [`MAX_RECORDED_HEADER_BYTES`], cut on a UTF-8 character boundary.
fn retain_header(value: Option<&str>) -> Option<String> {
    value.map(|value| {
        if value.len() <= MAX_RECORDED_HEADER_BYTES {
            return value.to_string();
        }
        let mut end = MAX_RECORDED_HEADER_BYTES;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        value[..end].to_string()
    })
}

/// One served invocation, recorded in order.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The access-key id the request was signed with, as the transport read it —
    /// the caller attribution for this invocation. Never a secret: an AKID is the
    /// public half of a credential.
    pub caller_akid: String,
    /// The `EndpointName` the request addressed.
    pub endpoint_name: String,
    /// The `TargetVariant` header, when the request carried one.
    pub target_variant: Option<String>,
    /// The variant actually selected, reported as
    /// `X-Amzn-Invoked-Production-Variant`.
    pub invoked_production_variant: String,
    /// The `ModelInvocationPath` header, when the request carried one.
    pub model_invocation_path: Option<String>,
    /// The `ModelQueryString` header, when the request carried one.
    pub model_query_string: Option<String>,
    /// The request frames retained for this entry, in wire order — at most
    /// [`MAX_RECORDED_FRAMES_PER_INVOCATION`] of them.
    pub request_frames: Vec<RequestFrame>,
    /// How many request frames the invocation actually consumed, whether or not
    /// every one is retained.
    pub request_frames_total: usize,
    /// The `PayloadPart` frames retained for this entry, in wire order — at most
    /// [`MAX_RECORDED_FRAMES_PER_INVOCATION`] of them.
    pub response_frames: Vec<ResponseFrame>,
    /// How many `PayloadPart` frames the invocation actually emitted, whether or
    /// not every one is retained.
    pub response_frames_total: usize,
    /// How the invocation ended.
    pub terminal: InvocationOutcome,
    /// When the invocation was served.
    pub started_at: SystemTime,
}

/// How a recorded invocation ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationOutcome {
    /// The response stream ran to a `COMPLETE` `PayloadPart`.
    Completed,
    /// The response stream was terminated by an in-stream `ModelStreamError`.
    ModelStreamError {
        /// The message that frame carried.
        message: String,
        /// The error code that frame carried.
        error_code: String,
    },
    /// The response stream was terminated by an in-stream
    /// `InternalStreamFailure`.
    InternalStreamFailure {
        /// The message that frame carried.
        message: String,
    },
    /// The invocation was refused with the modelled `ModelError` (HTTP 424), so
    /// no response stream was opened at all. Recorded because the request frames
    /// were still received and are still observable.
    ModelError {
        /// `ModelError$OriginalStatusCode`.
        original_status_code: i32,
        /// `ModelError$ErrorCode`.
        error_code: String,
    },
    /// The answer was computed but the emulator could not frame it, so the
    /// caller received a server error rather than a response stream.
    ///
    /// The entry is recorded BEFORE framing, so this is what the outcome is
    /// corrected to when framing fails — the transcript never claims an
    /// invocation completed that the caller saw fail.
    ResponseFramingFailed,
}

impl ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State {
    /// Append `invocation` to the transcript, evicting the oldest entries so the
    /// transcript never holds more than [`MAX_RECORDED_INVOCATIONS`].
    ///
    /// The ONE way an entry is added, so the cap cannot be bypassed by a caller
    /// pushing directly.
    pub fn record_invocation(&mut self, invocation: Invocation) {
        self.invocations.push(invocation);
        if self.invocations.len() > MAX_RECORDED_INVOCATIONS {
            let excess = self.invocations.len() - MAX_RECORDED_INVOCATIONS;
            self.invocations.drain(..excess);
        }
    }

    /// Put `endpoint` in the registry under its own name, replacing any entry
    /// already there.
    ///
    /// The only way an endpoint enters the registry after construction: the
    /// service models no control-plane operation that could.
    pub fn register_endpoint(&mut self, endpoint: Endpoint) {
        self.endpoints.insert(endpoint.name.clone(), endpoint);
    }

    /// Build an endpoint record from its name, variants and behaviour, taking the
    /// first variant as the default. The one constructor, so every registry entry
    /// has a default variant that is one of its own production variants.
    ///
    /// Returns `None` when `production_variants` is empty: an endpoint with no
    /// variant has nothing to route an invocation to.
    pub fn endpoint(
        name: &str,
        production_variants: &[&str],
        behaviour: ModelBehaviour,
    ) -> Option<Endpoint> {
        let default_variant = (*production_variants.first()?).to_string();
        Some(Endpoint {
            name: name.to_string(),
            production_variants: production_variants.iter().map(|v| (*v).to_string()).collect(),
            default_variant,
            behaviour,
            created_at: SystemTime::now(),
        })
    }
}

impl Default for ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State {
    /// The seeded registry.
    ///
    /// This service's model declares no operation that can create an endpoint, so
    /// an emulator with an empty registry could never serve a single successful
    /// invocation. Two endpoints are therefore seeded, each exercising one of the
    /// two answering rules, and both are replaceable through
    /// [`ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State::register_endpoint`].
    fn default() -> Self {
        let mut state = Self {
            endpoints: BTreeMap::new(),
            invocations: Vec::new(),
        };
        // One variant, so an invocation that names no `TargetVariant` is routed
        // to `AllTraffic` — the variant name SageMaker gives a single-variant
        // endpoint.
        if let Some(endpoint) = Self::endpoint(
            "my-endpoint",
            &["AllTraffic"],
            ModelBehaviour::Echo {
                chunk_size: 8,
                data_type: "UTF8".to_string(),
            },
        ) {
            state.register_endpoint(endpoint);
        }
        // Two variants, so `TargetVariant` selection is reachable without a test
        // first registering an endpoint of its own.
        if let Some(endpoint) = Self::endpoint(
            "my-canary-endpoint",
            &["AllTraffic", "Canary"],
            ModelBehaviour::Scripted {
                frames: vec![
                    ResponseFrame::new(
                        b"canary ".to_vec(),
                        Some("UTF8".to_string()),
                        Some("PARTIAL".to_string()),
                        None,
                    ),
                    ResponseFrame::new(
                        b"answer".to_vec(),
                        Some("UTF8".to_string()),
                        Some("COMPLETE".to_string()),
                        None,
                    ),
                ],
            },
        ) {
            state.register_endpoint(endpoint);
        }
        state
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State>)
}

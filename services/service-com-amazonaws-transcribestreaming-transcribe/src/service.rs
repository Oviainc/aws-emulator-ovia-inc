// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-transcribestreaming-transcribe` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use crate::ops::start_medical_scribe_stream::{self, StartMedicalScribeStream};
use crate::ops::start_medical_stream_transcription::{self, StartMedicalStreamTranscription};
use crate::ops::start_stream_transcription::{self, StartStreamTranscription};
use crate::ops::types::{request_path, EventStreamRoute, WEBSOCKET_PATH_SEGMENTS};
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment must
/// be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment
/// equality. It is the same rule `service_common::claims_request` applies for a REST
/// service; this crate reaches the framework helper directly, as
/// `service-com-amazonaws-appconfig-amazonappconfig`, `-appconfigdata-appconfigdata`,
/// `-ivs-*` and `-ivsrealtime-*` do.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method,
/// so nothing it decides on is missing.
///
/// It is called TWICE, over two path spaces this service owns, because the model
/// describes only one of them. `crate::ROUTE_PATH_SEGMENTS` is the model-derived set:
/// the first segment of each operation's `@http` URI, which is the HTTP/2 transport.
/// The same operations are also reachable over their WebSocket transport, at those
/// URIs plus a `-websocket` suffix, which the model does not describe at all (see
/// `WEBSOCKET_PATH_SEGMENTS`) — so that set is passed through the SAME framework rule
/// rather than matched by a hand-rolled one: a request still has to resolve to this
/// service's signing name, and still has to match a declared segment exactly.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    let claims = |segments: &[&str]| {
        emulator_server::can_handle_based_on_route_key_and_path(
            &request,
            crate::ROUTE_KEY,
            segments,
        )
    };
    claims(crate::ROUTE_PATH_SEGMENTS) || claims(WEBSOCKET_PATH_SEGMENTS)
}

/// The `com-amazonaws-transcribestreaming-transcribe` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
///
/// Besides the generated entry it holds this service's three event-stream
/// operations. They are NOT in the entry's routing table in a servable form: the
/// generated `ErasedOperation` shim of an operation whose output is a `@streaming`
/// union declines the request (it cannot drive `serialize_as_event_stream<P>`
/// through an object-safe `run`), so each one is held here behind an
/// [`EventStreamRoute`] and routed by [`EmulatedService::override_operation`] —
/// which the router consults FIRST, before the generated table.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    start_stream_transcription: EventStreamRoute<StartStreamTranscription<Eng>>,
    start_medical_stream_transcription: EventStreamRoute<StartMedicalStreamTranscription<Eng>>,
    start_medical_scribe_stream: EventStreamRoute<StartMedicalScribeStream<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine.clone()),
            start_stream_transcription: EventStreamRoute(StartStreamTranscription::new(
                engine.clone(),
            )),
            start_medical_stream_transcription: EventStreamRoute(
                StartMedicalStreamTranscription::new(engine.clone()),
            ),
            start_medical_scribe_stream: EventStreamRoute(StartMedicalScribeStream::new(engine)),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `transcribe` — a signing name it SHARES with the Amazon
        // Transcribe control plane
        // (`services/service-com-amazonaws-transcribe-transcribe`, `sdkId`
        // `Transcribe`, endpoint prefix `transcribe`, protocol `awsJson1_1`, 44
        // operations), a DIFFERENT service that AWS separates by endpoint host. The
        // router's routing key comes from the SigV4 credential scope first (then
        // `X-Amz-Target`), so both services resolve to `transcribe` and matching on the
        // signing name ALONE makes both claim every such request — which the router
        // answers `500 AmbiguousService`, leaving BOTH services unreachable.
        //
        // So the claim adds the framework's path conjunct over this service's own
        // model-derived path space — see [`can_handle_parts`]. This model is
        // `restJson1`: each of its five operations is bound to its own URI, and those
        // literal first segments are what the generated `ROUTE_PATH_SEGMENTS` carries.
        // The control plane's path space (the root path its `POST /` traffic arrives on,
        // plus the literal segments of its inert `@http` bindings) is disjoint from this
        // one, so each service answers only its own traffic. The WebSocket data-plane
        // segments this service also claims are disjoint from the peer's for the same
        // reason — no `transcribe` control-plane URI begins with a `-websocket` segment.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The three event-stream operations the generated routing table cannot
        // serve. Routed by the method and path each transport of the operation
        // arrives on, so a request named here reaches the operation that owns it and
        // nothing else is touched: a request the model routes itself still resolves
        // through the generated table (`GET /medical-scribe-stream/{SessionId}`), and
        // `StartCallAnalyticsStreamTranscription` — which this slice does not
        // implement — keeps answering the framework's "modeled but not served"
        // response rather than being half-served here.
        //
        // POST on the model's own URI is the HTTP/2 transport. GET on that URI plus
        // `-websocket` is the WebSocket transport of the SAME operation: a protocol
        // upgrade arrives as GET, so the method this seam accepts depends on the
        // transport, and routing it here is what keeps a client of the WebSocket data
        // plane from being answered the router's "no such service" 404 — the
        // operation answers it with its own modeled errors instead. Every arm names
        // its path through the owning operation's own const, so this table cannot
        // drift from the paths those operations recognise.
        match (req.method(), request_path(req.uri())) {
            ("POST", start_stream_transcription::MODELED_PATH) => {
                Some(&self.start_stream_transcription)
            }
            ("POST", start_medical_stream_transcription::MODELED_PATH) => {
                Some(&self.start_medical_stream_transcription)
            }
            ("POST", start_medical_scribe_stream::MODELED_PATH) => {
                Some(&self.start_medical_scribe_stream)
            }
            ("GET", start_stream_transcription::WEBSOCKET_PATH) => {
                Some(&self.start_stream_transcription)
            }
            ("GET", start_medical_stream_transcription::WEBSOCKET_PATH) => {
                Some(&self.start_medical_stream_transcription)
            }
            _ => None,
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

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

use crate::ops::start_medical_scribe_stream::StartMedicalScribeStream;
use crate::ops::start_medical_stream_transcription::StartMedicalStreamTranscription;
use crate::ops::start_stream_transcription::StartStreamTranscription;
use crate::ops::types::EventStreamRoute;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// The path of a request URI, without its query string.
fn request_path(uri: &str) -> &str {
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The three event-stream operations the generated routing table cannot
        // serve. Routed by the method and path the model gives each one, so a
        // request the model does not name here still resolves through the generated
        // table (`GET /medical-scribe-stream/{SessionId}`), and
        // `StartCallAnalyticsStreamTranscription` — which this slice does not
        // implement — keeps answering the framework's "modeled but not served"
        // response rather than being half-served here.
        if req.method() != http::Method::POST.as_str() {
            return None;
        }
        match request_path(req.uri()) {
            "/stream-transcription" => Some(&self.start_stream_transcription),
            "/medical-stream-transcription" => Some(&self.start_medical_stream_transcription),
            "/medical-scribe-stream" => Some(&self.start_medical_scribe_stream),
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

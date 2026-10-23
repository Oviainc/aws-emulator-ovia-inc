// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockruntime-amazonbedrockfrontendservice` service: how it claims requests and how it words the
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

use crate::ops::converse_stream::ConverseStream;
use crate::ops::invoke_model_with_response_stream::InvokeModelWithResponseStream;
use service_common::event_stream::EventStreamRoute;
// `request_path`: the one reader of a request URI's path, shared rather than inlined
// again here.
use service_common::request_path;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Whether `path` is `/model/{modelId}/{tail}` — the URI space every inference
/// operation of this service binds — and, if so, its `{tail}`.
///
/// `{modelId}` is a single `@httpLabel` segment, so the tail is the third segment
/// and there is nothing after it.
fn inference_route(path: &str) -> Option<&str> {
    let mut segments = path.trim_start_matches('/').split('/');
    let ("model", Some(model_id), Some(tail), None) = (
        segments.next()?,
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    (!model_id.is_empty() && !tail.is_empty()).then_some(tail)
}

/// The `com-amazonaws-bedrockruntime-amazonbedrockfrontendservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
///
/// Besides the generated entry it holds this service's two event-stream
/// operations. They are NOT in the entry's routing table in a servable form: the
/// generated `ErasedOperation` shim of an operation whose output is a `@streaming`
/// union declines the request (it cannot drive `serialize_as_event_stream<P>`
/// through an object-safe `run`), so each one is held here behind an
/// [`EventStreamRoute`] and routed by [`EmulatedService::override_operation`] —
/// which the router consults FIRST, before the generated table.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    converse_stream: EventStreamRoute<ConverseStream<Eng>>,
    invoke_model_with_response_stream:
        EventStreamRoute<InvokeModelWithResponseStream<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            converse_stream: EventStreamRoute(ConverseStream::new(Arc::clone(&engine))),
            invoke_model_with_response_stream: EventStreamRoute(
                InvokeModelWithResponseStream::new(engine),
            ),
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
        // This service signs as `bedrock` — the signing name the WHOLE Bedrock family
        // shares (the control plane, this data plane, `bedrock-agent`,
        // `bedrock-agent-runtime`, `bedrock-data-automation` and
        // `bedrock-data-automation-runtime`), while AWS separates them by endpoint host
        // (`bedrock-runtime` here). The router keys on the SigV4 credential scope first,
        // so all six resolve to `bedrock`: claiming on that name alone makes every one of
        // them claim every Bedrock request and the router answers `500 AmbiguousService`
        // — an error code no AWS model declares — instead of serving any of them.
        //
        // The claim is therefore this service's own modelled path space, through the one
        // rule `service_common::bedrock` states for the family (which narrows
        // `service_common::claims_request` with the family's shared-segment split):
        // `async-invoke`, `guardrail`, `guardrail-checks` and `model` — the segments this
        // `restJson1` model binds, none of which another member publishes, matched by
        // EXACT segment equality, never by prefix, which is what keeps `model` out of the
        // control plane's `model-copy-jobs`, `model-import-jobs` and the rest. `None` for
        // the RPC argument: every operation here is bound to a URI.
        service_common::bedrock::claims_request(
            parts,
            self.definition(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The two event-stream operations the generated routing table cannot
        // serve. Routed by the method and the URI the model gives each one, so
        // every other request — including `InvokeModelWithBidirectionalStream`,
        // which this slice does not implement — still resolves through the
        // generated table and keeps answering the framework's "modeled but not
        // served" response rather than being half-served here.
        if req.method() != http::Method::POST.as_str() {
            return None;
        }
        match inference_route(request_path(req.uri()))? {
            "converse-stream" => Some(&self.converse_stream),
            "invoke-with-response-stream" => Some(&self.invoke_model_with_response_stream),
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

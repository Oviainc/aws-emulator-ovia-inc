// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sagemakerruntime-amazonsagemakerruntime` service: how it claims requests and how it words the
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
//!
//! # How this service states a refused invocation
//!
//! Both halves of that answer — the error SHAPE a failure names in `x-amzn-errortype`,
//! and the envelope a request refused before any handler ran is stated in — are the same
//! for every route this service serves, so they live in one place,
//! [`error_envelope`]. Every modelled operation is served wrapped in that module's
//! [`error_envelope::InModelledEnvelope`], and the hand-authored streaming route states
//! its own errors through the same module, so the three routes cannot drift apart in how
//! they word a refusal.

pub mod error_envelope;

use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-sagemakerruntime-amazonsagemakerruntime` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`error_envelope::InModelledEnvelope`] so a request refused before its handler runs
    /// is refused in this service's own modelled `ValidationError`. Keyed by the routing
    /// key the protocol resolves — the key the generated table uses — and built from the
    /// SAME shared engine, so a wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
    /// The hand-authored `InvokeEndpointWithResponseStream` implementation, wrapped in the
    /// shared `EventStreamRoute` adapter: the generated routing table holds a declining
    /// entry for that operation (the scaffold generator does not serve a `@streaming`
    /// event-stream output), so this one is served through `override_operation`, which the
    /// router consults first.
    response_stream: service_common::event_stream::EventStreamRoute<
        crate::ops::invoke_endpoint_with_response_stream::ResponseStreamInvocation<Eng>,
    >,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // The two operations the generated table serves, each keyed by its operation shape
        // name — the routing key `ServerHttpProtocol::routing_key` resolves for a restJson1
        // request, which is what the generated table is keyed by. The third route
        // (`InvokeEndpointWithResponseStream`) is hand-authored below.
        let mut in_modelled_envelope = OperationRoutingTable::new();
        in_modelled_envelope.add_operation(
            "InvokeEndpoint",
            Box::new(error_envelope::InModelledEnvelope::new(
                crate::ops::invoke_endpoint::InvokeEndpoint::new(Arc::clone(&engine)),
            )) as Box<dyn ErasedOperation>,
        );
        in_modelled_envelope.add_operation(
            "InvokeEndpointAsync",
            Box::new(error_envelope::InModelledEnvelope::new(
                crate::ops::invoke_endpoint_async::InvokeEndpointAsync::new(Arc::clone(&engine)),
            )) as Box<dyn ErasedOperation>,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine.clone()),
            in_modelled_envelope,
            response_stream: service_common::event_stream::EventStreamRoute(
                crate::ops::invoke_endpoint_with_response_stream::ResponseStreamInvocation::new(
                    engine,
                ),
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
        // This service signs as `sagemaker` — a signing name it SHARES with every other
        // SageMaker-family service (the control plane, the other endpoint runtime, A2I,
        // Edge, FeatureStore, Metrics, the training-session runtime and the AgenticRFT
        // job runtime), while serving its own endpoint (`runtime.sagemaker`). The router
        // resolves the routing key from the SigV4 credential scope first, so every one of
        // them sees the key `sagemaker`: claiming on the signing name ALONE claims the
        // siblings' traffic too, and the router — which refuses rather than guesses —
        // answers `500 AmbiguousService` for ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind (the `/endpoints/…` space), which is what `claims_request`
        // matches against the generated `ROUTE_PATH_SEGMENTS` when a service declares no
        // RPC protocol.
        //
        // That path space is the ONE in the family that is not disjoint: the HTTP2
        // endpoint runtime signs as `sagemaker` too, publishes the SAME
        // `runtime.sagemaker` endpointPrefix, and binds its one operation under the
        // same `/endpoints/{EndpointName}/` prefix, so the first segment leaves the two
        // of them claiming each other's traffic. The segment their modelled URIs differ
        // at is the third, and `service_common::addresses_runtime_bidirectional_stream`
        // states that split once for both sides: this service claims the `/endpoints/…`
        // space EXCEPT the one bidirectional-stream invocation that sibling binds.
        // Stated as a complement rather than as "my own three operations" on purpose —
        // a request under `/endpoints/…` that matches no modelled invocation at all is
        // then still claimed by exactly one service, this one, which answers it in its
        // own protocol envelope, instead of by none, which the router would answer
        // `404 UnknownService`.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        ) && !service_common::addresses_runtime_bidirectional_stream(parts.uri.path())
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // `InvokeEndpointWithResponseStream` is served from here because the generated
        // routing table's entry for it declines: the scaffold generator does not emit the
        // event-stream framing a `@streaming` output needs. Every other operation
        // resolves through the generated table.
        use crate::ops::invoke_endpoint_with_response_stream::ResponseStreamInvocation;
        if ResponseStreamInvocation::<Eng>::claims(req) {
            return Some(&self.response_stream);
        }
        // NOT a re-route for the other two: the key is the one the generated table's own
        // protocol resolves, and the operation it names is the one served — wrapped in
        // `error_envelope::InModelledEnvelope` so a request refused before the handler runs
        // is refused in this service's modelled `ValidationError` rather than in the
        // framework's pre-dispatch envelope, which names a shape this model does not
        // declare and carries no `Message`. A key the model does not name resolves to
        // `None` here exactly as it does in the generated table, so the router's
        // routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` refuses a constraint violation itself, in the modelled
        // envelope, so this seam words any refusal the FRAMEWORK still renders — a refusal
        // raised before an operation is named at all, and the fallback in that module.
        // Wording it from the same violations keeps the two paths from disagreeing: a value
        // outside a declared bound is this service's `ValidationError` either way.
        error_envelope::stated_refusal(refusal)
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

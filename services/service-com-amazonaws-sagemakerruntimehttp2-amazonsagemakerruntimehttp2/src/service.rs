// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2` service: how it claims requests and how it words the
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The hand-written event-stream operation this service serves, held here
    /// because [`Service::override_operation`] hands back a borrow of `&self`.
    ///
    /// Built once in [`EmulatedService::new`] from the same shared engine the
    /// generated entry gets, so it reaches the same store.
    bidirectional_stream:
        crate::ops::invoke_endpoint_with_bidirectional_stream::BidirectionalStreamEntry<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            bidirectional_stream:
                crate::ops::invoke_endpoint_with_bidirectional_stream::BidirectionalStreamEntry::new(
                    Arc::clone(&engine),
                ),
            entry: crate::ServiceDefinition::new(engine),
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
        // SageMaker-family service, while serving its own endpoint
        // (`runtime.sagemaker`, on the HTTP/2 port). The router resolves the routing key
        // from the SigV4 credential scope first, so every one of them sees the key
        // `sagemaker`: claiming on the signing name ALONE claims the siblings' traffic
        // too, and the router — which refuses rather than guesses — answers
        // `500 AmbiguousService` for ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its one
        // operation binds (the `/endpoints/…` space), which is what `claims_request`
        // matches against the generated `ROUTE_PATH_SEGMENTS` when a service declares no
        // RPC protocol.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // This service's ONE modeled operation,
        // `InvokeEndpointWithBidirectionalStream`, binds a `@streaming` union to
        // both its input `@httpPayload` and its output `@httpPayload`. The
        // generated surface declines such an operation — its routing-table entry
        // answers the framework's "modeled but not served" signal — so the
        // hand-written entry in `ops/invoke_endpoint_with_bidirectional_stream.rs`
        // is returned here instead, and that entry does the event-stream framing
        // the generator does not emit. The generated table entry is still what the
        // router reads the model's AUTH SCHEMES off, so this operation stays
        // SigV4-signed exactly as the model declares.
        //
        // The match is the operation's own route (`POST
        // /endpoints/{EndpointName}/invocations-bidirectional-stream`), applied by
        // the same function the handler uses to read the `@httpLabel`, so routing
        // and handling cannot disagree about which requests this claims. That
        // function checks the METHOD as well as the path shape, so a non-POST
        // request to the same path is not claimed here and falls through to the
        // generated routing table. Any other request does too.
        let claimed =
            crate::ops::invoke_endpoint_with_bidirectional_stream::endpoint_name_label(req)
                .map(|_| &self.bidirectional_stream as &dyn ErasedOperation);
        if claimed.is_some() {
            // The accepted path leaves a trace too, not only the refusal paths
            // below. Value-free: the resolved operation, never the label.
            tracing::debug!(
                service = self.entry.identity().signing_name(),
                operation = "InvokeEndpointWithBidirectionalStream",
                "claimed a request for the hand-written event-stream operation"
            );
        }
        claimed
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
        //
        // The framework's wording is kept, but the refusal is RECORDED: a request
        // refused before any handler ran otherwise leaves no trace attributable
        // to the caller. Debug level, and only the refusal's own code — never a
        // request value.
        tracing::debug!(
            service = self.entry.identity().signing_name(),
            refusal = match refusal {
                Refusal::Invalid(violations) => {
                    // The member paths that failed, never the values they held.
                    let paths: Vec<&str> =
                        violations.iter().map(|v| v.path.as_str()).collect();
                    format!("invalid: {}", paths.join(", "))
                }
                Refusal::Unauthenticated => "unauthenticated".to_string(),
                // `Refusal` is `#[non_exhaustive]`, so a variant added later lands
                // here. Labelled neutrally rather than as one of the known kinds,
                // which would mis-attribute it.
                _ => "refused".to_string(),
            },
            "refused a request before any handler ran"
        );
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
        //
        // The default rendering is kept, and the unroutable request is recorded so
        // it is not silent. No body or header value is logged.
        tracing::debug!(
            service = self.entry.identity().signing_name(),
            method = req.method(),
            "a request reached this service but named no modeled operation"
        );
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
        //
        // The entry's rendering is kept, and the transport-level failure is
        // recorded so it is not silent. A `ProtocolError` for a transport failure
        // can embed fragments of the request, so its rendering goes through the
        // one log sanitiser this crate has.
        let rendered =
            crate::ops::invoke_endpoint_with_bidirectional_stream::printable(&error.to_string());
        tracing::debug!(
            service = self.entry.identity().signing_name(),
            error = %rendered,
            "a transport-level failure was detected before an operation was named"
        );
        None
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-eventbridgev2-awseventsv2` service: how it claims requests and how it words the
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

/// This service's own `awsService.endpointPrefix`, the second key a request may
/// resolve to.
///
/// The generated `ROUTE_KEY` is the model's `aws.auth#sigv4` name (`events`, shared
/// with the EventBridge endpoint), but a request that carries no SigV4 scope resolves
/// its key from the endpoint HOST instead, and this endpoint's host is
/// `eventsv2.<region>.<dnsSuffix>` — `smithy_query(query="service_info")`:
/// `awsService.endpointPrefix` `eventsv2`. No other emulated service claims that key,
/// so honouring it adds no ambiguity and lets an anonymous or presigned request that
/// addresses this endpoint by host reach it.
const OWN_ENDPOINT_PREFIX: &str = "eventsv2";

/// Whether the request these `parts` came from matches this service's wire identity
/// (its SigV4 signing name or its endpoint prefix) AND the modelled URI path space its
/// generated `ROUTE_PATH_SEGMENTS` declares.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of that rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match the key passed AND its first URI path segment must be one the
/// generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment
/// equality. That path conjunct is what separates two services sharing one SigV4
/// signing name — this V2 endpoint (`eventsv2`) and the EventBridge endpoint (`events`)
/// both sign as `events`
/// (`smithy_query(query="service_info")`: `sigv4Name` `events` on both,
/// `awsService.endpointPrefix` `eventsv2` here and `events` there), so the key alone
/// matches both and the router rejects every such request as `500 AmbiguousService`.
///
/// The path spaces are disjoint by protocol: this model is `rpcv2Cbor`, whose every
/// request is `POST /service/AWSEventsV2/operation/<Operation>`, so its generated
/// segment list is the `service` segment that form always carries; the EventBridge
/// model is `awsJson1_1`, which posts every request to `/`.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn claims_route_key_and_path(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    [crate::ROUTE_KEY, OWN_ENDPOINT_PREFIX].iter().any(|key| {
        emulator_server::can_handle_based_on_route_key_and_path(
            &request,
            key,
            crate::ROUTE_PATH_SEGMENTS,
        )
    })
}

/// The `com-amazonaws-eventbridgev2-awseventsv2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
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
        // Delegated to the framework's own predicate — see
        // [`claims_route_key_and_path`]: this service's signing name (`events`) is
        // shared with the EventBridge endpoint, so the claim is the route key AND the
        // model-derived first URI path segment (`service`, the fixed first segment of
        // every `rpcv2Cbor` request). Matching on the signing name alone would have
        // both services claim every `events`-signed request, which the router answers
        // with `500 AmbiguousService` instead of serving either one.
        claims_route_key_and_path(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
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

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-apigatewayv2-apigatewayv2` service: how it claims requests and how it words the
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
//! ## This service's other edited seam: the `execute-api` WebSocket data plane
//!
//! Beside its modelled `/v2/…` control plane, this service is addressed on the invoke
//! endpoint of every WebSocket api it mints — `wss://{api-id}.execute-api.{region}.
//! amazonaws.com/{stage}`, the URI `Api$ApiEndpoint` reports — where a client's FIRST
//! request is the RFC 6455 opening handshake and not a modelled operation. [`websocket`]
//! owns that half: which handshakes this service claims ([`websocket::claims_parts`], read
//! by [`Service::can_handle`]) and what each is answered with
//! ([`websocket::serve`], reached from [`Service::override_routing_error`]).

mod websocket;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of this rule, and it is CALLED here rather than re-implemented: the request's route key
/// must match this service's signing name AND its first URI path segment must be one the
/// generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment equality. It
/// is the same rule `service_common::claims_request` applies for a REST service; this
/// crate reaches the framework helper directly, as
/// `service-com-amazonaws-transcribe-transcribe`, `-appconfig-amazonappconfig` and
/// `-ivs-*` do.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-apigatewayv2-apigatewayv2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The one shared engine, held here because the WebSocket data plane ([`websocket`])
    /// is decided and served from seams the generated entry does not reach: a claim and a
    /// routing-error answer, both of which read this service's own records.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
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
        // This service signs as `apigateway` — a signing name it SHARES with the Amazon
        // API Gateway control plane
        // (`services/service-com-amazonaws-apigateway-backplanecontrolservice`, `sdkId`
        // `API Gateway`, the same `apigateway` endpoint prefix and signing name), a
        // DIFFERENT service that AWS separates by URI path space: every operation of THIS
        // model is bound under `/v2/…` (`/v2/apis`, `/v2/domainnames`, `/v2/vpclinks`,
        // `/v2/tags`, …), while the control plane's are bound under `/restapis`,
        // `/apikeys`, `/account`, … The router resolves a request's routing key from the
        // SigV4 credential scope, so both services resolve to `apigateway`; claiming on
        // the signing name ALONE makes both claim every such request, which the router
        // answers `500 AmbiguousService` — a code no AWS model carries — leaving BOTH
        // services unreachable.
        //
        // So the claim adds the framework's path conjunct over this service's own
        // model-derived path space — see [`can_handle_parts`], which for this model is the
        // single segment `v2`. None of the control plane's segments is `v2`, so it alone
        // claims its traffic and each service answers only its own.
        if can_handle_parts(parts) {
            return true;
        }
        // And the WEBSOCKET DATA plane: an RFC 6455 opening handshake addressed to a stage
        // of a WebSocket api THIS service holds. Such a request carries no credential
        // scope and its path is no `/v2/…` route, so the front door could resolve no
        // target service for it and answered `404 UnknownService` — for the endpoint this
        // service itself reported in `ApiEndpoint`. The claim is narrowed to an api this
        // service HOLDS, read from the one shared store, so exactly one service claims the
        // request; see [`websocket`] for the whole rule and for what each handshake is
        // answered with.
        websocket::claims_parts(&*self.engine, parts)
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
        //
        // A WebSocket opening handshake IS such a request: it names no modelled operation
        // (its path is a stage's, not a `/v2/…` route's), so this pre-authentication seam
        // is where this service's WebSocket data plane answers it. It is served here
        // rather than named as an operation because a handshake carries no credential
        // whose verification could change the answer — the refusals below are computed
        // from the api's and the stage's stored state alone.
        if websocket::claims_request(&*self.engine, req) {
            return Some(websocket::serve(&*self.engine, req));
        }
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

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-securitylake-securitylake` service: how it claims requests and how it words the
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
//! ## Request admission
//!
//! Security Lake decides whether the PRINCIPAL that signed a request may invoke the
//! operation before that operation runs, so the decision is independent of what the
//! request targets and of whether that target exists. A request from a principal this
//! service does not admit is refused here, ahead of dispatch, with the front door's own
//! HTTP 401 `{"message": …}` body — never with one of this service's modelled resource
//! errors, which would tell a caller that may not ask what this account holds.
//!
//! The decision itself lives in state, beside the resources, as
//! [`crate::state::ComAmazonawsSecuritylakeSecuritylakeState::admission`] keyed by the
//! request's signing principal ([`crate::state::caller_principal`]); this file only
//! reaches it ahead of dispatch, through [`Service::override_operation`] — the one seam
//! that sees a whole request before the modelled walk, which is what a pre-dispatch
//! refusal needs (the walk validates and dispatches before any handler could answer).
//!
//! The refusal shape is NOT in this service's model (which declares
//! `AccessDeniedException` 403, `BadRequestException` 400, `ConflictException` 409,
//! `InternalServerException` 500, `ResourceNotFoundException` 404 and
//! `ThrottlingException` 429, and no 401 at all), so it is rendered as the refusal the
//! front door states rather than through the modelled error channel: HTTP 401, a JSON
//! `message` body, and no `x-amzn-errortype` — there is no modelled error code to name.

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use crate::state::Admission;

/// The HTTP status the front door refuses an unauthorized request with.
///
/// RECORDED behaviour, not modelled: this service's model declares no 401 shape (see the
/// module docs), and the recording of a refused Security Lake call carries this status.
const UNAUTHORIZED_STATUS: u16 = 401;

/// The front door's own text for an authorization refusal: a CONSTANT, carrying nothing
/// from the request — the refusal is reached from the principal alone, so there is no
/// per-request detail in it to derive, and nothing about this account's state may leak
/// into it.
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The refusal of a request whose principal this service does not admit, as a
/// pre-dispatch operation.
///
/// Built here rather than through the protocol's modelled-error renderer because the
/// shape is not in the model: that renderer would name an error code in
/// `x-amzn-errortype`, and the front door states none.
struct UnauthorizedAnswer;

impl ErasedOperation for UnauthorizedAnswer {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let body = ::serde_json::json!({ "message": UNAUTHORIZED_MESSAGE }).to_string();
            let status = StatusCode::try_from(UNAUTHORIZED_STATUS)
                .expect("401 is a valid HTTP status code");
            let mut response = RtResponse::new(status, SdkBody::from(body));
            // `restJson1`'s own media type, so the body a client parses is the one it
            // parses for every other answer from this service.
            response
                .headers_mut()
                .insert("content-type", "application/json");
            response
        })
    }
}

/// The `com-amazonaws-securitylake-securitylake` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The shared engine, so request admission can read the authorization records this
    /// service's store holds BEFORE dispatch. The generated entry owns the same engine;
    /// this is the one `Arc` both share, never a second store.
    engine: Arc<Eng>,
    /// Answers a request whose principal this service does not admit.
    unauthorized: UnauthorizedAnswer,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            unauthorized: UnauthorizedAnswer,
        }
    }

    /// The authorization decision for one request, reached from its signing principal
    /// alone and before anything is looked up.
    ///
    /// A request that names no modelled operation is left alone — there is nothing to
    /// authorize, and the routing failure is the answer it gets. A store this process
    /// cannot reach cannot hold a refusal either, so such a request is admitted and the
    /// handler reports the unreachable store as the modelled server error.
    ///
    /// This is a READ: it takes the store's lock to look one principal up and writes
    /// nothing, so a refused request changes nothing a later caller sees.
    fn admission(&self, req: &aws_smithy_runtime_api::http::Request) -> Admission {
        if self.definition().protocol().routing_key(req).is_err() {
            return Admission::Admitted;
        }
        let principal =
            crate::state::caller_principal(req.headers().get(http::header::AUTHORIZATION.as_str()));
        match crate::state::service_state(&*self.engine) {
            Some(state) => state.lock().admission(&principal),
            None => Admission::Admitted,
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // Security Lake authorizes the signing principal BEFORE the operation runs (see
        // the module docs), and the modelled walk deserializes, validates and dispatches
        // before any handler could answer, so this pre-dispatch seam is the only place
        // that refusal is reachable from.
        //
        // The request's auth schemes are unaffected: the router reads them off the
        // MODELLED operation the routing key names, so a refusal here is still only
        // reached by a request whose signature verified.
        match self.admission(req) {
            Admission::Admitted => None,
            Admission::Unauthorized => Some(&self.unauthorized as &dyn ErasedOperation),
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

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
//! ## The account-entitlement gate
//!
//! Security Lake's data lake is administered by the organization's delegated Security
//! Lake administrator or by a standalone Security Lake account
//! ([`crate::state::AccountRole`]). A caller account holding neither role may not
//! configure or inspect a data lake, nor reach a subscriber of one, and that verdict is
//! reached from the caller's role alone: it precedes the request-level validation the
//! modelled walk performs, so a gated call is refused identically whatever it carries,
//! and nothing it would have written is applied. That ordering is only reachable from
//! this pre-dispatch seam, which is why the gate lives here rather than in each handler.
//!
//! Each gated route reports that refusal the way Security Lake does for that route
//! ([`ENTITLEMENT_GATED_OPERATIONS`]): the routes the service itself denies answer the
//! modelled `AccessDeniedException` (HTTP 403) carrying the fixed entitlement text, and
//! those whose authorizer refuses ahead of the service answer HTTP 401
//! `UnauthorizedException` / "Unauthorized". A reporting call that only tells the caller
//! about its OWN account — which of its Regions are enabled (`ListDataLakes`) or what
//! those Regions have recorded (`ListDataLakeExceptions`) — is not gated, and neither are
//! the delegated-administrator registration calls, which the organization MANAGEMENT
//! account makes rather than the account being administered.
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
//! `message` body, and the RECORDED error code `UnauthorizedException` in
//! `x-amzn-errortype` — which is where `restJson1` resolves an error from, so a client
//! branching on that code reads the same verdict however the caller came to be refused.
//! One refused caller therefore gets ONE answer on every route, gated or not.

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use crate::ops::types::{AccessDeniedException, SCHEMA_ACCESS_DENIED_EXCEPTION};
use crate::state::{AccountRole, Admission};

/// How one entitlement-gated operation reports a caller account that holds no Security
/// Lake role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// The service denies the call itself: this service's modelled
    /// `AccessDeniedException`, which the model gives `@httpError 403`, carrying the
    /// fixed entitlement text.
    AccessDenied,
    /// The route's authorizer refuses the call ahead of the service: the front door's own
    /// refusal (HTTP 401, `UnauthorizedException`, "Unauthorized"). RECORDED per route —
    /// this service's model declares no 401 shape at all, so it is rendered as the front
    /// door's own refusal rather than through the modelled error channel.
    Unauthorized,
}

/// The operations an unentitled caller account may not invoke, and how each one reports
/// the refusal.
///
/// The gated set is the data lake AND the resources it administers: the operations that
/// CONFIGURE the account's data lake, its exception subscription and its organization
/// configuration, report what those configurations hold, or reach one named subscriber of
/// that data lake. The reporting calls that only tell the caller about its OWN account —
/// which of its Regions are enabled (`ListDataLakes`) and what those Regions have
/// recorded (`ListDataLakeExceptions`) — are absent on purpose: any account may ask about
/// itself, and Security Lake answers them from that account's own state, so their
/// precondition is enablement rather than a role verdict. So are
/// `RegisterDataLakeDelegatedAdministrator` and
/// `DeregisterDataLakeDelegatedAdministrator`, which the organization management account
/// calls to GIVE an account the role this gate tests for.
///
/// The refusal is a property of the ROUTE rather than one verdict for the whole service,
/// so each entry carries the refusal Security Lake reports on ITS route: a route whose
/// authorizer refuses ahead of the service answers HTTP 401 even where the operation
/// beside it answers the modelled 403. A route enters this table only where Security
/// Lake's refusal of an account holding no role is recorded for it, which is why the
/// subscriber surface is here by the single-subscriber routes while the collection read
/// beside them (`ListSubscribers`) is not.
const ENTITLEMENT_GATED_OPERATIONS: &[(&str, Gate)] = &[
    ("CreateDataLake", Gate::AccessDenied),
    ("CreateDataLakeExceptionSubscription", Gate::AccessDenied),
    ("CreateDataLakeOrganizationConfiguration", Gate::AccessDenied),
    ("DeleteDataLake", Gate::Unauthorized),
    ("DeleteDataLakeExceptionSubscription", Gate::AccessDenied),
    ("DeleteDataLakeOrganizationConfiguration", Gate::AccessDenied),
    ("DeleteSubscriber", Gate::Unauthorized),
    ("GetDataLakeExceptionSubscription", Gate::AccessDenied),
    ("GetDataLakeOrganizationConfiguration", Gate::AccessDenied),
    ("GetDataLakeSources", Gate::Unauthorized),
    ("GetSubscriber", Gate::Unauthorized),
    ("UpdateDataLake", Gate::Unauthorized),
    ("UpdateDataLakeExceptionSubscription", Gate::AccessDenied),
];

/// How `operation` reports an unentitled caller account, or `None` when the operation is
/// not gated on the caller's Security Lake role.
fn entitlement_gate(operation: &str) -> Option<Gate> {
    ENTITLEMENT_GATED_OPERATIONS
        .iter()
        .find(|(gated, _)| *gated == operation)
        .map(|(_, gate)| *gate)
}

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

/// The error code a client reads an authorization refusal's verdict from, in
/// `x-amzn-errortype` (which is where `restJson1` resolves an error shape from).
/// RECORDED, not modelled: this service's model declares no 401 shape, so the code cannot
/// come from it.
///
/// It is the same code whichever way the caller came to be refused — a principal this
/// service does not admit, or an unentitled account on a route whose authorizer refuses
/// ahead of the service — because a client branches on the code to tell "you may not call
/// this service" from this service's own modelled resource verdicts.
const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// A refusal the front door states itself, as a pre-dispatch operation: one whose shape
/// is NOT in this service's model, so it cannot be rendered through the modelled error
/// channel.
///
/// `error_type` is the code a client branches on, carried the way `restJson1` carries an
/// error code; `message` is fixed text, so nothing from the request or from this
/// account's state reaches a caller that may not ask for it.
struct GatewayRefusal {
    status: u16,
    error_type: &'static str,
    message: &'static str,
}

impl ErasedOperation for GatewayRefusal {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let body = ::serde_json::json!({ "message": self.message }).to_string();
            let status =
                StatusCode::try_from(self.status).expect("the refusal statuses are constants");
            let mut response = RtResponse::new(status, SdkBody::from(body));
            // `restJson1`'s own media type, so the body a client parses is the one it
            // parses for every other answer from this service.
            response
                .headers_mut()
                .insert("content-type", "application/json");
            response
                .headers_mut()
                .insert("x-amzn-errortype", self.error_type);
            response
        })
    }
}

/// The refusal of a gated request from a caller account that holds no Security Lake
/// role, as a pre-dispatch operation: this service's own modelled
/// `AccessDeniedException`.
///
/// Rendered through the SERVICE's protocol rather than by hand, so a pre-dispatch denial
/// and the same modelled error returned by a handler answer in one envelope.
struct NotEntitledAnswer;

impl ErasedOperation for NotEntitledAnswer {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let denied = AccessDeniedException {
                message: Some(crate::state::NOT_ENTITLED_MESSAGE.to_string()),
                // The model's other member is for a service-supplied sub-code; the
                // recorded refusal carries none.
                error_code: None,
            };
            let definition = svc.definition();
            definition
                .protocol()
                .serialize_error(
                    &denied,
                    &SCHEMA_ACCESS_DENIED_EXCEPTION,
                    ACCESS_DENIED_CODE,
                    ACCESS_DENIED_STATUS,
                )
                .unwrap_or_else(|error| definition.framework_error_response(error, req))
        })
    }
}

/// The modelled `AccessDeniedException`'s shape name, which `restJson1` carries in
/// `x-amzn-errortype` and in the body's `__type`.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The status the model gives `AccessDeniedException` (`@httpError 403`).
const ACCESS_DENIED_STATUS: u16 = 403;

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
    /// Answers a request this service's authorization refuses outright: one whose
    /// principal it does not admit, and a gated request from an unentitled caller account
    /// on a route whose authorizer refuses it ahead of the service
    /// ([`Gate::Unauthorized`]). One refusal, so a caller reads one verdict on every
    /// route.
    unauthorized: GatewayRefusal,
    /// Answers a gated request from an unentitled caller account on a route the service
    /// itself denies ([`Gate::AccessDenied`]).
    not_entitled: NotEntitledAnswer,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            unauthorized: GatewayRefusal {
                status: UNAUTHORIZED_STATUS,
                error_type: UNAUTHORIZED_CODE,
                message: UNAUTHORIZED_MESSAGE,
            },
            not_entitled: NotEntitledAnswer,
        }
    }

    /// The authorization decision for one request, reached from its signing principal
    /// alone and before anything is looked up.
    ///
    /// A store this process cannot reach cannot hold a refusal either, so such a request
    /// is admitted and the handler reports the unreachable store as the modelled server
    /// error.
    ///
    /// This is a READ: it takes the store's lock to look one principal up and writes
    /// nothing, so a refused request changes nothing a later caller sees.
    fn admission(&self, req: &aws_smithy_runtime_api::http::Request) -> Admission {
        let principal =
            crate::state::caller_principal(req.headers().get(http::header::AUTHORIZATION.as_str()));
        match crate::state::service_state(&*self.engine) {
            Some(state) => state.lock().admission(&principal),
            None => Admission::Admitted,
        }
    }

    /// The Security Lake role the caller account holds, as the entitlement gate reads it
    /// before dispatch. A store this process cannot reach holds no topology either, so
    /// such a request is treated as the administering account's and the handler reports
    /// the unreachable store as the modelled server error.
    ///
    /// This is a READ: it takes the store's lock to look one record up and writes
    /// nothing, so a refused request changes nothing a later caller sees.
    fn account_role(&self) -> AccountRole {
        match crate::state::service_state(&*self.engine) {
            Some(state) => state.lock().account_role(),
            None => AccountRole::Standalone,
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
        //
        // A request that names no modelled operation is left alone: there is nothing to
        // authorize, and the routing failure is the answer it gets.
        let operation = self.definition().protocol().routing_key(req).ok()?;

        if self.admission(req) == Admission::Unauthorized {
            return Some(&self.unauthorized as &dyn ErasedOperation);
        }

        // The account-entitlement gate (see the module docs): the caller's Security Lake
        // role decides, so the verdict precedes the modelled walk's own validation and
        // nothing the request names is looked up or written.
        let gate = entitlement_gate(operation)?;
        if self.account_role().administers_security_lake() {
            return None;
        }
        Some(match gate {
            Gate::AccessDenied => &self.not_entitled as &dyn ErasedOperation,
            Gate::Unauthorized => &self.unauthorized as &dyn ErasedOperation,
        })
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

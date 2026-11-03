// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-marketplacereporting-awsmarketplacereporting` service: how it claims requests and how it words the
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
//!   correctly. THIS service overrides it: `GetBuyerDashboard`'s availability
//!   decision is answered ahead of the modelled input constraints (see
//!   `AvailabilityGate` below), and an available caller is then handed to the
//!   generated walk unchanged.
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
    http, BoxFuture, ErasedOperation, Operation, OperationError, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-marketplacereporting-awsmarketplacereporting` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Answers `GetBuyerDashboard`'s availability decision ahead of the modelled
    /// input constraints; see [`AvailabilityGate`].
    availability_gate: AvailabilityGate<Eng>,
}

/// Serves `GetBuyerDashboard` with its availability decision made BEFORE the
/// modelled input constraints are checked, then hands an available caller to the
/// generated walk unchanged.
///
/// WHY this is a seam and not a line in the handler: the framework validates every
/// modelled constraint — `dashboardIdentifier`'s `@pattern` and `@length`,
/// `embeddingDomains`' `@length` — BEFORE a handler runs, and answers a violation
/// as the protocol's generic `400 ValidationException`. AWS Marketplace Reporting
/// decides the other way round: a caller the API is not available to is refused with
/// `UnauthorizedException` (401) whatever it sent, so an identifier outside that
/// pattern comes back 401 and not 400 — which is what the recorded traffic this
/// service is emulated from shows. A gate inside `handle` cannot produce that order,
/// because `handle` runs after the validator. So the decision is answered HERE, from
/// the same [`crate::ops::get_buyer_dashboard::access_refusal`] the handler itself
/// uses, and rendered in the modelled error shape through this service's own
/// protocol.
///
/// It is a pure READ of recorded registration state keyed by the request's principal:
/// it writes nothing, so it cannot change what a later caller sees, and two identical
/// requests — sequential or concurrent — get the identical answer.
///
/// Generic over the engine type for the same reason the rest of this file is: the
/// operation whose name and state it reads is, so naming a concrete engine here would
/// let this seam drift from the service the router mounts.
struct AvailabilityGate<Eng> {
    /// The shared engine, so the gate reads THIS service's store — the one the
    /// handler reads — rather than a private one.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> AvailabilityGate<Eng> {
    /// The routing key the model names the gated operation by, read off the
    /// operation rather than spelled out here.
    fn operation_name() -> &'static str {
        <crate::ops::get_buyer_dashboard::GetBuyerDashboard<Eng> as Operation>::NAME
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for AvailabilityGate<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let principal = crate::state::caller_principal(ctx.parts());
            // A store this service cannot reach is a server fault, not an
            // authorization decision, and it is not this seam's to state: `None`
            // falls through to the generated walk, which answers whatever that
            // request comes to there — the handler's modelled
            // `InternalServerException` for a request the boundary admits, and the
            // boundary's own refusal for one it does not.
            let refusal = crate::state::service_state(&*self.engine)
                .map(|state| state.lock().access_decision(&principal))
                .and_then(crate::ops::get_buyer_dashboard::access_refusal);
            if let Some(refusal) = refusal {
                return svc
                    .definition()
                    .protocol()
                    .serialize_error(
                        refusal.as_serializable(),
                        refusal.variant_schema(),
                        refusal.shape_name(),
                        refusal.http_status(),
                    )
                    .unwrap_or_else(|e| svc.definition().framework_error_response(e, req));
            }
            // Available: run the generated deserialize -> validate -> handle ->
            // serialize walk. Reached through the routing table rather than through
            // `override_operation`, which would route straight back here.
            match svc.definition().routing_table().get(Self::operation_name()) {
                Some(op) => op.run(svc, req, ctx).await,
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            availability_gate: AvailabilityGate { engine },
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
        // This service signs as `aws-marketplace` — a signing name it SHARES with
        // every other AWS Marketplace-family service (Catalog, Deployment, Discovery,
        // Agreement, Entitlement, Metering), each serving its own endpoint
        // (`reporting-marketplace` here). The router's routing key comes from the
        // SigV4 credential scope first, so all of them resolve to `aws-marketplace`
        // and matching on the signing name ALONE makes every one of them claim every
        // request — which the router answers `500 AmbiguousService`.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1` with one operation binding a literal first
        // segment (`/getBuyerDashboard`), disjoint from every sibling's, which is what
        // `claims_request` matches against the generated `ROUTE_PATH_SEGMENTS` when a
        // service declares no RPC protocol.
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
        // One case needs it: `GetBuyerDashboard`, whose availability decision is
        // made ahead of the modelled input constraints — see [`AvailabilityGate`]
        // for why that cannot be done from inside `handle`. The operation name comes
        // from the model, not from a literal, and a request naming any other
        // operation is left to the generated routing table.
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        (routing_key == AvailabilityGate::<Eng>::operation_name())
            .then_some(&self.availability_gate as &dyn ErasedOperation)
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

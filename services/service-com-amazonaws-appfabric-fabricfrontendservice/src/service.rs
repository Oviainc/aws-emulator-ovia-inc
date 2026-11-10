// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appfabric-fabricfrontendservice` service: how it claims requests and how it words the
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
//! ## How this service states a refused REQUEST
//!
//! Through [`request_validation`], which every modelled operation of this service is
//! served behind: a refusal of the request is answered in the modelled
//! `ValidationException`, whose `reason` the model requires and the framework's generic
//! refusal body cannot carry. The wording of each clause still lives in this file (see
//! [`constraint_clause`]), so one place decides how a violation reads and one place
//! decides which members the answer carries.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

pub mod request_validation;

/// The error code this service answers an input-constraint violation with: the
/// modelled `ValidationException` (`@httpError 400`,
/// `com.amazonaws.appfabric#ValidationException`), which is also the code the
/// framework's own refusal renderer picks. Restated here because
/// `override_refusal` replaces the code and the message together, and a message
/// without its matching code is worse than leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How AppFabric states a violation of a NUMERIC bound (`@range`): the input
/// position that failed and the bound it failed, and NOT the number that was
/// submitted there.
///
/// Every other violation class keeps the clause the framework builds
/// ([`Violation::message`]), which opens `Value '<submitted>' at …`: the value
/// is part of those messages, and this service has no evidence that AppFabric
/// withholds it there.
const RANGE_SUBJECT: &str = "Value at";

/// Render one constraint violation the way this service states it.
///
/// A `@range` violation is reworded to withhold the submitted number; anything
/// else is the framework's own clause, so one wording change cannot drift the
/// rest.
fn constraint_clause(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::MinValue(min) => format!(
            "{RANGE_SUBJECT} '{}' failed to satisfy constraint: \
             Member must have value greater than or equal to {min}",
            violation.path
        ),
        ViolationKind::MaxValue(max) => format!(
            "{RANGE_SUBJECT} '{}' failed to satisfy constraint: \
             Member must have value less than or equal to {max}",
            violation.path
        ),
        _ => violation.message(),
    }
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the
/// order the validating walk found them — the same envelope the framework
/// renders (`aws_smithy_schema_server::constraints::envelope`), so only the
/// reworded clause differs.
///
/// `None` declines the refusal (no violations to state), which keeps the
/// framework's own wording.
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations.iter().map(constraint_clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-appfabric-fabricfrontendservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// States a refused request of ANY of this service's operations in the modelled
    /// `ValidationException`; see [`request_validation`].
    request_validation: request_validation::RequestValidation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            request_validation: request_validation::RequestValidation::new::<Eng>(),
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
        // Every modelled operation is served behind the request validator, so that a
        // refusal of the REQUEST — an absent required member, a value outside a
        // declared constraint, a body the codec cannot read — is stated in the modelled
        // `ValidationException` with the `reason` the model requires, instead of the
        // framework's generic refusal body, which carries a message alone. See
        // [`request_validation`] for why that cannot be done from inside `handle` or
        // from `override_refusal`. The validator delegates to the operation the routing
        // table names, so a request with nothing to fault is served exactly as before.
        //
        // It also claims the request the routing table names NO operation for although
        // the model binds its method and path to one: the REST router reads an
        // operation's `@required` query/header bindings as route discriminators and
        // withholds the route of a request that leaves one out, which for a model that
        // overloads no route can only turn a refusable request into an "operation not
        // implemented" answer. Such a request is served by the operation whose own
        // `@http` route it addresses, which refuses it on its own values — the modelled
        // `ValidationException` naming the absent member, exactly as an absent body or
        // path member of that same operation is already refused. A request addressing no
        // route of this service is left to the framework's own routing refusal.
        self.request_validation
            .serves(&self.entry, req)
            .then_some(&self.request_validation as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // AppFabric states a violation of a numeric `@range` bound against the
        // input POSITION and the bound only — `Value at 'maxResults' failed to
        // satisfy constraint: Member must have value less than or equal to 100`
        // — so the number a caller submitted is not reflected back. Every other
        // violation class keeps the framework's clause, and the code stays the
        // modelled `ValidationException` (HTTP 400), which is what the framework
        // would have answered; only the wording changes.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        //
        // An input-constraint refusal of a modelled operation no longer reaches this
        // seam at all: `request_validation` answers it in the modelled shape BEFORE the
        // framework renders it, because the words are not the whole problem — `Refused`
        // is a code and a message, so the body it renders cannot carry the REQUIRED
        // `reason`. The wording below still decides how each clause reads (the
        // restatement builds its message from `constraint_envelope`), and still stands
        // for a refusal raised where no operation was named.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
            _ => None,
        }
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

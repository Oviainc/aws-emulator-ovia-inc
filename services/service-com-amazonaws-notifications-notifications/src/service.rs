// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-notifications-notifications` service: how it claims requests and how it words the
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
//!   before any handler ran. EDITED here: a numeric-bound (`@range`) violation is
//!   stated against the member and the bound, without quoting the submitted number
//!   back; every other class keeps the framework's wording. See [`clause`].
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
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the
/// modelled `ValidationException` (`@httpError 400`,
/// `com.amazonaws.notifications#ValidationException`, bound to every operation as a
/// service-common error), which is also the code the framework's own refusal renderer
/// picks. Restated here because `override_refusal` replaces the code and the message
/// together, and a message without its matching code is worse than leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The clause this service states for one violated constraint.
///
/// A NUMERIC BOUND (`@range`) violation names the member and the bound it broke and does
/// NOT quote the number submitted there: the recorded refusal of a `maxResults` outside
/// its range reads `Value at '<member>' failed to satisfy constraint: Member must have
/// value less than or equal to <max>`. Every other violation class keeps the framework's
/// own clause — which quotes the submitted value, the form
/// `services/common/src/validation.rs` states as the cross-service default — so this
/// service rewords only the half its recordings pin.
///
/// (The subject half of a validation message is one of the two parts
/// `services/common/src/validation.rs` lists as still to migrate into it; until that
/// lands it is spelled per service, as in the other service crates that word their own
/// refusals.)
fn clause(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::MinValue(min) => format!(
            "Value at '{}' failed to satisfy constraint: Member must have value greater than \
             or equal to {min}",
            violation.path
        ),
        ViolationKind::MaxValue(max) => format!(
            "Value at '{}' failed to satisfy constraint: Member must have value less than or \
             equal to {max}",
            violation.path
        ),
        _ => violation.message(),
    }
}

/// True for the violation classes this service words differently from the framework.
fn reworded(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)
    )
}

/// How this service states a refusal of the values a request carried, or `None` to keep
/// the framework's wording when no clause of it is one this service rewords.
///
/// The envelope is the framework's own (`N validation error(s) detected: …`, clauses
/// joined with `; ` in the order the validating walk found them), so a refusal mixing a
/// range violation with another class reads as one message.
fn invalid_refusal(violations: &[Violation]) -> Option<Refused> {
    if !violations.iter().any(reworded) {
        return None;
    }
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    let message = match clauses.len() {
        0 => return None,
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message,
    })
}

/// The `com-amazonaws-notifications-notifications` emulated service: the generated [`ServiceDefinition`] plus
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
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a NUMERIC BOUND failure against the member that failed
        // and the bound it broke, without quoting the submitted number back (see
        // [`clause`]); every other class keeps the framework's wording, and so does an
        // authentication refusal, which is not an input-constraint failure at all. The
        // code stays the modelled `ValidationException` (HTTP 400), which is what the
        // framework would have answered; only the wording changes.
        match refusal {
            Refusal::Invalid(violations) => invalid_refusal(violations),
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

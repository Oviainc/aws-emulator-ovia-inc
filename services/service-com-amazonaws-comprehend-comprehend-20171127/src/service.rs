// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-comprehend-comprehend-20171127` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with — the
/// same code (and HTTP 400) the framework would otherwise have named, restated
/// here because `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The requirement a violation that states NO value reports, in the phrasing the
/// validation envelope uses. `None` for every other violation class, which keeps
/// the framework's own clause for it ([`ViolationKind`] is `#[non_exhaustive]`, so
/// a class added upstream falls here too).
fn valueless_requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        _ => return None,
    })
}

/// One clause for one violated member.
///
/// Whether the rejected value is echoed follows the violation's CLASS, as AWS's
/// own request validator does. Two classes are reported WITHOUT it — `Value at
/// '<member>' failed to satisfy constraint: <requirement>`:
///
/// * a value outside a member's numeric `@range`, because the bound and the member
///   name are the whole of what a caller has to act on and the magnitude it sent is
///   not restated;
/// * a value longer than a member's `@length` MAXIMUM, which the clause has just
///   called oversized — restating it would quote back the very bytes the limit
///   refuses, and a collection has no scalar rendering to quote at all.
///
/// Every other class (an absent member, a `@length` MINIMUM, a `@pattern`, an enum
/// value set) keeps the framework's clause, which does echo the value: there the
/// value is what a caller has to read the requirement against.
fn violation_clause(violation: &Violation) -> String {
    match valueless_requirement(&violation.kind) {
        Some(requirement) => format!(
            "Value at '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ),
        None => violation.message(),
    }
}

/// The `ValidationException` message for a framework refusal: the same
/// `<N> validation error(s) detected: ` envelope the framework renders, over one
/// clause per violated member.
///
/// `None` when no violation is one of the valueless classes, which declines the
/// refusal and keeps the framework's wording verbatim rather than re-deriving it
/// here.
fn validation_envelope(violations: &[Violation]) -> Option<String> {
    if !violations
        .iter()
        .any(|violation| valueless_requirement(&violation.kind).is_some())
    {
        return None;
    }
    // At least one violation reached here — the check above returned otherwise —
    // so the envelope always has a clause to carry.
    let clauses: Vec<String> = violations.iter().map(violation_clause).collect();
    match clauses.as_slice() {
        [only] => Some(format!("1 validation error detected: {only}")),
        many => Some(format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        )),
    }
}

/// The `com-amazonaws-comprehend-comprehend-20171127` emulated service: the generated [`ServiceDefinition`] plus
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
        // Comprehend words such a refusal as a `ValidationException` (HTTP 400 —
        // the status and code the framework would also have answered, so only the
        // wording changes). The one difference from the framework's own rendering
        // is which clauses state the rejected value: a numeric `@range` violation
        // and a `@length` maximum name the member and the bound and echo NO value;
        // see [`violation_clause`]. An authentication refusal keeps the
        // framework's wording: it is not an input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: validation_envelope(violations)?,
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

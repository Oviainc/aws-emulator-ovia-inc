// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-healthlake-healthlake` service: how it claims requests and how it words the
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

/// The error code this service refuses a request that failed a modelled
/// constraint with: `ValidationException`, the service-common client error the
/// model declares at HTTP 400 (`smithy_query(query="service_info")` lists it
/// among the common errors, and every operation carries it).
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// How this service states a set of constraint violations the framework refused
/// before any handler ran.
///
/// The code is [`VALIDATION_EXCEPTION_CODE`] and the message is the AWS
/// `N validation error(s) detected` envelope — the same framing the framework
/// renders — with ONE difference, in the enum clause: see [`clause_for`].
fn refused_for(violations: &[Violation]) -> Refused {
    Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: validation_envelope(violations),
    }
}

/// `1 validation error detected: <clause>` / `N validation errors detected:
/// <clause>; <clause>`, each clause as [`clause_for`] words it.
///
/// `pub(crate)` because a handler re-stating a modelled constraint of its own
/// has to answer in the SAME words as the boundary validator; see
/// [`crate::ops::create_fhir_datastore`].
pub(crate) fn validation_envelope(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(clause_for).collect();
    match clauses.len() {
        // The framework only calls this with at least one violation; an empty
        // set still has to read as a refusal rather than as an empty sentence.
        0 => "1 validation error detected".to_string(),
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// One violation as this service states it.
///
/// A value outside an enumerated type's value set is the one class whose clause
/// states the member and the permitted set ALONE — `Value at '<member>' failed
/// to satisfy constraint: Member must satisfy enum value set: [...]` — with no
/// echo of what the caller sent: an unknown enum name never becomes a value of
/// the modelled type, so there is no value for the refusal to quote. Every
/// other class (length, range, pattern, uniqueness, absence) keeps the
/// framework's own clause, which quotes the offending value as AWS does.
fn clause_for(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::EnumValue(allowed) => format!(
            "Value at '{}' failed to satisfy constraint: Member must satisfy enum value set: [{}]",
            violation.path,
            allowed.join(", ")
        ),
        // `ViolationKind` is `#[non_exhaustive]`: anything else — including a
        // class added later — keeps the framework's wording rather than being
        // dropped or reworded on no evidence.
        _ => violation.message(),
    }
}

/// The message this service answers a value outside `allowed` at the member
/// `path` with, for a handler that re-states an enum constraint the boundary
/// validator has already applied. Worded by [`clause_for`], so the handler's
/// answer and the validator's are the same sentence.
pub(crate) fn enum_violation_message(path: &str, allowed: &[&str]) -> String {
    validation_envelope(&[Violation {
        path: path.to_string(),
        kind: ViolationKind::EnumValue(allowed.iter().map(|value| value.to_string()).collect()),
        // Deliberately absent: this service's enum clause quotes no value.
        value: None,
    }])
}

/// The `com-amazonaws-healthlake-healthlake` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        match refusal {
            // Constraint violations are stated the way this service states them
            // — code and message together; see `refused_for`.
            Refusal::Invalid(violations) => Some(refused_for(violations)),
            // Authentication is the framework's answer to give; this service
            // words it no differently.
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

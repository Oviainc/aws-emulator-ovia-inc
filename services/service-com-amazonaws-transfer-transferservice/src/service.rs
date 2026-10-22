// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-transfer-transferservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! Transfer's `ValidationException` message is a TEMPLATE over the request's
//! structure, never over its values: every clause names the member path and the
//! modelled constraint the member failed, and no clause quotes the value that failed
//! it (a `@pattern` is quoted as the model's expression with the anchors a Smithy
//! pattern writes around it removed). [`constraint_envelope`] is the one place that
//! sentence is built.
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

/// The error code this service answers an input-constraint violation with.
///
/// Transfer's model declares no `ValidationException` of its own, so a refusal the
/// framework makes before any handler runs renders the framework's synthetic one —
/// which is also the code the real service answers these refusals with. It is
/// restated here because `override_refusal` replaces code and message together: only
/// the message's CONSTRUCTION changes below.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The MODEL's pattern as a message states it: the expression without the anchors a
/// Smithy `@pattern` writes around it.
///
/// Smithy `@pattern` semantics are a *find* — the expression must match SOMEWHERE in
/// the value — so a model's leading `^` and trailing `$` are redundant notation
/// rather than part of the requirement, and the service quotes the expression
/// without them. A `$` the expression ESCAPES (`\$`) is a literal dollar sign, not an
/// anchor, so it stays.
///
/// The strip is POSITIONAL, so it handles the one anchored expression Transfer's model
/// writes (`^…$`) and would garble an alternation that anchors each branch
/// (`^a$|^b$`); the model carries none. The same rule is staged, unwired, in
/// `services/common/src/validation.rs` and inlined in ~25 service crates — this copy
/// is one of them, and should move when that module is wired up.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    let Some(trimmed) = body.strip_suffix('$') else {
        return body;
    };
    // An ODD number of backslashes before the `$` escapes it, making it a literal.
    let escaped = trimmed.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1;
    if escaped {
        body
    } else {
        trimmed
    }
}

/// The REQUIREMENT one violation states, as Transfer words it — the modelled
/// constraint the member failed, with nothing of the value that failed it.
///
/// `None` for a violation class this service has no wording for (the framework's
/// `ViolationKind` is `#[non_exhaustive]`, so one can be added), which declines the
/// rewording and keeps the framework's own.
///
/// The sentences are the ones `services/common/src/validation.rs` stages for every
/// service; that module is not wired into any crate yet, so this table is a copy, and
/// it should move when it is.
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        // The bracketed set is the model's own value order; no documented rule fixes it.
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// Render ONE constraint violation the way Transfer states it: the member PATH and
/// the modelled constraint it failed, and NOTHING of the value that was submitted.
///
/// A member that was absent has no submitted value at all, and `null` is the
/// service's own word for that absence rather than caller data, so that one clause
/// keeps its `Value null at '…'` subject.
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let subject = match violation.kind {
        ViolationKind::MissingRequiredMember => "Value null at",
        _ => "Value at",
    };
    Some(format!(
        "{subject} '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// The refusal message for a set of constraint violations: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the same
/// shape the framework's own envelope uses.
///
/// `None` declines the whole refusal — including one that mixes a class this service
/// words with a class it does not — which keeps one consistent wording for the
/// message rather than two half-rendered ones.
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-transfer-transferservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // Transfer states a constraint violation WITHOUT the value that was
        // submitted: the message names the member path and the modelled constraint
        // only, so caller-controlled input is never echoed back into a client's
        // logs. The code stays the `ValidationException` the framework
        // would have answered with. An authentication refusal, and a violation
        // class this service has no wording for, keep the framework's
        // wording.
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

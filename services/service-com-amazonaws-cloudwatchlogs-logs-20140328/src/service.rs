// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudwatchlogs-logs-20140328` service: how it claims requests and how it words the
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

/// The error code this service answers an input-constraint violation with.
///
/// The modeled `ValidationException` (`com.amazonaws.cloudwatchlogs#ValidationException`,
/// `@error("client")`, HTTP 400), which is also the code the framework's own refusal
/// renderer picks. Restated here because [`Service::override_refusal`] replaces the code
/// and the message together, and a message without its matching code is worse than
/// leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How CloudWatch Logs names the subject of a constraint clause when a value WAS
/// submitted: the POSITION in the input that failed, and never the value sitting there.
///
/// A caller's own data — a delivery id, a delivery-source name, an integration-source
/// identifier, a page size — is not reflected back into an error body a client may log or
/// match on.
const SUBJECT: &str = "Value at";

/// How the subject reads when the member was ABSENT, which is the cross-service rule
/// `services/common/src/validation.rs` states ("an ABSENT member keeps
/// `Value null at '<path>' …`"): there is no submitted value to withhold, and the `null`
/// is a fact of the request's SHAPE rather than a rendering of anything the caller sent.
const ABSENT_SUBJECT: &str = "Value null at";

/// The requirement one violation states: the modeled constraint the value failed.
///
/// Read off the violation's structural [`ViolationKind`] — the bound, the pattern, the
/// value set the MODEL declares — so a clause states the constraint rather than anything
/// about the request beyond which member it names.
///
/// THE PATTERN RENDERING RULE, stated once for the whole service (the handler-side
/// refusal in `crate::state` words its clause through [`one_violation_message`], so both
/// refusal paths read alike): a `@pattern` is reported as the MODEL spells it, anchors
/// included. The model is this emulator's only source for that expression, and the
/// recorded refusals of this service disagree with each other about the anchors — the
/// three recorded `DeliveryId` refusals print the model's `^[0-9A-Za-z]+$` anchored,
/// while the recorded `DeliverySourceName` one prints the model's `^[\w-]*$` as
/// `[\w-]*` — so no rule over the model can produce both and the expression is rendered
/// rather than rewritten.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
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
        ViolationKind::Pattern(pattern) => {
            format!("Member must satisfy regular expression pattern: {pattern}")
        }
        // A "value set" is just that: the requirement is WHICH values the model allows,
        // listed in the order the model declares them — the only order this emulator has
        // a source for.
        ViolationKind::EnumValue(allowed) => format!(
            "Member must satisfy enum value set: [{}]",
            allowed
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// Render one constraint violation the way this service states it: the input position and
/// the constraint it failed, and NOTHING of the value that was rejected.
///
/// The absent-member subject is chosen by whether a value reached the member at all
/// (`Violation::value`), not by the violation's class: `null` reports that the request
/// carried no value there, which is the one case where there is nothing to withhold.
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    Some(match violation.value {
        Some(_) => present_value_clause(&violation.path, &requirement),
        None => format!(
            "{ABSENT_SUBJECT} '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ),
    })
}

/// The clause a present-but-invalid value at `path` is refused with: the position and the
/// requirement, with the value withheld.
fn present_value_clause(path: &str, requirement: &str) -> String {
    format!("{SUBJECT} '{path}' failed to satisfy constraint: {requirement}")
}

/// The envelope one clause is wrapped in.
fn one_error_envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The whole `ValidationException` message for ONE refused member, for a violation a
/// HANDLER found itself at a member the framework's validating walk no longer sees (a
/// member carrying `skip_validation`, which `crate::state` judges).
///
/// Built from the same subject, the same envelope and the same pattern rendering as a
/// refusal the framework raised, so one violation reads as one message whichever of the
/// two refused the request, and the submitted value is withheld either way.
pub(crate) fn one_violation_message(member: &str, requirement: &str) -> String {
    one_error_envelope(&present_value_clause(member, requirement))
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the order the
/// validating walk found them.
///
/// `None` declines the whole refusal — see [`requirement`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(one_error_envelope(&clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-cloudwatchlogs-logs-20140328` emulated service: the generated [`ServiceDefinition`] plus
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
        // CloudWatch Logs states a member-constraint failure against the INPUT POSITION
        // that failed and the constraint it failed — never the value that was submitted
        // there, so a delivery id, a source name or a page size a caller sent is not
        // reflected back into a 400 body. The code stays the modeled
        // `ValidationException` (HTTP 400), which is what the framework would have
        // answered; only the wording changes.
        //
        // A `@pattern` keeps the anchors the model spells it with — the rule stated once
        // on `requirement` above, and shared with the handler-side refusal through
        // `one_violation_message`.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
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

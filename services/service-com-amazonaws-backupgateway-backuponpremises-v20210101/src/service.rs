// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-backupgateway-backuponpremises-v20210101` service: how it claims requests and how it words the
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
//! This file also owns how this service WORDS a constraint violation — the clause, the
//! member's validation path and the `N validation error(s) detected: ` envelope — because
//! `override_refusal` below is the seam the framework renders one through. A rule the model
//! states only in prose reaches no boundary validator, so the handler that refuses it words
//! its message through [`handler_constraint_violation`] rather than spelling a sentence of
//! its own: one condition class, one wording, one spelling of the member.
//!
//! Two of those seams also serve this service's ACTIVATION handshake — the unsigned,
//! non-modelled HTTP request a deployed gateway appliance makes to be issued the
//! `ActivationKey` a later `CreateGateway` redeems. It carries no credential scope and no
//! `X-Amz-Target`, so `can_handle` has to claim it and `override_routing_error` is where it
//! is answered; see [`activation`] for the whole rule.

mod activation;

use aws_smithy_schema_server::constraints::path as validation_path;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the modelled
/// `ValidationException`, which this service declares for every operation
/// (`smithy_query(query="service_info")` -> `common_errors`) with `@httpError 400`. It
/// is the framework's own default code for a refusal too, restated here because
/// `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The members this service marks `@sensitive`, as a validation path spells them.
///
/// `smithy_query(query="shape",shape="Password,Username")`: the `Password` and
/// `Username` string shapes both carry `sensitive: true`, and they are the only
/// sensitive shapes in this model. They are reached as the `Password` / `Username`
/// members of `ImportHypervisorConfigurationInput`,
/// `TestHypervisorConfigurationInput` and `UpdateHypervisorInput`; a path segment is
/// lowerCamel, so a violation on one of them is reported at `password` / `username`.
const SENSITIVE_MEMBERS: &[&str] = &["password", "username"];

/// Whether `path` ends in a member this service marks `@sensitive`.
///
/// The LAST segment decides: the member a violation is reported on is the one whose
/// value would be echoed, wherever in a nested path it sits.
fn names_a_sensitive_member(path: &str) -> bool {
    let member = path.split('.').next_back().unwrap_or(path);
    SENSITIVE_MEMBERS.contains(&member)
}

/// Render one constraint violation the way this service states it: the member path and
/// the violated constraint, and the rejected value ONLY where the member is not
/// `@sensitive`.
///
/// A `@sensitive` member's value is a credential — this service's are a hypervisor's
/// password and username — and the service that redacts it is the only thing standing
/// between a rejected credential and every client log that records an error body, so
/// the clause names the member and the constraint and stops there.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = match &violation.kind {
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
        // The requirement reported is the expression itself, without the whole-value
        // `^` / `$` pair the model writes around it: this service states the character
        // class a value must satisfy, not the anchoring the model uses to make the match
        // total. An expression that anchors its own BRANCHES is already a whole-value
        // expression and is stated as the model declares it — the shared rendering's rule,
        // called rather than restated here.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            ::service_common::validation::unanchored_keeping_inner_anchors(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    let subject = match (&violation.kind, &violation.value) {
        // A value the validating walk itself WITHHELD is a withheld value and not an
        // absent one: the walk drops a `@sensitive` member's value as it records the
        // violation, so an empty value half is not evidence that the member was absent.
        _ if violation.sensitive => "Value".to_string(),
        // An absent member is reported as the null it was: a fact of the request's
        // shape, not a rendering of a value the caller sent.
        (ViolationKind::MissingRequiredMember, _) => "Value null".to_string(),
        // A `@sensitive` member names itself and nothing of what it held. Its value is
        // DROPPED from the violation by the framework's own walk, which reports the trait
        // there, so the two valueless cases are told apart by that flag rather than by the
        // missing value.
        _ if violation.sensitive => "Value".to_string(),
        (_, None) => "Value null".to_string(),
        (_, Some(_)) if names_a_sensitive_member(&violation.path) => "Value".to_string(),
        (_, Some(shown)) => format!("Value '{shown}'"),
    };
    Some(constraint_clause_at(&subject, &violation.path, &requirement))
}

/// One clause of this service's constraint wording: the subject, the member the violation
/// is reported ON, and the requirement that member failed.
///
/// The ONE place the clause is spelled, so a constraint this service refuses in a handler
/// ([`handler_constraint_violation`]) states the identical sentence a violation of a
/// modelled constraint on the very same member does.
fn constraint_clause_at(subject: &str, at: &str, requirement: &str) -> String {
    format!("{subject} at '{at}' failed to satisfy constraint: {requirement}")
}

/// How a refusal of this service NAMES the member it is about: the member's validation
/// path, which is lowerCamel whatever case the model spells the member in.
///
/// Built through the framework's own path module — the one the validating walk builds the
/// paths in [`Violation`] with — so a member cannot be named one way by a refusal the
/// boundary made and another way by a refusal a handler made.
pub(crate) fn member_path(member: &str) -> String {
    validation_path::member(member)
}

/// The validation path of `member` inside the element at `position` of the list member
/// `list_member` — the path a violation INSIDE a collection is reported at, which names
/// the element's place so a caller sending several of them is told which one was refused.
///
/// `position` counts from 1, as a validation path counts a list's elements.
pub(crate) fn element_member_path(list_member: &str, position: usize, member: &str) -> String {
    validation_path::join(&[
        validation_path::member(list_member),
        validation_path::element(position),
        validation_path::member(member),
    ])
}

/// This service's whole `ValidationException$Message` for ONE constraint a HANDLER refused
/// itself: a rule the model states in prose, which no trait expresses and the boundary
/// validator therefore never saw.
///
/// Same clause, same envelope and same member naming as a boundary refusal: "this member
/// failed a constraint" is one condition class, so this service answers it in one wording
/// wherever the check happens to live. `at` is a [`member_path`] or an
/// [`element_member_path`]; `value` is the value refused, or `None` where the clause names
/// the member alone, as it does for a `@sensitive` one.
pub(crate) fn handler_constraint_violation(
    value: Option<&str>,
    at: &str,
    requirement: &str,
) -> String {
    let subject = match value {
        Some(shown) => format!("Value '{shown}'"),
        None => "Value".to_string(),
    };
    envelope(&[constraint_clause_at(&subject, at, requirement)])
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    (!clauses.is_empty()).then(|| envelope(&clauses))
}

/// The AWS envelope over the clauses one refusal states, counting them: the whole message
/// a client reads, for a refusal made at the boundary or in a handler alike.
///
/// At least one clause — an empty refusal is declined before it gets here, by
/// [`constraint_envelope`].
fn envelope(clauses: &[String]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The `com-amazonaws-backupgateway-backuponpremises-v20210101` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the [`activation`] handshake is answered from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own: the activation it records has to go into the one store
    /// `CreateGateway` reads, never a second copy.
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        if let Some(key) = smithy_server_runtime::resolve_route_key(parts) {
            if key == self.entry.identity().signing_name() {
                return true;
            }
        }
        // And the ACTIVATION handshake, which is not a modelled request: an appliance makes
        // it before anybody holds a gateway, so it carries no AWS credential to resolve a
        // route key from and the front door would answer `404 UnknownService` for the very
        // endpoint a client was told to ask a key of. The claim is narrow — a `GET` of the
        // endpoint root carrying `activationRegion` — so it takes no request another service
        // serves; see [`activation`].
        activation::claims_parts(parts)
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
        // This service states a constraint violation on a `@sensitive` member WITHOUT
        // the value that violated it: the clause names the member path and the
        // constraint only, so a rejected hypervisor password or username is never
        // echoed back into a client's logs. A non-sensitive member keeps its value, as
        // the framework renders it. The code stays the modelled `ValidationException`
        // (HTTP 400), which is what the framework would have answered — only the
        // wording changes.
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
        //
        // The ACTIVATION handshake IS such a request: it names no modelled operation, so
        // this pre-authentication seam is where it is answered — which is what lets an
        // appliance holding no AWS credential be issued a key at all. Every other
        // unresolvable request keeps the protocol-correct default.
        if activation::claims_request(req) {
            return Some(activation::serve(&*self.engine, req));
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

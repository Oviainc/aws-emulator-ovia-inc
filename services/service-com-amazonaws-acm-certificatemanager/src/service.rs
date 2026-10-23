// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-acm-certificatemanager` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name. EDITED HERE: it
//!   also claims a request addressed to an ACME enrollment host (see "The ACME HTTP
//!   surface" below).
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. EDITED HERE: a request to a published ACME URL that presents a SigV4
//!   credential is named as an operation, so the router verifies that credential before
//!   anything is served. Every other request resolves through the generated routing
//!   table.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. EDITED HERE: that is what an unsigned ACME client's
//!   request to a published ACME URL is, by construction, and this is where it is
//!   served. Anything else keeps the protocol-correct default (awsJson -> 404).
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
//! ## The ACME HTTP surface
//!
//! ACM publishes two kinds of hostname. The CONTROL plane is
//! `acm.<region>.amazonaws.com`, whose first label names the service, and that is what
//! the framework's route key resolves. Beside it, every managed ACME endpoint is given
//! a URL on the Region's ACME ENROLLMENT host,
//! `acm-acme-enroll.<region>.amazonaws.com` ([`crate::state::acme_endpoint_url`]),
//! which is the URL `AcmeEndpoint$EndpointUrl` hands an ACME client and the address
//! that client is configured with. A request to it has to reach THIS service rather
//! than the control-plane resolver, which cannot name a service from the enrollment
//! label and answers `404 UnknownService` — for a URL this service handed out.
//!
//! Three seams carry it, and nothing else changes:
//!
//! - [`Service::can_handle`] also claims a request whose host is an enrollment host
//!   ([`acme_http::enrollment_region_of_parts`]), which is what routes it here at all.
//! - [`Service::override_routing_error`] serves the request that presents NO
//!   credential. An ACME client holds none, and an ACME interaction names no modelled
//!   operation by construction, so the router reaches exactly this seam for it — BEFORE
//!   authentication, which is what lets an unsigned ACME client be answered at all.
//! - [`Service::override_operation`] serves the request that DOES present a SigV4
//!   credential, through the same [`acme_http::AcmeHttpSurface`]. The router
//!   authenticates whatever it names an operation for, so naming one here is what puts
//!   the emulator's own signature verification in front of a credentialed request to a
//!   published URL — the same verification a control-plane request gets, rather than a
//!   second signature check written into this service.
//!
//! What is served on that surface — the RFC 8555 directory, a fresh nonce, and a `501`
//! problem document for the JWS interactions this emulator cannot perform — is
//! documented in [`acme_http`].

pub mod acme_http;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the
/// modeled `ValidationException` (`@httpError 400`, `com.amazonaws.acm#ValidationException`),
/// which is also the code the framework's own refusal renderer picks. Restated
/// here because `override_refusal` replaces the code and the message together,
/// and a message without its matching code is worse than leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How ACM names the subject of a constraint clause when a value WAS submitted:
/// the position in the input that failed, and NOT the value itself.
///
/// A caller's own data (a certificate ARN, an idempotency token, a domain name)
/// is never reflected back into an error body a client may log.
const SUBJECT: &str = "Value of the input at";

/// How the subject reads when the member was ABSENT, which is the cross-service
/// rule `services/common/src/validation.rs` states ("an ABSENT member keeps
/// `Value null at '<path>' …`"): there is no submitted value to withhold, and the
/// `null` is a fact of the request's SHAPE rather than a rendering of anything the
/// caller sent.
const ABSENT_SUBJECT: &str = "Value null at";

/// The requirement one violation states, as ACM phrases it.
///
/// The requirement half and the unanchored `@pattern` below are two of the rules
/// `services/common/src/validation.rs` lists as still to migrate into it (its
/// "Still to migrate here (staged, not yet wired)" section): that module is
/// declared, but it owns only which value a clause echoes, not these, so they are
/// inlined here as they are in the other service crates that word their own
/// refusals. The migration that moves them there owns this copy too. (ACM echoes
/// no value at all, so that module's `with_submitted_values` does not apply.)
///
/// Returns `None` for a violation class this service has no wording for, which
/// lets [`constraint_envelope`] decline the whole refusal and keep the
/// framework's wording rather than invent a sentence for it ([`ViolationKind`]
/// is `#[non_exhaustive]`, so a class can be added upstream).
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
        // The requirement reported is the expression a value must match, without
        // the `^` / `$` the model spells it with: the anchoring makes the model's
        // match total, it is not part of the expression the service advertises,
        // so a client that re-validates locally with the pattern it read out of
        // the message uses the one ACM documents.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        // A "value set" is just that: the requirement is WHICH values the model
        // allows, and the order they are listed in carries no information. They
        // are listed in the order the model declares them, which is the only
        // order this emulator has a source for.
        ViolationKind::EnumValue(allowed) => enum_value_set(allowed),
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// The requirement an enum violation states: WHICH values the model allows.
fn enum_value_set<S: AsRef<str>>(allowed: &[S]) -> String {
    let values: Vec<&str> = allowed.iter().map(AsRef::as_ref).collect();
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}

/// The whole `ValidationException` message for an enum violation a HANDLER found
/// itself, at a member the framework's validating walk does not reach — a value
/// sitting directly on a union member, which that walk does not descend into.
///
/// Built from the same subject, the same requirement and the same envelope as a
/// refusal the framework raised, so one violation class reads as one message
/// whichever of the two refused the request, and the submitted value is withheld
/// either way.
pub(crate) fn enum_value_refusal(path: &str, allowed: &[&str]) -> String {
    one_error_envelope(&format!(
        "{SUBJECT} '{path}' failed to satisfy constraint: {}",
        enum_value_set(allowed)
    ))
}

/// The subject a violation's clause opens with: the absent-member form for a
/// member the request never sent, else the value-withholding form.
fn subject(kind: &ViolationKind) -> &'static str {
    match kind {
        ViolationKind::MissingRequiredMember => ABSENT_SUBJECT,
        _ => SUBJECT,
    }
}

/// Render one constraint violation the way this service states it: the input
/// position and the violated constraint, and NOTHING of the value that was
/// rejected.
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let subject = subject(&violation.kind);
    // A failure of the collection's own ELEMENT is reported against the
    // collection member, with the element's requirement nested — the position a
    // caller fixes is the collection it sent, and the index inside it is not part
    // of the requirement the model declares on the element.
    let (path, requirement) = match collection_element(&violation.path) {
        Some(collection) => (
            collection,
            format!("Member must satisfy constraint: [{requirement}]"),
        ),
        None => (violation.path.as_str(), requirement),
    };
    Some(format!(
        "{subject} '{path}' failed to satisfy constraint: {requirement}"
    ))
}

/// The collection member a violation belongs to, when the failing leaf IS one of
/// that collection's elements.
///
/// The validating walk names a list element `<collection>.<1-based index>.member`.
/// A path ENDING at that step is the element itself, so the clause is reported
/// against `<collection>`; a member reached THROUGH an element
/// (`tags.1.member.key`) is an input position of its own and is reported there.
///
/// The index must be an integer, which is what distinguishes a list step from a
/// map entry (`<member>.<caller-chosen key>.member` — see `path::entry` in
/// `vendor/smithy-schema-server/src/constraints.rs`). No ACM member targets a map
/// with a constrained value, so no caller key reaches here today; were one added,
/// a purely numeric key would be read as an index.
fn collection_element(path: &str) -> Option<&str> {
    let head = path.strip_suffix(".member")?;
    let (collection, index) = head.rsplit_once('.')?;
    let indexed = !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit());
    (indexed && !collection.is_empty()).then_some(collection)
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not
/// the end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    // See the note on `requirement`: one of the rules
    // `services/common/src/validation.rs` lists as still to migrate into it,
    // inlined until it owns them.
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The AWS envelope a single-clause refusal message carries.
fn one_error_envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the
/// order the validating walk found them.
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

/// The `com-amazonaws-acm-certificatemanager` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves the ACME HTTP surface an ACME endpoint publishes — the wire its
    /// `EndpointUrl` advertises. Held on the service because
    /// [`Service::override_operation`] hands the router a BORROW of one of its own
    /// operations, and it owns the shared engine so every answer is computed from the
    /// same store the control-plane handlers write.
    acme_http: acme_http::AcmeHttpSurface<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            acme_http: acme_http::AcmeHttpSurface::new(Arc::clone(&engine)),
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
        // A request whose resolved route key is this service's signing name is the
        // control plane, exactly as the framework's default claims it.
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request addressed to an ACME enrollment host, the hostname this service
        // PUBLISHES in every ACME endpoint's `EndpointUrl`, whose first label is not
        // this service's signing name and which no route key can therefore name. See
        // the module docs: without this the router answers `404 UnknownService` for a
        // URL this service handed out.
        acme_http::enrollment_region_of_parts(parts).is_some()
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // A request to a published ACME URL that PRESENTS a SigV4 credential is named as
        // an operation, so the router verifies that credential's signature before
        // anything is served — the router authenticates only what it names an operation
        // for. A request carrying NO credential is left unnamed on purpose: it is
        // answered by `override_routing_error` below, which the router reaches before
        // authentication, so an ACME client — which holds no AWS credential at all —
        // still gets this service's own answer rather than the emulator's "sign this
        // request".
        if acme_http::carries_sigv4_credential(req) && acme_http::addresses_acme_surface(req) {
            return Some(&self.acme_http);
        }
        // Everything else resolves through the generated routing table.
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // ACM states a member-constraint failure against the INPUT POSITION that
        // failed and the constraint it failed — never the value that was
        // submitted there, so a certificate ARN, an idempotency token or a domain
        // name a caller sent is not reflected back into a 400 body. A `@pattern`
        // requirement names the expression without the model's `^` / `$`
        // anchoring. The code stays the modeled `ValidationException` (HTTP 400),
        // which is what the framework would have answered; only the wording
        // changes.
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
        // modeled operation — neither `override_operation` nor the generated routing
        // table resolved it. An ACME interaction is exactly that case by construction:
        // it carries no `X-Amz-Target` and no modelled operation serves its path, and
        // an ACME client presents no credential (`override_operation` names an
        // operation only for the credentialed ones), so this is where an unsigned
        // client's fetch of a published ACME URL is served. Anything else keeps the
        // framework's protocol-correct default (awsJson -> 404).
        acme_http::addresses_acme_surface(req).then(|| self.acme_http.serve(req))
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

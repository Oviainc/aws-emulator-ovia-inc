// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-verifiedpermissions-verifiedpermissions` service: how it claims requests and how it words the
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
//!   before any handler ran. This service words a broken model constraint itself
//!   ([`validation_refusal`]) and keeps the framework's wording for every other
//!   class of refusal.
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
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_schema_server::Violation;

/// The error code this service answers a boundary constraint refusal with — the
/// `ValidationException` its model declares on every operation, which is also the
/// code the framework's own default picks on this protocol. It travels with the
/// message below, so a reworded refusal never loses its matching code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The segment the framework's dotted member path puts between a collection and
/// one of its elements (`requests.1.member.principal`). A JSON pointer names the
/// position itself, so this step has no counterpart in one.
const COLLECTION_STEP: &str = "member";

/// One member of the request as a JSON pointer rooted at the request body, built
/// from the dotted path the framework walks its constraints with.
///
/// Each dotted segment becomes one pointer segment. A step from a collection into
/// one of its elements arrives as `<position-or-key>.member`, which a pointer
/// spells as the position alone: an element's position is renumbered from the
/// framework's 1-based count to the 0-based index RFC 6901 gives an array
/// element, and a map entry's key — a value the caller chose rather than a model
/// name — is kept as sent, with the two characters a pointer segment escapes
/// (`~` and `/`) escaped. A numeric map key is therefore indistinguishable from a
/// list position here and is renumbered as one; the dotted form carries nothing
/// that tells the two apart.
fn json_pointer(dotted: &str) -> String {
    let mut pointer = String::with_capacity(dotted.len() + 1);
    let mut segments = dotted.split('.').peekable();
    while let Some(segment) = segments.next() {
        pointer.push('/');
        if segments.peek() == Some(&COLLECTION_STEP) {
            // The step into the element is spelled by the position alone, so the
            // `member` segment that followed it is consumed here.
            segments.next();
            match segment.parse::<usize>() {
                Ok(position) => pointer.push_str(&position.saturating_sub(1).to_string()),
                Err(_) => pointer.push_str(&escaped_segment(segment)),
            }
        } else {
            pointer.push_str(&escaped_segment(segment));
        }
    }
    pointer
}

/// One pointer segment with the two characters a JSON pointer gives a meaning of
/// its own escaped: `~` as `~0` and `/` as `~1` (RFC 6901), so a caller-chosen
/// key holding either still names one member instead of splitting into two.
fn escaped_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// The clause one violation states: the member it is about, addressed as a JSON
/// pointer, and the requirement that member failed — and NOTHING of the value
/// that was submitted.
///
/// The requirement half is the framework's, taken off the clause it renders for
/// this violation rather than rewritten here, so a constraint class it learns to
/// report cannot drift from this wording. Only the `Value '<submitted>' ` /
/// `Value null ` subject is replaced, because this service states no value at
/// all: a member outside its constraint can carry caller data, and the member the
/// clause names is what a caller has to fix.
fn clause(violation: &Violation) -> String {
    let addressed = Violation {
        path: json_pointer(&violation.path),
        kind: violation.kind.clone(),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    };
    let stated = addressed.message();
    let subject = match &addressed.value {
        Some(submitted) => format!("Value '{submitted}' "),
        None => "Value null ".to_string(),
    };
    match stated.strip_prefix(&subject) {
        Some(requirement_half) => format!("Value {requirement_half}"),
        // The framework worded the clause some other way; it is still a true
        // statement of the refusal, so it is passed through rather than dropped.
        None => stated,
    }
}

/// How this service states the framework's refusal of a request that broke a
/// constraint its model declares: one clause per violation, in the envelope
/// `N validation error(s) detected. <clause>; <clause>`.
///
/// The count is followed by a PERIOD, where the framework's own default follows
/// it with a colon, and each clause addresses its member as a JSON pointer rather
/// than as a dotted path, because that pointer is the machine-readable half of
/// the message — the same spelling this service's own input checks report in
/// `ValidationException$fieldList` ([`crate::ops::paths`]). `None` when there is
/// nothing to state, which keeps the framework's wording.
fn validation_refusal(violations: &[Violation]) -> Option<Refused> {
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    let message = match clauses.as_slice() {
        [] => return None,
        [only] => format!("1 validation error detected. {only}"),
        many => format!(
            "{} validation errors detected. {}",
            many.len(),
            many.join("; ")
        ),
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message,
    })
}

/// The `com-amazonaws-verifiedpermissions-verifiedpermissions` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it. This service states that class of refusal in its own
        // envelope — see [`validation_refusal`], which carries the matching
        // `ValidationException` code — and keeps the framework's code and wording
        // for every other class, which an unauthenticated caller's 403 is.
        match refusal {
            Refusal::Invalid(violations) => validation_refusal(violations),
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

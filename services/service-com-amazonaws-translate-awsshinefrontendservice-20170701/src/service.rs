// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-translate-awsshinefrontendservice-20170701` service: how it claims requests and how it words the
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

use crate::state::{
    limit_exceeded, text_size_limit_exceeded, DISPLAY_LANGUAGE_CODE_VALUE_SET, MAX_DOCUMENT_BYTES,
    MAX_TEXT_BYTES,
};

/// The error code this service answers an input-constraint violation with when
/// it keeps the framework's class — the same code (and HTTP 400) the framework
/// would have named, restated here because a reworded refusal replaces the code
/// and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error class the service answers a request whose text is larger than the
/// documented limit with (`smithy_query operation=TranslateText`).
const TEXT_SIZE_LIMIT_EXCEEDED: &str = "TextSizeLimitExceededException";

/// The error class the service answers a request whose document is larger than
/// the documented limit with (`smithy_query operation=TranslateDocument`).
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The dotted path of the member carrying the content of the document one
/// synchronous translation translates, as a refusal names it.
const DOCUMENT_CONTENT_MEMBER_PATH: &str = "document.content";

/// The dotted path of the member carrying the text one translation request
/// translates, as a refusal names it.
const TEXT_MEMBER_PATH: &str = "text";

/// The size, in bytes, of the value a refusal echoed.
///
/// The framework caps the value it echoes into a refusal message and states the
/// full byte length of what it elided, so the size of the value the REQUEST
/// carried survives in the refusal either way: as that stated length, or — for a
/// value short enough to be echoed whole — as the echo's own length.
fn echoed_byte_length(echoed: &str) -> usize {
    echoed
        .strip_suffix(" bytes]")
        .and_then(|head| head.rsplit_once("... ["))
        .and_then(|(_, digits)| digits.parse::<usize>().ok())
        .unwrap_or(echoed.len())
}

/// The violation reporting that a request's text is longer than the documented
/// limit allows, if any.
///
/// The limit AWS documents for that member counts BYTES, so a request past it is
/// this service's own size refusal rather than a shape-constraint rejection; the
/// member's character-length bound is the only form the model can state it in,
/// and a request that trips it has necessarily passed the byte limit too.
fn oversize_text(violations: &[Violation]) -> Option<&Violation> {
    violations.iter().find(|violation| {
        violation.path == TEXT_MEMBER_PATH
            && matches!(violation.kind, ViolationKind::MaxLength(max)
                if usize::try_from(max).is_ok_and(|bound| bound == MAX_TEXT_BYTES))
    })
}

/// The violation reporting that a request's document is larger than the
/// documented limit allows, if any.
///
/// "The document size must not exceed 100 KB" is a limit of the operation, and
/// the error class it names is the operation's own `LimitExceededException`
/// rather than this service's generic input-constraint refusal — which is also
/// the only class the model declares for it, since the document member's length
/// bound states exactly that limit.
fn oversize_document(violations: &[Violation]) -> Option<&Violation> {
    violations.iter().find(|violation| {
        violation.path == DOCUMENT_CONTENT_MEMBER_PATH
            && matches!(violation.kind, ViolationKind::MaxLength(max)
                if usize::try_from(max).is_ok_and(|bound| bound == MAX_DOCUMENT_BYTES))
    })
}

/// The accepted-value set of an enum this service renders in its own order,
/// matched by the set of values the violation carries rather than by the member
/// that happened to carry it, so every member targeting that enum reads alike.
fn service_value_order(allowed: &[String]) -> Option<&'static [&'static str]> {
    let same_set = allowed.len() == DISPLAY_LANGUAGE_CODE_VALUE_SET.len()
        && DISPLAY_LANGUAGE_CODE_VALUE_SET
            .iter()
            .all(|value| allowed.iter().any(|declared| declared == value));
    same_set.then_some(DISPLAY_LANGUAGE_CODE_VALUE_SET)
}

/// One clause for one violated member.
///
/// Only the accepted-value set of an out-of-enum value is reworded, and only in
/// its ORDER: the set a refusal states is the one the service emits, which is
/// part of the fixed text of that message. Every other class keeps the
/// framework's clause verbatim.
fn violation_clause(violation: &Violation) -> String {
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return violation.message();
    };
    match service_value_order(allowed) {
        Some(ordered) => format!(
            "Value '{}' at '{}' failed to satisfy constraint: Member must satisfy enum \
             value set: [{}]",
            violation.value.as_deref().unwrap_or_default(),
            violation.path,
            ordered.join(", ")
        ),
        None => violation.message(),
    }
}

/// The `ValidationException` message for a framework refusal: the same
/// `<N> validation error(s) detected: ` envelope the framework renders, over one
/// clause per violated member.
///
/// `None` when no violation is one this service words differently, which
/// declines the refusal and keeps the framework's wording verbatim rather than
/// re-deriving it here.
fn validation_envelope(violations: &[Violation]) -> Option<String> {
    let reworded = violations.iter().any(|violation| {
        matches!(&violation.kind, ViolationKind::EnumValue(allowed)
            if service_value_order(allowed).is_some())
    });
    if !reworded {
        return None;
    }
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

/// The `com-amazonaws-translate-awsshinefrontendservice-20170701` emulated service: the generated [`ServiceDefinition`] plus
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
        // Three classes are not this service's generic input-constraint refusal:
        //
        // - a translation request carrying more text than the service allows,
        //   which is a modelled size refusal of that operation and names its own
        //   error class, the limit and the size the request sent;
        // - a document translation whose document is larger than the operation
        //   allows, which is that operation's own modelled limit refusal;
        // - an out-of-enum value, whose accepted-value set this service states in
        //   its own order.
        //
        // Everything else — including an authentication refusal, which is not an
        // input-constraint failure at all — keeps the framework's code and
        // wording.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        if let Some(violation) = oversize_text(violations) {
            let bytes = echoed_byte_length(violation.value.as_deref().unwrap_or_default());
            return Some(Refused {
                code: TEXT_SIZE_LIMIT_EXCEEDED.to_string(),
                message: text_size_limit_exceeded(bytes).message.unwrap_or_default(),
            });
        }
        if oversize_document(violations).is_some() {
            return Some(Refused {
                code: LIMIT_EXCEEDED.to_string(),
                message: limit_exceeded().message.unwrap_or_default(),
            });
        }
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: validation_envelope(violations)?,
        })
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

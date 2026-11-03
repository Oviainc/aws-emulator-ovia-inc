// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-redshiftserverless-redshiftserverless` service: how it claims requests and how it words the
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

/// A modelled `@pattern` as this service states it inside a validation message: without
/// the anchors.
///
/// Smithy's `@pattern` is an unanchored FIND, so `^...$` in a model is how that model
/// writes "the whole value", not part of the requirement a caller is told about — this
/// service interpolates the constraint itself. A `$` escaped by an odd number of
/// backslashes is a literal dollar inside the expression, not the end anchor, so it stays.
///
/// The SAME helper is hand-written in four sibling services for the same reason
/// (`service-com-amazonaws-controlcatalog-controlcatalog/src/service.rs`,
/// `...-directoryservicedata-directoryservicedata/src/service.rs`,
/// `...-osis-amazonopensearchingestionservice/src/service/validation.rs`,
/// `...-computeoptimizerautomation-computeoptimizerautomationservice/src/state.rs`), and
/// this copy is deliberately the same function as theirs so the five cannot drift. The
/// rendering rule is framework-level: it belongs in the shared constraint renderer
/// (`vendor/smithy-schema-server/src/constraints.rs`, `Violation::requirement`), which a
/// fix may not change from here — once it learns this, delete all five and the
/// `override_refusal` body below with them.
fn unanchored(pattern: &str) -> &str {
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

/// True when [`restate`] states this violation differently from the framework, i.e. when
/// there is anything to reword at all.
fn is_reworded(violation: &Violation) -> bool {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => unanchored(pattern) != pattern.as_str(),
        _ => false,
    }
}

/// One violation as this service states it: a `@pattern` violation carrying the
/// un-anchored constraint, every other class untouched.
///
/// Only the DATUM changes — the sentence is still written by the framework's
/// `Violation::message()` through [`validation_envelope`], so there is one copy of the
/// wording and this service cannot drift from it.
fn restate(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(unanchored(pattern).to_string()),
            value: violation.value.clone(),
            sensitive: violation.sensitive,
        },
        _ => violation.clone(),
    }
}

/// The `ValidationException` envelope for `violations`, or `None` when this service states
/// every one of them exactly as the framework does (so the framework should answer).
///
/// Both the clauses and the envelope around them are rendered by the framework's own
/// [`aws_smithy_schema_server::envelope`]: this service only hands it violations whose
/// `@pattern` datum has been restated.
fn validation_envelope(violations: &[Violation]) -> Option<String> {
    if !violations.iter().any(is_reworded) {
        return None;
    }
    let restated: Vec<Violation> = violations.iter().map(restate).collect();
    aws_smithy_schema_server::envelope(&restated)
}

/// The `com-amazonaws-redshiftserverless-redshiftserverless` emulated service: the generated [`ServiceDefinition`] plus
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
        // One rewording: how a `@pattern` is rendered INSIDE the sentence. This service
        // states the constraint un-anchored — `[a-z0-9-]+` for a member whose modelled
        // pattern is `^[a-z0-9-]+$` — because Smithy's `@pattern` is an unanchored find
        // and the anchors are not part of the requirement a caller is told about. Only the
        // interpolated pattern changes: the envelope, the path, the echoed value and the
        // `ValidationException` code are the framework's, so a client that branches on the
        // code sees exactly what it saw before.
        match refusal {
            Refusal::Invalid(violations) => {
                let message = validation_envelope(violations)?;
                Some(Refused {
                    code: "ValidationException".to_string(),
                    message,
                })
            }
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

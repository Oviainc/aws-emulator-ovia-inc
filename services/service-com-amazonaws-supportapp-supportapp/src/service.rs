// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-supportapp-supportapp` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service names for a request it refused as invalid: the
/// modelled `ValidationException`, which every Support App operation declares with
/// `@httpError 400` ("Your request input doesn't meet the constraints that the Amazon
/// Web Services Support App specifies"). It is also what the framework's default
/// renderer answers, restated here because `override_refusal` replaces the code and
/// the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A violation as this service states it: the framework's own clause, with a
/// `@pattern` named WITHOUT the anchors the model brackets it with.
///
/// The Support App states the pattern a refused member must satisfy as the
/// expression alone — `[\w\- ]+` for `awsAccountAlias`'s `^[\w\- ]+$` — because the
/// clause already says the whole member must satisfy it, so the anchors would only
/// restate the sentence. Every other clause, and the value the clause echoes, is the
/// framework's unchanged.
fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            kind: ViolationKind::Pattern(unanchored(pattern).to_string()),
            ..violation.clone()
        },
        _ => violation.clone(),
    }
}

/// `pattern` without a leading `^` or a trailing `$`. A trailing `$` escaped by a
/// backslash is a literal dollar sign in the expression, not the end anchor, so it
/// stays — and so does a `$` whose backslash is itself escaped.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(stripped) => {
            let backslashes = stripped.len() - stripped.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                stripped
            } else {
                body
            }
        }
        None => body,
    }
}

/// The `com-amazonaws-supportapp-supportapp` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // The Support App answers a constraint violation in the standard validation
        // envelope the framework already builds — `N validation error(s) detected: `
        // over one clause per violation, each naming the rejected value and the
        // member path — and names a `@pattern` without the anchors the model spells
        // it with. The clauses therefore come from the framework's own renderer, so a
        // constraint class added upstream cannot arrive with a sentence this file
        // forgot to write; `as_stated` changes nothing but the pattern.
        //
        // The code stays the modelled `ValidationException` (HTTP 400), which is what
        // the framework would have answered: only the wording changes. An
        // authentication refusal keeps the framework's wording — it is not an
        // input-constraint failure, and this service models `AccessDeniedException`
        // for it only once a handler runs.
        match refusal {
            Refusal::Invalid(violations) => {
                let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
                Some(Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message: constraints::envelope(&stated)?,
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

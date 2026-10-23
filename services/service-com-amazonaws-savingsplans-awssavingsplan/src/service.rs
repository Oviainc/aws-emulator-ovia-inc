// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-savingsplans-awssavingsplan` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::envelope;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: its
/// own modelled `ValidationException`, which the model declares with
/// `@httpError 400`
/// (`smithy_query(query="shape",shape="ValidationException")`). It is also the
/// code the framework picks for a boundary refusal on restJson1, restated here
/// because `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The object this service roots a refused member's path at.
///
/// A savingsplans constraint-violation clause names the member path from the
/// REQUEST OBJECT inwards — `originalRequest.<member>` — not from the member
/// alone, so a caller parsing the clause to locate the offending member reads
/// that prefix. Recorded behaviour of the live service, which no model records
/// (fixture `savingsplans-tagging-error-paths-unknown-resource`: a malformed
/// `resourceArn` is refused with `… at 'originalRequest.resourceArn' …`) — and
/// exactly the kind of per-service wording `override_refusal` exists for.
const REQUEST_ROOT: &str = "originalRequest";

/// `violations` with every member path rooted at [`REQUEST_ROOT`].
///
/// ONLY the path is restated. The envelope, the requirement sentence and the
/// echoed value stay the framework's own (`Violation::message` plus
/// `aws_smithy_schema_server::envelope`), so this service cannot drift from the
/// others in anything but the one thing it words differently. In particular a
/// `@pattern` requirement keeps the model's expression VERBATIM, anchors and
/// all, which is how savingsplans quotes it — so this deliberately does not
/// delegate to `services/common`'s `aws_validation_refusal`, which quotes a
/// wrapping `^…$` pair unanchored for the services that do that.
fn rooted_at_request(violations: &[Violation]) -> Vec<Violation> {
    violations
        .iter()
        .map(|violation| Violation {
            path: if violation.path.is_empty() {
                REQUEST_ROOT.to_owned()
            } else {
                format!("{REQUEST_ROOT}.{}", violation.path)
            },
            kind: violation.kind.clone(),
            value: violation.value.clone(),
        })
        .collect()
}

/// The `com-amazonaws-savingsplans-awssavingsplan` emulated service: the generated [`ServiceDefinition`] plus
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
        // savingsplans states the member path rooted at the request object —
        // `originalRequest.<member>` — for EVERY constraint class and every
        // operation, because that root is a property of the service's validator
        // and not of one member. The code stays the modelled
        // `ValidationException` (HTTP 400), which is what the framework would
        // have answered, so only the path changes; a refusal that is not a
        // constraint violation (an unauthenticated caller) keeps the
        // framework's own code and wording.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_owned(),
                message: envelope(&rooted_at_request(violations))?,
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

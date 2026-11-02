// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-identitystore-awsidentitystore` service: how it claims requests and how it words the
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

use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// How this service words a request the boundary validator refused. A submodule rather
/// than a block in this file: it is the longest of the seams and it is driven by the
/// MODEL (the constraint a value failed, the shapes a member targets), while everything
/// else here is routing.
pub mod validation;

/// The wire error code of the one error class this service answers with a status its
/// model's `@httpError` does not state. The code itself is unchanged on the wire: it is
/// the STATUS beside it that this service decides here.
const NOT_FOUND_ERROR_CODE: &str = "ResourceNotFoundException";

/// The HTTP status this service answers its not-found class with.
///
/// This service's wire reports a resource it does not hold as a 4xx CLIENT fault with the
/// not-found code in the body, at the status its other client faults use, and not at the
/// separate 404 its published model's `@httpError` declares for the shape. A caller still
/// branches on the error code, which is what names the failure.
const NOT_FOUND_STATUS: u16 = 400;

/// The HTTP status this service answers a modelled error with: the status the model
/// declares for that error shape, except for the not-found class
/// ([`NOT_FOUND_ERROR_CODE`]), which this service serves at [`NOT_FOUND_STATUS`].
///
/// Keyed on the WIRE ERROR CODE, so it holds for every operation that raises the class
/// and for every resource kind it names — there is one status per error class, decided
/// once, rather than one per handler. `modeled` is what the error shape's own
/// `@httpError` states, and every other class keeps it.
pub fn error_status(error_code: &str, modeled: u16) -> u16 {
    if error_code == NOT_FOUND_ERROR_CODE {
        NOT_FOUND_STATUS
    } else {
        modeled
    }
}

/// The `com-amazonaws-identitystore-awsidentitystore` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service states a constraint violation in the AWS validation envelope
        // with its own three rules — the reported `@pattern` is the expression the
        // service applies rather than the model's trait text, a `@sensitive` member's
        // value is not echoed, and violations on the elements of a list member are
        // reported once at that member with its element shape's whole constraint set.
        // All three live in [`validation`].
        //
        // The STATUS and the CODE are already what the model declares for this
        // refusal (`ValidationException`, `@httpError(400)`); only the MESSAGE
        // differs. The code is restated unchanged alongside it, because `Refused`
        // carries both and a message without its matching code is worse than leaving
        // both alone.
        //
        // An authentication refusal is left to the framework: it is not a
        // modelled-input failure and this service does not restate it.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        Some(Refused {
            code: "ValidationException".to_string(),
            message: validation::message(violations)?,
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

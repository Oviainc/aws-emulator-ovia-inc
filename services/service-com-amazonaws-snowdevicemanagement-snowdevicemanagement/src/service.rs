// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-snowdevicemanagement-snowdevicemanagement` service: how it claims requests and how it words the
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

/// The input member whose emptiness Snow Device Management refuses at the front
/// door rather than as a request-shape problem: `CreateTaskInput$targets`, the
/// devices the call is authorized against (`@required`, `@length(min: 1, max:
/// 10)`).
const TARGETS: &str = "targets";

/// The error code the service answers a `CreateTask` that names no target with.
///
/// It is NOT one of this service's modelled shapes — recorded behaviour shows the
/// front-door denial, not a modelled refusal: a `CreateTask` carrying no target
/// has no device for the call to be authorized against, so it never reaches the
/// service's own shape validation. A targets list the model's MAXIMUM refuses is a
/// different class and keeps the framework's `ValidationException`.
const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The message that denial carries.
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// Whether `violation` is "this `CreateTask` names no target device".
///
/// Both forms of that are the same claim from the caller's side: a `targets` list
/// below `@length`'s minimum of 1 is an empty list, and an absent `@required`
/// `targets` is no list at all. Neither names a device for the call to be
/// authorized against. A violation DEEPER than the member itself
/// (`targets.1.member`, an element that fails `ManagedDeviceId`'s own bounds) is a
/// request-shape problem and is left to the framework's `ValidationException`.
fn names_no_target(violation: &Violation) -> bool {
    violation.path == TARGETS
        && matches!(
            violation.kind,
            ViolationKind::MinLength(_) | ViolationKind::MissingRequiredMember
        )
}

/// The `com-amazonaws-snowdevicemanagement-snowdevicemanagement` emulated service: the generated [`ServiceDefinition`] plus
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
        // Snow Device Management does not answer a `CreateTask` that names no
        // target device with a shape refusal: with no target there is nothing for
        // the call to be authorized against, so the front door denies it
        // (`UnauthorizedException` / "Unauthorized") ahead of any constraint
        // check. Every other refusal — including a targets list past the modelled
        // maximum, which IS a request-shape problem — keeps the framework's
        // `ValidationException` and its wording, which names the member, the value
        // and the constraint the way this service's validation messages do.
        //
        // The HTTP status stays the protocol's (400 for restJson1): this seam
        // chooses the code and the message a client branches on, and the framework
        // owns the status of a pre-dispatch refusal.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        if !violations.iter().any(names_no_target) {
            return None;
        }
        Some(Refused {
            code: UNAUTHORIZED_CODE.to_string(),
            message: UNAUTHORIZED_MESSAGE.to_string(),
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

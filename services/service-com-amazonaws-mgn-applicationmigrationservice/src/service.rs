// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mgn-applicationmigrationservice` service: how it claims requests and how it words the
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
//!   before any handler ran. THIS SERVICE REWORDS IT: a request body whose member
//!   violates a modelled VALUE constraint is MGN's front-end `BadRequestException`,
//!   not the modelled `ValidationException`. See the inline note on that method.
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

/// The error code MGN's REST front end answers a request body that fails the model's
/// member value constraints with.
///
/// `BadRequestException` is NOT a shape in this service's model
/// (`smithy_query(query="shape",shape="BadRequestException")` finds nothing): it is the
/// code the REST front end in front of every MGN operation emits for a body that fails
/// model validation, before any operation's own modelled errors are reachable. It is
/// discriminating — a client branches on it to tell "this request is malformed" from the
/// modelled 400s an operation raises itself (`ValidationException` for a rule the handler
/// checks, `UninitializedAccountException` for account state) — so the refusal the
/// framework makes ahead of the handler must carry it rather than `ValidationException`.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The message that front end states a body-validation failure with.
///
/// The front end validates the body against the model and reports no member path and no
/// submitted value, so the sentence is a constant rather than a rendering of the
/// violations: a caller is told the body is invalid, not which member failed. (The
/// requirement this pins is the 400 status and the `BadRequestException` code; this
/// wording is the one the recordings carry.)
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// Whether `violation` is a VALUE that failed a constraint the model declares on its
/// member, as opposed to a member that was absent altogether.
///
/// Those are two different refusals: a value outside `@length` / `@pattern` / `@range` /
/// an enum's value set / `@uniqueItems` is a body the front end's model validation
/// rejects, whereas an absent `@required` member keeps the framework's modelled
/// `ValidationException` wording — nothing recorded says MGN answers absence with the
/// front-end code, so it is left alone.
///
/// [`ViolationKind`] is `#[non_exhaustive]`, so a class added upstream answers `false`
/// and keeps the framework's wording rather than being swept into the front-end code.
fn is_value_constraint(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinLength(_)
            | ViolationKind::MaxLength(_)
            | ViolationKind::MinValue(_)
            | ViolationKind::MaxValue(_)
            | ViolationKind::Pattern(_)
            | ViolationKind::EnumValue(_)
            | ViolationKind::UniqueItems
    )
}

/// The `com-amazonaws-mgn-applicationmigrationservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // Every MGN operation sits behind a REST front end that validates the request
        // body against the model before the operation is reached: a member whose value
        // violates a modelled constraint — at any depth, in any operation — is refused
        // there with HTTP 400 and the code `BAD_REQUEST_EXCEPTION`, which is not one of
        // the operation's modelled errors. Both the code and the message are replaced
        // together, because that code is what a client branches on to separate a
        // malformed body from the modelled 400s a handler raises; the status (400)
        // belongs to the protocol and is the same either way.
        //
        // Everything else keeps the framework's wording: an absent `@required` member
        // (see [`is_value_constraint`]) and an authentication refusal, which is not an
        // input-constraint failure at all.
        match refusal {
            Refusal::Invalid(violations)
                if !violations.is_empty() && violations.iter().all(is_value_constraint) =>
            {
                Some(Refused {
                    code: BAD_REQUEST_EXCEPTION.to_string(),
                    message: INVALID_REQUEST_BODY.to_string(),
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

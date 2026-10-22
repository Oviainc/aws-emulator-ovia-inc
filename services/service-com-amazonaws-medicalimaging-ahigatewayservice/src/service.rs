// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-medicalimaging-ahigatewayservice` service: how it claims requests and how it words the
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

/// The error code this service answers an input-constraint violation with: the modeled
/// `ValidationException`, which every operation that can be refused declares with
/// `@httpError 400`. It is also the framework's own default code for a refusal, restated
/// here because `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code this service answers a malformed resource ARN with: its own
/// modelled `BadRequestException` — "The request is invalid or malformed.",
/// `@error("client")`, `@httpError(400)`
/// (`smithy_query(query="shape",shape="BadRequestException")`). Restated here
/// because `override_refusal` replaces the code and the message TOGETHER.
pub(crate) const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// What this service says when a resource ARN does not parse.
///
/// A CONSTANT, interpolating no part of the request: the ARN parser reports that the
/// label was not an ARN, and nothing about which member or which rule it failed.
/// Shared with the tagging handlers, so the refusal reads the same whether the
/// boundary validator or the handler's own parse found it.
pub(crate) const MALFORMED_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// Whether `violation` is a value that failed the `@pattern` of this service's `Arn`
/// shape — i.e. a resource ARN that is not an ARN at all.
///
/// Keyed on the pattern the value failed ([`crate::state::ARN_PATTERN`], the `Arn`
/// shape's own), not on a member name or an operation: every member that targets
/// `Arn` is a `resourceArn` label, and every other ARN-shaped member of this service
/// targets a different shape with a different pattern, so none of them is caught
/// here.
fn is_malformed_resource_arn(violation: &Violation) -> bool {
    matches!(
        &violation.kind,
        ViolationKind::Pattern(pattern) if pattern == crate::state::ARN_PATTERN
    )
}

/// The `com-amazonaws-medicalimaging-ahigatewayservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // AWS HealthImaging PARSES a resource ARN before it validates anything
        // else, so a `resourceArn` that is not an ARN is a malformed REQUEST —
        // answered with the modelled `BadRequestException` (HTTP 400) — and not a
        // member that failed a constraint, which is what `ValidationException`
        // reports. The status is 400 either way; the code is what a client
        // branches on, so both it and the message are replaced together.
        //
        // `any`, not "every violation": the ARN parse runs first, so it decides
        // the answer even when the same request also breaks another rule.
        //
        // Every OTHER refusal of a value the model constrains keeps the modelled
        // `ValidationException` code — restated unchanged, since it is also the
        // framework's default — and only its message changes: this service quotes a
        // `@pattern` without the model's anchors, and withholds the value of a
        // member carrying a caller-chosen name, an encryption-key reference or a
        // DICOM attribute. The one place that rule lives is
        // `crate::state::validation_envelope`, which the handlers that refuse a
        // request themselves call too.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) if violations.iter().any(is_malformed_resource_arn) => {
                Some(Refused {
                    code: BAD_REQUEST_EXCEPTION.to_string(),
                    message: MALFORMED_ARN_MESSAGE.to_string(),
                })
            }
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: crate::state::validation_envelope(violations)?,
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

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-pi-performanceinsightsv20180227` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::constraints::Violation;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The wire error code (`__type`) a request refused by the protocol validation
/// stage carries — the stage that runs BEFORE any handler, so none of the three
/// errors the pi model declares on its operations
/// (`InternalServiceError`/`InvalidArgumentException`/`NotAuthorizedException`)
/// is reachable from it. Recorded behaviour: Performance Insights answers a
/// violation of a modelled input constraint with `ValidationException`, a shape
/// its own model does not declare precisely because the validation framework,
/// not the service, emits it.
pub const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// How this service words a request whose START time does not precede its END
/// time — a cross-member ordering rule no single-member `@range`, `@length` or
/// `@pattern` can express, so the validation stage passes it and the handler
/// refuses it with the modelled `InvalidArgumentException`.
///
/// ONE constant for the whole service: the sentence names no member, no
/// operation and nothing from the request, and Performance Insights was recorded
/// answering this same condition with it on two unrelated operations
/// (`GetResourceMetrics` and `CreatePerformanceAnalysisReport`). Every handler
/// that enforces the ordering reads it from here so a second wording cannot
/// appear beside it.
pub const TIME_RANGE_ORDERING_MESSAGE: &str =
    "Specify a start time that is before or equal to the end time.";

/// The message the validation stage states for `violations`.
///
/// The template names the MEMBER and nothing else: the offending value is not
/// echoed and the Smithy constraint that rejected it is not quoted. That is what
/// Performance Insights was recorded doing for a `@range` violation on an
/// integer, a `@length` violation on a list and a `@pattern` violation on a
/// string alike — three constraint kinds, one sentence — so the template is a
/// property of the stage rather than of the constraint. Suppressing the value
/// also means a caller's own input never comes back out of the service in an
/// error a client may log.
///
/// Several violations are stated as several clauses in one envelope, in the order
/// the validating walk found them; only the SINGLE-violation form is recorded, so
/// the per-member clause is AWS's and both the join and the clause order are this
/// emulator's choice — a future recording of a multi-violation refusal is what
/// would settle them, and this is the one place to adjust.
fn validation_stage_message(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations
        .iter()
        .map(|violation| format!("Value at '{}' failed to satisfy constraints", violation.path))
        .collect();
    format!("Validation error(s) detected: {}.", clauses.join("; "))
}

/// The `com-amazonaws-pi-performanceinsightsv20180227` emulated service: the generated [`ServiceDefinition`] plus
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
        // A request that breaks a rule the MODEL declares — an absent required
        // member, or a value outside a `length`, `range`, `pattern`, enum value
        // set or `uniqueItems` — is refused by the validation stage of the
        // request walk, before this service's handler runs. Performance Insights
        // answers that stage's refusals with `ValidationException` and the
        // framework template, NOT with the `InvalidArgumentException` its model
        // declares: that error belongs to the handler, which such a request never
        // reaches. The code and the message travel together, so both are stated
        // here. An authentication refusal keeps the framework's wording: the
        // service does not word it.
        match refusal {
            Refusal::Invalid(violations) if !violations.is_empty() => Some(Refused {
                code: VALIDATION_EXCEPTION_CODE.to_string(),
                message: validation_stage_message(violations),
            }),
            // Nothing to state a clause about, and no authentication wording of
            // our own: keep the framework's.
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

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-outposts-outpostsolafservice` service: how it claims requests and how it words the
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

/// The dotted member path every tagging operation's resource ARN is refused
/// under: `ListTagsForResource`, `TagResource` and `UntagResource` all bind the
/// modelled `Arn` member `ResourceArn`, and a refusal names it in lowerCamel.
const RESOURCE_ARN_PATH: &str = "resourceArn";

/// The error code this service names an ARN-syntax refusal with.
///
/// A resource ARN that is not an Outposts resource ARN is refused by the tagging
/// API itself rather than by the generic parameter validator, so it carries this
/// code and not `ValidationException` — which is what a client branches on.
const RESOURCE_ARN_SYNTAX_CODE: &str = "BadRequestException";

/// How this service words that refusal: the ARN could not be read, with no echo
/// of what was sent.
const RESOURCE_ARN_SYNTAX_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// Whether `violations` include the tagging API's resource ARN failing to be an
/// Outposts resource ARN — its modelled `@pattern`, or its modelled maximum length,
/// which an ARN too long to be one exceeds.
///
/// ANY such violation is enough: the tagging operations read that ARN before they
/// report parameter validation, so an unreadable ARN is what the caller is told
/// about even when something else in the request is also wrong. Every refusal that
/// does NOT involve the ARN's syntax (a missing member, a bad tag map, a bound on
/// another member) keeps the framework's generic wording.
fn is_resource_arn_syntax(violations: &[Violation]) -> bool {
    violations.iter().any(|violation| {
        violation.path == RESOURCE_ARN_PATH
            && matches!(
                violation.kind,
                ViolationKind::Pattern(_) | ViolationKind::MaxLength(_)
            )
    })
}

/// The `com-amazonaws-outposts-outpostsolafservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // TWO classes are reworded, the more specific one first, and each replaces
        // BOTH the code and the message:
        //
        // 1. A resource ARN that is not an Outposts resource ARN. The tagging
        //    operations parse that ARN themselves before any parameter validation
        //    is reported, so they answer an ARN-syntax error rather than a
        //    validation report — and a client branching on the error code has to
        //    see the code the service names.
        // 2. Everything else the boundary validator found: a `ValidationException`
        //    body is the standard AWS validation report — the violation count,
        //    then one `Value at '<member>' failed to satisfy constraint:
        //    <requirement>` clause per violated constraint — and it NEVER renders
        //    the offending value. The framework's default interpolates that value,
        //    which this API's own recordings contradict, and which would echo back
        //    a member the model marks `@sensitive` (`CreateQuote$Description`,
        //    say). Its code is the one the modelled `ValidationException` (HTTP
        //    400) already carries, so that rewording never moves a client off the
        //    code it branches on.
        //
        // Authentication refusals keep the framework's wording: they are neither.
        match refusal {
            Refusal::Invalid(violations) if is_resource_arn_syntax(violations) => Some(Refused {
                code: RESOURCE_ARN_SYNTAX_CODE.to_string(),
                message: RESOURCE_ARN_SYNTAX_MESSAGE.to_string(),
            }),
            Refusal::Invalid(violations) if !violations.is_empty() => Some(Refused {
                code: crate::state::VALIDATION_EXCEPTION.to_string(),
                message: crate::state::refusal_report(violations),
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

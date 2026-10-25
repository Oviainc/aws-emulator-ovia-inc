// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ecs-amazonec2containerservicev20141113` service: how it claims requests and how it words the
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

/// The error this service answers a request refused before any handler ran with: "The
/// specified parameter isn't valid. Review the available parameters for the API
/// request", the one such failure this service's model declares. Every ECS operation
/// that takes parameters declares it, and none declares a validation error shape
/// besides it.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The member a describe names the optional fields it wants back in.
const INCLUDE_MEMBER: &str = "include";

/// What Amazon ECS says when a CLUSTER read asks for a field that is not one of the
/// fields a cluster can be described with.
const INVALID_CLUSTER_IDENTIFIER: &str = "The cluster identifier is invalid";

/// Whether `violation` is a value of a cluster read's `include` list that is not one of
/// the fields a cluster can be described with.
///
/// The refused element names the `include` member it came from, and the values it was
/// checked against are the cluster fields ([`crate::state::CLUSTER_FIELDS`]) — which is
/// what distinguishes it from the `include` of a read of some other resource, whose
/// member accepts that resource's own fields.
fn is_unknown_cluster_field(violation: &Violation) -> bool {
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return false;
    };
    violation
        .path
        .split('.')
        .next()
        .is_some_and(|member| member == INCLUDE_MEMBER)
        && allowed.len() == crate::state::CLUSTER_FIELDS.len()
        && allowed
            .iter()
            .all(|value| crate::state::CLUSTER_FIELDS.contains(&value.as_str()))
}

/// What the modelled refusal says when the walk reported no clause to state: the
/// `InvalidParameterException` doc's own sentence. A refusal of this service carries a
/// modelled code whatever the walk had to say about it, so this is the message of last
/// resort rather than a reason to leave the refusal unworded.
const INVALID_PARAMETER_MESSAGE: &str =
    "The specified parameter isn't valid. Review the available parameters for the API \
     request.";

/// How this service words the refusal of a request whose values failed the rules the
/// model declares.
///
/// A cluster read that asks for an unknown `include` field is the one such refusal
/// Amazon ECS states in its own sentence ([`INVALID_CLUSTER_IDENTIFIER`]); every other
/// one keeps the framework's clause, which states the value, the position and the
/// requirement it failed, falling back to [`INVALID_PARAMETER_MESSAGE`] when there is no
/// clause to state.
fn refusal_message(violations: &[Violation]) -> String {
    if violations.iter().any(is_unknown_cluster_field) {
        return INVALID_CLUSTER_IDENTIFIER.to_string();
    }
    ::aws_smithy_schema_server::envelope(violations)
        .unwrap_or_else(|| INVALID_PARAMETER_MESSAGE.to_string())
}

/// The `com-amazonaws-ecs-amazonec2containerservicev20141113` emulated service: the generated [`ServiceDefinition`] plus
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
        // Amazon ECS answers such a refusal with an error it MODELS: a client
        // branches on the error type, and this service declares no validation
        // error shape at all, so the code is the modeled
        // `InvalidParameterException` (HTTP 400) rather than a shape no operation
        // of this service can return. The wording comes from [`refusal_message`].
        //
        // An authentication refusal keeps the framework's wording: it is not a
        // request-parameter failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: INVALID_PARAMETER_EXCEPTION.to_string(),
                message: refusal_message(violations),
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

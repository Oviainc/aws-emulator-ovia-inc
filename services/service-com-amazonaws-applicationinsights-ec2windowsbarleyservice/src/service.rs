// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-applicationinsights-ec2windowsbarleyservice` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Every INPUT shape of this service, as the generated schema statics.
///
/// What they are read for: a constraint this service's model declares on the
/// ELEMENTS of a collection member is reported against that MEMBER, carrying the
/// element shape's WHOLE declared constraint set — and that set is a property of
/// the model, not of the request, because a refusal carries only the constraints a
/// value actually broke. Codegen attaches a collection member's element
/// constraints to that member's own schema, so the sets are read off these shapes.
///
/// ALL of them, not the ones one refusal happens to name: the rule holds for every
/// constrained collection of every operation. `grep -h 'const INPUT_SCHEMA'
/// src/ops/*.rs` names exactly these statics; one missing from this list is not a
/// wrong clause, only a clause stating the constraints that failed rather than the
/// element shape's whole set.
const INPUT_SCHEMAS: &[&::aws_smithy_schema::Schema] = &[
    crate::ops::types::ADD_WORKLOAD_INPUT_SCHEMA,
    crate::ops::types::CREATE_APPLICATION_INPUT_SCHEMA,
    crate::ops::types::CREATE_COMPONENT_INPUT_SCHEMA,
    crate::ops::types::CREATE_LOG_PATTERN_INPUT_SCHEMA,
    crate::ops::types::DELETE_APPLICATION_INPUT_SCHEMA,
    crate::ops::types::DELETE_COMPONENT_INPUT_SCHEMA,
    crate::ops::types::DELETE_LOG_PATTERN_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_APPLICATION_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_COMPONENT_CONFIGURATION_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_COMPONENT_CONFIGURATION_RECOMMENDATION_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_COMPONENT_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_LOG_PATTERN_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_OBSERVATION_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_PROBLEM_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_PROBLEM_OBSERVATIONS_INPUT_SCHEMA,
    crate::ops::types::DESCRIBE_WORKLOAD_INPUT_SCHEMA,
    crate::ops::types::LIST_APPLICATIONS_INPUT_SCHEMA,
    crate::ops::types::LIST_COMPONENTS_INPUT_SCHEMA,
    crate::ops::types::LIST_CONFIGURATION_HISTORY_INPUT_SCHEMA,
    crate::ops::types::LIST_LOG_PATTERNS_INPUT_SCHEMA,
    crate::ops::types::LIST_LOG_PATTERN_SETS_INPUT_SCHEMA,
    crate::ops::types::LIST_PROBLEMS_INPUT_SCHEMA,
    crate::ops::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
    crate::ops::types::LIST_WORKLOADS_INPUT_SCHEMA,
    crate::ops::types::REMOVE_WORKLOAD_INPUT_SCHEMA,
    crate::ops::types::TAG_RESOURCE_INPUT_SCHEMA,
    crate::ops::types::UNTAG_RESOURCE_INPUT_SCHEMA,
    crate::ops::types::UPDATE_APPLICATION_INPUT_SCHEMA,
    crate::ops::types::UPDATE_COMPONENT_CONFIGURATION_INPUT_SCHEMA,
    crate::ops::types::UPDATE_COMPONENT_INPUT_SCHEMA,
    crate::ops::types::UPDATE_LOG_PATTERN_INPUT_SCHEMA,
    crate::ops::types::UPDATE_PROBLEM_INPUT_SCHEMA,
    crate::ops::types::UPDATE_WORKLOAD_INPUT_SCHEMA,
];

/// The `com-amazonaws-applicationinsights-ec2windowsbarleyservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        //
        // This endpoint reports a constraint its MODEL declares on the ELEMENTS of a
        // collection member against that MEMBER: no element position, no `member`
        // step and no element value, with every requirement the element shape
        // declares nested inside `Member must satisfy constraint: [ … ]` — so a
        // caller is told what every element of that member has to satisfy rather
        // than which one element offended, and the rejected value is not read back
        // to it. The framework reports it at the offending element instead, which is
        // the only difference: every other clause, the envelope, the member paths and
        // the error code stay the framework's, and `ValidationException` is also the
        // code this awsJson1_1 service already answers a constraint refusal with.
        //
        // An authentication refusal is a different failure with its own code and
        // status, and nothing about it is this service's to word.
        ::service_common::validation_message::aws_validation_refusal_over_elements(
            refusal,
            INPUT_SCHEMAS,
            // This endpoint quotes a `@pattern` requirement as the MODEL declares it,
            // anchors and all — the framework's own rendering of the expression.
            str::to_string,
        )
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

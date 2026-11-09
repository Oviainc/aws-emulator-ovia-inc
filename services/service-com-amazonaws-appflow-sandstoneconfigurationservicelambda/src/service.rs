// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appflow-sandstoneconfigurationservicelambda` service: how it claims requests and how it words the
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
//! - `override_refusal` — EDITED HERE. A request refused for violating a modelled
//!   constraint is stated the way AppFlow states it: the framework's own envelope and
//!   clause, with a `@pattern` requirement quoted as the expression the SERVICE validates
//!   against. See "How this service words a refusal" below.
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
//!
//! # How this service words a refusal
//!
//! The envelope, the clause and the requirement are the framework's — `N validation
//! error(s) detected: Value '<submitted>' at '<member path>' failed to satisfy constraint:
//! <requirement>` — reached through the one shared statement of that wording
//! ([`::service_common::validation_message::aws_validation_refusal_with`]), so a
//! constraint class the framework gains later cannot arrive with a sentence of this
//! service's own. The clause QUOTES BACK the value the request submitted, which is the
//! only part of the message built from the request rather than chosen.
//!
//! One part of the clause is AppFlow's: a `@pattern` requirement is quoted as the
//! expression the SERVICE validates a value against ([`stated_pattern`]) rather than as
//! the model spells it — without the `^`/`$` anchors Smithy writes to turn a find into a
//! whole-string match, and, for a member whose service-side expression admits a narrower
//! character class than the published model's, as the service's own expression
//! ([`NAMED_BY_THE_SERVICE`]). Both are properties of a SHAPE, so they hold for every
//! member that reaches it and every operation that takes one, and neither depends on the
//! request. The CHECK is unaffected: it is still the model's pattern as written, so only
//! the quoting changes.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `@pattern` expressions this service VALIDATES with that its published model spells
/// differently, as (what the model declares, what the service names) pairs.
///
/// The declared expression is what identifies the shape here: the walk hands this seam a
/// pattern violation's EXPRESSION, not the shape that declared it, and the pair below is
/// the one expression of this service's model whose recorded refusals quote a different
/// character class — `FlowName`, which AppFlow validates without the `#` the published
/// model admits (`smithy_query(query="shape", shape="FlowName")`). Every other pattern is
/// quoted as the model declares it, minus its anchors.
///
/// A pair is a property of the SHAPE rather than of one request, so it is the same for
/// every member that reaches it (`CreateFlow`, `UpdateFlow`, `StartFlow`, `StopFlow`,
/// `DeleteFlow`, `DescribeFlow`, … all take a `FlowName`) and for every operation that
/// takes one.
const NAMED_BY_THE_SERVICE: [(&str, &str); 1] = [(
    r"^[a-zA-Z0-9][\w!@#.-]+$",
    r"[a-zA-Z0-9][\w!@.-]+",
)];

/// The expression a refusal quotes for the `@pattern` `declared`: the one the SERVICE
/// validates against.
///
/// That is [`NAMED_BY_THE_SERVICE`]'s for a shape whose service-side expression differs
/// from the published one, and otherwise the model's own expression without the anchors a
/// whole-value pattern wraps it in — the shared rendering
/// ([`::service_common::validation::unanchored`]), so this service cannot drop an anchor
/// differently from any other.
fn stated_pattern(declared: &str) -> String {
    for (model_names, service_names) in NAMED_BY_THE_SERVICE {
        if declared == model_names {
            return service_names.to_string();
        }
    }
    ::service_common::validation::unanchored(declared)
}

/// The `com-amazonaws-appflow-sandstoneconfigurationservicelambda` emulated service: the generated [`ServiceDefinition`] plus
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
        // reword it. AppFlow states such a refusal in the framework's own envelope and
        // clause — the submitted value quoted back included — with a `@pattern`
        // requirement quoted as the expression the service validates against; see "How
        // this service words a refusal" at the top of this file.
        ::service_common::validation_message::aws_validation_refusal_with(refusal, stated_pattern)
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

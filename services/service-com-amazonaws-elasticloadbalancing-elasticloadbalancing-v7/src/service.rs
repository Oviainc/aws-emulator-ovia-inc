// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7` service: how it claims requests and how it words the
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
//!
//! One of those seams is taken here, for ONE reason: this service's signing name is the
//! endpoint prefix it SHARES with the ELBv2 model, so neither the claim nor the operation
//! lookup can come from the model alone. [`api_version`] carries that whole rule — which
//! API version the request's awsQuery envelope names, which of the two models therefore
//! serves it, and what happens to an envelope naming a version neither declares.

pub mod api_version;

use api_version::{ServingModel, UnsupportedApiVersionOperation, V2FrontDoor};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The ELBv2 half of this shared endpoint, served for an envelope naming that
    /// model's API version. Held in a field, built once over the same shared engine,
    /// because `override_operation` hands back a reference borrowed from `&self`.
    elbv2: V2FrontDoor<Eng>,
    /// The refusal served for an envelope naming an API version NEITHER Elastic Load
    /// Balancing model declares. Held in a field for the same reason.
    unsupported_api_version: UnsupportedApiVersionOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            elbv2: V2FrontDoor::new(engine),
            unsupported_api_version: UnsupportedApiVersionOperation,
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
        // Claim a request whose resolved route key is this service's signing name,
        // `elasticloadbalancing`.
        //
        // That key is SHARED: the ELBv2 model
        // (`services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10`)
        // publishes the same `endpointPrefix`, `arnNamespace` and `aws.auth#sigv4` name,
        // and every AWS SDK for either API signs it — nothing in a request's headers or
        // URI tells the two apart, so the key must resolve to exactly ONE service. THIS
        // service owns it: its own identity IS that name (`sdkId: Elastic Load
        // Balancing`, `docId: elasticloadbalancing-2012-06-01`), while the other model's
        // is `Elastic Load Balancing v2`, which claims its own `elasticloadbalancingv2`
        // key. Claiming the shared key is therefore claiming BOTH APIs' traffic: an
        // envelope naming the ELBv2 model's API version is served out of THAT model, not
        // out of this model's 2012-06-01 shapes (see [`api_version`]).
        //
        // Before this, both services claimed the key and the router answered every
        // Elastic Load Balancing request `500 AmbiguousService` — a code no AWS model
        // carries, in a JSON body an awsQuery client cannot parse. The claim table is
        // `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The ONE routing decision the model's key cannot make: an awsQuery envelope
        // names the API version it is for alongside the action, and the framework's
        // routing key reads the action alone — which matters here because TEN action
        // names are declared by this model AND by the ELBv2 model with different inputs
        // and outputs. The version therefore chooses the MODEL that serves the request:
        // this one, the ELBv2 one through its own front door, or neither. The whole rule
        // is in [`api_version`].
        match api_version::serving_model(req) {
            // This model's own API version (or no `Version` field at all): the generated
            // routing table resolves it, and an `Action` naming no modelled operation is
            // the framework's own `400 InvalidAction` in this service's envelope.
            ServingModel::Classic => None,
            // The ELBv2 API version: served out of the ELBv2 model, over the one shared
            // engine, which is how an AWS SDK ELBv2 client — signing the shared name and
            // naming that version — reaches those operations at all.
            ServingModel::V2 => Some(&self.elbv2),
            // A version no Elastic Load Balancing model declares: an action/version pair
            // nothing can resolve.
            ServingModel::Unresolvable => Some(&self.unsupported_api_version),
        }
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
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // Reached when this model served the request (see `override_operation`) and its
        // own routing table named no operation for the `Action`. That is the SAME
        // unresolvable action/version pair the ELBv2 front door and the unknown-version
        // refusal answer, so it is worded by the same renderer — `400 InvalidAction`
        // naming the action and the version it could not be found for — rather than the
        // framework's version-less default, which would word one refusal of this shared
        // endpoint differently from the other two.
        Some(api_version::unresolved_pair_response(
            req,
            api_version::CLASSIC_API_VERSION,
        ))
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

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-lambdamicrovms-lambdamicrovms` service: how it claims requests and how it words the
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

use service_common::shared_path::SharedPathArn;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The ONE first URI path segment this model and the Lambda control plane model BOTH
/// bind, and therefore the only place the framework's model-derived path discriminator
/// cannot separate them: `2017-03-31`, the API date of both models' tagging operations.
/// Every other first segment either model binds belongs to one of them alone
/// (`2025-09-09` here; `2015-03-31`, `2018-10-31`, … there). Source:
/// `smithy_query(query="digest", …)` on both models.
const SHARED_PATH_SEGMENTS: &[&str] = &["2017-03-31"];

/// The literal first URI path segments that LEAD to the `{Resource}` ARN label of the
/// one URI this model and the Lambda control plane bind IDENTICALLY: `ListTags` (`GET`),
/// `TagResource` (`POST`) and `UntagResource` (`DELETE`) are all bound to
/// `/2017-03-31/tags/{Resource}` here AND there. Source:
/// `smithy_query(query="digest", …)` on both models.
const SHARED_TAGGING_PATH: &[&str] = &["2017-03-31", "tags"];

/// The `aws.api#service$arnNamespace` every service signing as `lambda` shares
/// (`smithy_query(query="service_info")`: `arnNamespace: lambda` for all four), and so
/// the only service field an ARN any of their tagging operations addresses can carry.
const SHARED_ARN_NAMESPACE: &str = "lambda";

/// The ARN resource types THIS service owns — the ones its own operations mint, and
/// therefore the only ones its tagging operations can address.
///
/// `CreateMicrovmImage` mints a MicroVM image, and `microvm-image` is the resource
/// segment this model's `TaggableResource` pattern accepts for it (the Lambda control
/// plane's own pattern does not). Every other type this model's pattern accepts —
/// `function`, `layer`, `code-signing-config`, `event-source-mapping`,
/// `capacity-provider`, `network-connector` — is a resource another `lambda`-signing
/// service creates, so a tagging request naming one of those is not this service's.
/// Source: `smithy_query(query="shape", shape="TaggableResource")` on both models, and
/// this model's `CreateMicrovmImage` / `GetMicrovm` `MicrovmImageArn` pattern
/// (`^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$`).
const OWN_TAGGING_RESOURCE_TYPES: &[&str] = &["microvm-image"];

/// Whether this service claims a request on the first path segment it shares with the
/// Lambda control plane.
///
/// Off that segment, the modelled path spaces have already decided the claim and this
/// discriminator adds nothing. On it, the only URI either model binds is
/// `/2017-03-31/tags/{Resource}`, where the ARN names the resource and a resource belongs
/// to exactly one of the two endpoints: this service claims such a request only when the
/// ARN names one of ITS resources. Everything else on that segment — another endpoint's
/// resource, a label that names no recognisable `lambda` resource, a path neither model
/// binds — is left to the control-plane endpoint, whose own identity IS the shared
/// signing name, so exactly one service claims every request on it.
fn claims_shared_path_space(parts: &http::request::Parts) -> bool {
    if !service_common::claims_modeled_path(parts, SHARED_PATH_SEGMENTS) {
        return true;
    }
    matches!(
        service_common::shared_path::shared_path_arn(
            parts.uri.path(),
            SHARED_TAGGING_PATH,
            SHARED_ARN_NAMESPACE,
        ),
        SharedPathArn::Resource(kind) if OWN_TAGGING_RESOURCE_TYPES.contains(&kind.as_str())
    )
}

/// The `com-amazonaws-lambdamicrovms-lambdamicrovms` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `lambda` — a signing name it SHARES with the Lambda
        // control plane, Lambda Core and Lambda Web (`smithy_query(
        // query="service_info")` gives all four `sigv4Name: lambda`). The router
        // resolves a request's routing key from the SigV4 credential scope first, so all
        // four see the key `lambda`: claiming on the signing name alone makes each one
        // claim the others' traffic, which the router answers `500 AmbiguousService`
        // instead of serving any of them.
        //
        // So the claim is narrowed to this service's own request space, by the one rule
        // `service-common` states for a shared signing name: this model is `restJson1`
        // (a REST protocol, so the `rpc_service_shape_id` argument is `None`), and a
        // REST service's own surface is the modelled URI path space its generated
        // `ROUTE_PATH_SEGMENTS` publishes — here `2025-09-09` (its MicroVM and image
        // operations) and `2017-03-31` (its tagging operations). That second segment is
        // the one the Lambda control plane also binds, so there the ARN in the path
        // decides (see [`claims_shared_path_space`]).
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        ) && claims_shared_path_space(parts)
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
        let _ = refusal;
        None
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

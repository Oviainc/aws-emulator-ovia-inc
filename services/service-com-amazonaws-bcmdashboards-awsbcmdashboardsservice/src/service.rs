// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bcmdashboards-awsbcmdashboardsservice` service: how it claims requests and how it words the
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
//! - `override_protocol` — the codec this service's own responses are serialized in.
//!   The default returns `None`, keeping the model-derived protocol; this service
//!   overrides it (see below).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! Four of those seams are taken here, all for the one reason named in
//! [`response_media_type`]: every response this service answers announces the media type
//! the real service stamps (`application/x-amz-json-1.1`) rather than the one the model's
//! `awsJson1_0` protocol trait implies.
//!
//! - `override_protocol` serves the awsJson `1.1` codec, which is the one place the media
//!   type of a response this SERVICE produces — a success, and any error its handler
//!   raises — is decided. The JSON framing is byte-identical to `1.0`, so the request side
//!   is unchanged.
//! - `override_operation`, `override_routing_error` and `override_framework_error`
//!   re-announce the responses the FRAMEWORK renders in front of the service, which
//!   `override_protocol` does not reach: a refusal of a modelled constraint, an unroutable
//!   request, and a transport-level failure.
//!
//! Routing, status, codes, messages and bodies all stay with the framework.

pub mod response_media_type;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-bcmdashboards-awsbcmdashboardsservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The codec this service answers in (awsJson `1.1`; see [`response_media_type`]).
    /// Built once here because [`Service::override_protocol`] hands back a borrow of it.
    protocol: aws_smithy_schema_server::rpc::HttpRpcServerProtocol<
        aws_smithy_schema_server::json::JsonCodec,
    >,
    /// The wrapper every routable request is served through, which re-announces the one
    /// response `override_protocol` cannot reach. Held in a field, built once, because
    /// `override_operation` hands back a reference to it.
    media_type: response_media_type::ResponseMediaTypeOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: aws_smithy_schema_server::rpc::AwsJson11(),
            media_type: response_media_type::ResponseMediaTypeOperation,
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
        // Every request the generated routing table can serve is served through the
        // media-type wrapper, which delegates to that same table entry and then
        // re-announces whatever came back (see [`response_media_type`]). It is there for
        // the ONE response `override_protocol` below cannot reach: the refusal of a
        // modelled constraint, rendered inside the centralized walk. The routing stays the
        // model's: a request the table cannot serve is declined here, so the framework
        // answers it as the unroutable request it is — through `override_routing_error`.
        response_media_type::routes(&self.entry, req).then_some(&self.media_type as _)
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The codec the centralized walk serializes this service's own responses with —
        // every success and every error a handler raises. This is the one place the media
        // type such a response announces is decided, and `AwsJson11()` is the same
        // `JsonCodec` over the same `X-Amz-Target` routing key and the same empty body as
        // the `awsJson1_0` the model declares, so only the announced media type changes.
        Some(&self.protocol)
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
        // routing table resolved it. The framework's protocol-correct default is kept word
        // for word, status, code and body alike; only the media type is re-announced, so an
        // unroutable request is answered under the same media type as a routed one (see
        // [`response_media_type`]).
        Some(response_media_type::with_response_media_type(
            self.entry.routing_error_response(req),
        ))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. The entry's protocol-correct rendering is kept word for
        // word; only the media type is re-announced (see [`response_media_type`]).
        //
        // The error is borrowed and `ProtocolError` is neither `Clone` nor reconstructable
        // in general, so the default renderer is reached by rebuilding the one variant this
        // seam is ever handed: the router raises `Malformed` here, from the `aws-chunked`
        // decode it runs before naming an operation. Anything else declines to `None` and
        // keeps the framework's rendering unchanged rather than losing what the error said.
        match error {
            aws_smithy_schema_server::ProtocolError::Malformed(message) => {
                let rebuilt =
                    aws_smithy_schema_server::ProtocolError::Malformed(message.clone());
                Some(response_media_type::with_response_media_type(
                    self.entry.framework_error_response(rebuilt),
                ))
            }
            _ => None,
        }
    }
}

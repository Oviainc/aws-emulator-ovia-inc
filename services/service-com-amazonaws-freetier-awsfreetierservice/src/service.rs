// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-freetier-awsfreetierservice` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself; this
//!   service returns the shared wrapper that serves the routing table's own
//!   operation and re-announces its answer's media type (see "The media type this
//!   service announces" below).
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …); this service keeps that rendering word for word and re-announces its
//!   media type.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering; this service keeps that rendering and re-announces its media type.
//! - `override_protocol` — the codec this service's own responses are serialized
//!   in. The default returns `None`, keeping the model-derived protocol; this
//!   service overrides it (see "The media type this service announces" below).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The media type this service announces
//!
//! The model declares the `awsJson1_0` protocol trait, but every recorded response
//! of this service — HTTP 200 and HTTP 400 alike — announces
//! `application/x-amz-json-1.1`, and the media type a client reads is what it keys
//! content negotiation and protocol assertion off. The rule, the one function that
//! applies it and the operation-seam wrapper that reaches the refusal tail all live
//! in [`service_common::response_media_type`];
//! [`RESPONSE_MEDIA_TYPE`](announced_media_type::RESPONSE_MEDIA_TYPE) is this
//! service's pair, and the three seams it is applied at are:
//!
//! - `override_protocol` serves the awsJson `1.1` codec, which is what the
//!   centralized walk serializes this service's own responses with — every success
//!   and every error a handler raises. The JSON framing is byte-identical to `1.0`,
//!   so a request sent with either media type still reads, and only the announced
//!   media type changes;
//! - `override_operation` hands back the shared
//!   [`ResponseMediaTypeOperation`](service_common::response_media_type::ResponseMediaTypeOperation),
//!   which serves the modelled operation the generated routing table names and
//!   re-announces what that walk answered — reaching the framework's refusal tail
//!   INSIDE the walk (an absent `@required` member, a value outside a declared
//!   bound), which `override_protocol` deliberately does not reach;
//! - `override_routing_error` and `override_framework_error` re-announce the other
//!   two framework-rendered responses, keeping their status, code and body word for
//!   word.
//!
//! One response is still announced by the framework alone, because no author seam
//! reaches it: the authentication refusal, which the router renders through
//! `definition().refusal_response(..)` before this service is asked anything
//! (`vendor/smithy-server-runtime/src/multi_service_router.rs`).

pub mod announced_media_type;

use announced_media_type::RESPONSE_MEDIA_TYPE;
use service_common::response_media_type::ResponseMediaTypeOperation;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// The `com-amazonaws-freetier-awsfreetierservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The codec this service answers in (awsJson `1.1`; see the module docs).
    /// Built once here because [`Service::override_protocol`] hands back a borrow
    /// of it.
    protocol: aws_smithy_schema_server::rpc::HttpRpcServerProtocol<
        aws_smithy_schema_server::json::JsonCodec,
    >,
    /// The wrapper every routable request is served through, so a refusal raised
    /// inside the request walk announces this service's media type too. Held in a
    /// field, built once, because [`Service::override_operation`] hands back a
    /// borrow of it.
    media_type: ResponseMediaTypeOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: aws_smithy_schema_server::rpc::AwsJson11(),
            media_type: ResponseMediaTypeOperation::new(RESPONSE_MEDIA_TYPE),
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
        // shared media-type wrapper, which delegates to that same table entry and
        // then re-announces the media type of what it answered — reaching the
        // refusal the framework renders inside the request walk, which
        // `override_protocol` does not (see the module header). The routing stays
        // the model's: a request whose routing key names no modelled operation is
        // declined here, so the framework answers it as the unroutable request it
        // is — through `override_routing_error` below, which re-announces that
        // answer too.
        service_common::response_media_type::routes(&self.entry, req)
            .then_some(&self.media_type as _)
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response this SERVICE produces — a success and a modelled failure
        // its handler raises — is serialized by the protocol returned here, so this
        // is the one place the media type it announces is decided. See the module
        // docs: every recorded response of this service carries the awsJson `1.1`
        // media type, which is what a client keys its parsing off. A refusal the
        // framework renders in front of a handler is not reached from here; the
        // three seams the module header names re-announce those.
        Some(&self.protocol)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. The framework's protocol-correct default for
        // this service's envelope is kept word for word, status, code and body
        // alike; only the media type is re-announced, so an unroutable request is
        // answered under the same media type as a routed one (see the module
        // header).
        Some(RESPONSE_MEDIA_TYPE.announce(self.entry.routing_error_response(req)))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. The entry's protocol-correct rendering is kept word
        // for word; only the media type is re-announced (see the module header).
        //
        // The error is borrowed and `ProtocolError` is neither `Clone` nor
        // reconstructable in general, so the default renderer is reached by
        // rebuilding the error. The match is TOTAL over what this seam is ever
        // handed, not a partial application of the rule: the router raises it from
        // exactly ONE call site, with exactly one variant —
        // `ProtocolError::Malformed(..)`, from the `aws-chunked` decode it runs
        // before naming an operation. `ProtocolError` is an open enum this crate
        // does not own, so the remaining arm exists for a variant a future
        // framework may route here: it declines to `None`, keeping the framework's
        // own rendering of an error this service cannot rebuild rather than losing
        // what that error said.
        match error {
            aws_smithy_schema_server::ProtocolError::Malformed(message) => {
                let rebuilt =
                    aws_smithy_schema_server::ProtocolError::Malformed(message.clone());
                Some(RESPONSE_MEDIA_TYPE.announce(self.entry.framework_error_response(rebuilt)))
            }
            _ => None,
        }
    }
}

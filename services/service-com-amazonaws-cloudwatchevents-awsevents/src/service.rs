// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudwatchevents-awsevents` service: how it claims requests and how it words the
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

/// The route key THIS service answers under, which is NOT the `events` key its model's
/// `aws.auth#sigv4` name resolves to.
///
/// `CloudWatch Events` is the LEGACY `sdkId` of the service now called `EventBridge`,
/// not a second endpoint beside it, and the two models publish one and the same wire
/// identity — service shape `AWSEvents`, version `2015-10-07`, protocol `awsJson1_1`
/// (hence `POST /` with the same `X-Amz-Target: AWSEvents.<Operation>`),
/// `awsService.endpointPrefix` `events` and `aws.auth#sigv4` name `events`:
/// `smithy_query(query="service_info")` on this model and on
/// `services/service-com-amazonaws-eventbridge-awsevents/smithy-model/com-amazonaws-eventbridge-awsevents.json`
/// differ only in `sdkId` (`CloudWatch Events` vs `EventBridge`). No signal in a request
/// can tell them apart, so the `events` key has to be served by exactly ONE of them, and
/// this model's 51 operations are a strict SUBSET of the EventBridge model's 57 (which
/// adds `CreateEndpoint`, `DeleteEndpoint`, `DescribeEndpoint`, `ListEndpoints`,
/// `UpdateEndpoint` and `UpdateEventBus`): the EventBridge service — the endpoint's
/// current name, and the superset — owns `events`, so a client that signs for it loses
/// no operation. Claiming `events` here as well would make the router answer
/// `500 AmbiguousService`, a code that exists in no AWS model, for every request to
/// either service.
///
/// This service therefore answers under the key the framework itself derives from an
/// `aws.api#service` sdkId when a model carries no sigv4 name (`emulator-core-build-support`'s
/// `read_service_info`: the sdkId lower-cased with its spaces removed), so the legacy
/// model stays mounted, reachable and exercised by its own suite without ever contending
/// for the `events` key.
const OWN_SDK_ID_KEY: &str = "cloudwatchevents";

/// The first URI path segment EVERY request to this service carries: the ROOT path.
///
/// This model is `awsJson1_1` and binds no `smithy.api#http` trait to any operation, so
/// every request is `POST /` with `X-Amz-Target: AWSEvents.<Operation>` (the crate's
/// `OPERATIONS.md`, "Wire protocol"). `emulator_server::ROOT_PATH_SEGMENT` is the
/// framework's sentinel for exactly that path; claiming it rather than the generated
/// EMPTY `ROUTE_PATH_SEGMENTS` (which claims every path under the key) keeps this
/// service out of any other service's path space.
const OWN_PATH_SEGMENTS: &[&str] = &[emulator_server::ROOT_PATH_SEGMENT];

/// Whether the request these `parts` came from resolves to [`OWN_SDK_ID_KEY`] AND the one
/// URI path this service's wire protocol publishes ([`OWN_PATH_SEGMENTS`]).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule, and it is CALLED here rather than re-implemented: the request's route
/// key must match the key passed AND its first URI path segment must be one of the
/// segments passed, by exact segment equality.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn claims_own_key_and_root_path(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        OWN_SDK_ID_KEY,
        OWN_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-cloudwatchevents-awsevents` emulated service: the generated [`ServiceDefinition`] plus
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
        // Delegated to the framework's own predicate — see
        // [`claims_own_key_and_root_path`]: this model is the LEGACY `CloudWatch Events`
        // alias of the service now called EventBridge, publishing the very same wire
        // identity, so the generated `events` route key cannot tell the two apart. It is
        // claimed by the EventBridge crate and this service answers under its OWN
        // sdkId-derived key instead, so exactly one service claims every request.
        claims_own_key_and_root_path(parts)
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

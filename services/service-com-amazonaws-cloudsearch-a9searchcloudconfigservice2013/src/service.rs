// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudsearch-a9searchcloudconfigservice2013` service: how it claims requests and how it words the
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

/// The first URI path segment EVERY request to this service carries: the ROOT path.
///
/// This model is `awsQuery`, so every operation is `POST /` with the operation named
/// by the `Action` body field and no `smithy.api#http` binding on the wire (the
/// crate's `OPERATIONS.md`, "Wire protocol"). `emulator_server::ROOT_PATH_SEGMENT` is
/// the framework's sentinel for exactly that path; claiming it rather than the
/// generated EMPTY `ROUTE_PATH_SEGMENTS` (which claims every path under the signing
/// name) keeps this service out of the CloudSearch Domain endpoint's path space.
const OWN_PATH_SEGMENTS: &[&str] = &[emulator_server::ROOT_PATH_SEGMENT];

/// Whether the request these `parts` came from resolves to `route_key` AND addresses
/// the one URI path this service's wire protocol publishes ([`OWN_PATH_SEGMENTS`]).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of that rule and is CALLED here rather than re-implemented: the request's
/// route key must match the key passed AND its first URI path segment must be one of
/// the segments passed, by exact segment equality.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed
/// only the request's parts, so the two things it reads — the URI and the headers —
/// are re-presented to it as a bodiless request. It reads neither the body nor the
/// method, so nothing it decides on is missing.
fn claims_own_key_and_root_path(parts: &http::request::Parts, route_key: &str) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(&request, route_key, OWN_PATH_SEGMENTS)
}

/// The `com-amazonaws-cloudsearch-a9searchcloudconfigservice2013` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service and the CloudSearch DOMAIN data plane share one SigV4
        // signing name (`cloudsearch`), so the signing name alone cannot tell
        // them apart and a router that matched on it would see two claimants and
        // answer 500 `AmbiguousService` — a code no AWS model declares — for
        // every request to either one. What separates them is the endpoint: this
        // model is `awsQuery`, so every operation is `POST /` with `Action` in
        // the body (the crate's `OPERATIONS.md`, "Wire protocol"), while the
        // domain endpoint serves only the dated data-plane paths
        // (`/2013-01-01/search`, `/2013-01-01/suggest`,
        // `/2013-01-01/documents/batch`).
        //
        // The rule is the framework's own — `emulator_server`'s
        // `can_handle_based_on_route_key_and_path`, the agent-immutable owner of
        // the "route key AND modelled path space" test — so it is CALLED rather
        // than restated here, exactly as the cloudwatchevents and IAM services
        // call it. (`service_common::claims_request` wraps this same helper; this
        // crate reaches the owner directly so the fix adds no dependency, and so
        // no `Cargo.lock` edit, which the merge gate's file-level squash cannot
        // land while mainline rewrites that file every few minutes.) The
        // generated `ROUTE_PATH_SEGMENTS` is EMPTY for a query protocol ("no
        // constraint", which would claim every path under the key), so the
        // segment list passed is the framework's own sentinel for the one path
        // this protocol publishes.
        claims_own_key_and_root_path(parts, self.entry.identity().signing_name())
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
        match refusal {
            // The framework refused a value against a rule the model declares (an
            // absent required member, a length or pattern violation on `DomainName`
            // or `IndexFieldName`). This service MODELS that refusal: every operation
            // that takes caller input lists `ValidationException` (HTTP 400), and the
            // shape carries no `awsQueryError` trait, so its wire code is the shape
            // name. The query family's default wording answers `ValidationError`
            // instead, which is a code no CloudSearch client branches on — so both the
            // code and the message are replaced here, the message keeping the
            // framework's own violation envelope so it still names the member and the
            // constraint it failed.
            Refusal::Invalid(violations) => Some(Refused {
                code: "ValidationException".to_string(),
                message: aws_smithy_schema_server::envelope(violations)
                    .unwrap_or_else(|| "invalid request".to_string()),
            }),
            // Authentication is the framework's answer to give, not this service's.
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

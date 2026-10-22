// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-s3-amazons3` service: how it claims requests and how it words the
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
//! Six opt-in seams are yours to change; every one defaults to the framework's
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
//! - `override_protocol` — the codec the modeled sequence reads and answers in.
//!   The default returns `None`, serving the model-derived protocol; THIS service
//!   returns [`wire::UnwrappedXmlRestXml`], the model's own restXml with the
//!   envelope-free output document the model marks `s3UnwrappedXmlOutput`.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

pub mod wire;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use wire::UnwrappedXmlRestXml;

/// The first URI path segments another emulated service that SHARES S3's `s3` SigV4
/// signing name owns, and which S3 must therefore not claim.
///
/// S3's own modeled path space cannot discriminate: every bucket/object operation binds
/// `/{Bucket}` or `/{Bucket}/{Key+}`, a path LABEL that matches ANY first segment, which
/// is why the generated `crate::ROUTE_PATH_SEGMENTS` is EMPTY ("no constraint") for this
/// service. S3 Control
/// (`services/service-com-amazonaws-s3control-awss3controlservicev20180820`) signs as
/// `s3` too — `smithy_query(query="service_info")` on its model reports sigv4Name `s3`
/// with endpointPrefix `s3-control` — and binds EVERY one of its operations under the
/// single literal first segment below (`/v20180820/bucket/{Bucket}`, `/v20180820/jobs`,
/// `/v20180820/accesspoint`, …). Excluding that one segment is what leaves exactly one
/// claimant for every request signed for `s3`, instead of two and a
/// `500 AmbiguousService` for both.
///
/// AWS itself separates the two by endpoint host, which an emulator cannot use: a client
/// configured with one local endpoint URL sends the same `Host` for both services. The
/// one request this costs is a bucket literally NAMED `v20180820`, which is the narrowest
/// possible loss and strictly better than refusing every `s3` request.
const PEER_PATH_SEGMENTS: &[&str] = &["v20180820"];

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses the path space of a peer service listed in
/// `peer_segments`. Matching is EXACT segment equality, deliberately never a prefix test:
/// a bucket named `v20180820-logs` is S3's, not the peer's.
fn addresses_peer_path(parts: &http::request::Parts, peer_segments: &[&str]) -> bool {
    match first_path_segment(parts.uri.path()) {
        Some(segment) => peer_segments.contains(&segment),
        // The ROOT path (`GET /?x-id=ListBuckets`) is S3's own; no peer claims it.
        None => false,
    }
}

/// The `com-amazonaws-s3-amazons3` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it. See [`wire`] for what it
    /// changes about the generated one: the output document of the operations the model
    /// marks `s3UnwrappedXmlOutput` carries no envelope.
    protocol: UnwrappedXmlRestXml,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: UnwrappedXmlRestXml::new(),
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
        // This service signs as `s3` — a signing name it SHARES with S3 Control, whose
        // own endpoint is `s3-control`. The router's routing key comes from the SigV4
        // credential scope first, so both services resolve to `s3`, and matching on the
        // signing name ALONE makes both claim every request — which the router answers
        // `500 AmbiguousService` for, to BOTH services' traffic.
        //
        // S3 cannot narrow itself by its own path space (every operation is bound under
        // the `{Bucket}` label, so its segment list is empty), so it claims every path
        // under the shared signing name EXCEPT the peer's own (see `PEER_PATH_SEGMENTS`).
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                !addresses_peer_path(parts, PEER_PATH_SEGMENTS)
            }
            _ => false,
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restXml protocol in every respect but one: an operation the
        // model marks `aws.customizations#s3UnwrappedXmlOutput` answers the document AWS
        // sends for it, its single output member promoted to the response root, which the
        // framework's generic envelope cannot express. See [`wire`].
        Some(&self.protocol)
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

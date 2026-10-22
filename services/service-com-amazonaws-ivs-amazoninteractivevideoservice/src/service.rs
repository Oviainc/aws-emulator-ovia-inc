// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ivs-amazoninteractivevideoservice` service: how it claims requests and how it words the
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

/// Whether the request these `parts` came from matches this service's SigV4 signing
/// name AND the modelled URI path space its generated `ROUTE_PATH_SEGMENTS` declares.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of that rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment must
/// be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment
/// equality. That path conjunct is what separates two services sharing one SigV4 signing
/// name — this low-latency IVS endpoint (`ivs`) and the IVS RealTime endpoint
/// (`ivsrealtime`) both sign as `ivs`, so the key alone matches both and the router
/// rejects every such request as `500 AmbiguousService`.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn claims_route_key_and_path(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The one first URI path segment the two services that sign as `ivs` BOTH bind, so the
/// model-derived path discriminator alone cannot separate them there.
///
/// Each of the two models binds its three tagging operations to `/tags/{resourceArn}`
/// (`ListTagsForResource` `GET`, `TagResource` `POST`, `UntagResource` `DELETE`); every
/// other operation of either service binds a URI whose first segment is its own
/// operation name, and those are disjoint. Source: `smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-ivs-amazoninteractivevideoservice/\
/// smithy-model/com-amazonaws-ivs-amazoninteractivevideoservice.json")` and the same
/// call on the `…-ivsrealtime-amazoninteractivevideoservicerealtime` model.
const SHARED_TAGGING_PATH_SEGMENT: &str = "tags";

/// The ARN resource types the SIBLING `ivsrealtime` endpoint owns — the resources its own
/// model creates and therefore the only ones its tagging operations can address.
///
/// Both services' ARNs live in the one `ivs` arnNamespace, so the resource TYPE is what
/// names the owning endpoint. Source: the `ivsrealtime` model's ARN shapes, whose
/// patterns pin the resource segment —
/// `smithy_query(query="shape", shape="CompositionArn,EncoderConfigurationArn,\
/// IngestConfigurationArn,PublicKeyArn,StageArn,StorageConfigurationArn",
/// model_path="services/service-com-amazonaws-ivsrealtime-\
/// amazoninteractivevideoservicerealtime/smithy-model/\
/// com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime.json")`. That model's
/// `ChannelArn` is deliberately NOT here: it is a REFERENCE to a low-latency channel
/// (RealTime has no operation that creates one), and a channel is tagged through this
/// endpoint.
const SIBLING_ENDPOINT_RESOURCE_TYPES: &[&str] = &[
    "composition",
    "encoder-configuration",
    "ingest-configuration",
    "public-key",
    "stage",
    "storage-configuration",
];

/// The `arnNamespace` both services share, and so the only service field an ARN either
/// of their tagging operations addresses can carry. From
/// `smithy_query(query="service_info")` on either model (`awsService.arnNamespace`:
/// `ivs` for both).
const SHARED_ARN_NAMESPACE: &str = "ivs";

/// What a request path says about the `/tags/{resourceArn}` space both `ivs`-signing
/// services publish.
#[derive(Debug)]
enum TaggingTarget {
    /// Not on the shared path at all: the modelled path space has already decided the
    /// claim, and this discriminator adds nothing.
    NotShared,
    /// On the shared path, naming an IVS resource of this type (lowercased).
    Resource(String),
    /// On the shared path, naming no IVS resource either model could recognise.
    Unrecognised,
}

/// Read the shared tagging path: whether this request is on it, and which IVS resource
/// type its `{resourceArn}` label names.
///
/// One function rather than a path-segment helper plus an ARN helper, so the whole
/// shared-path decision is read in one place. The `{resourceArn}` label is
/// percent-decoded (it is a non-greedy `@httpLabel`, so a client encodes its `:` and
/// `/`) and its resource type is lowercased, so a non-canonical ARN cannot land a
/// request on the wrong endpoint. The `tags` segment itself is compared RAW, because
/// the framework's path conjunct has already matched that segment against the model's
/// `ROUTE_PATH_SEGMENTS` by exact equality: decoding it here could only disagree with
/// the conjunct that runs first.
///
/// The label counts as naming a resource only when it is an ARN in the `ivs` namespace
/// the two services share — `arn:<partition>:ivs:<region>:<account>:<type>/<id>`, the
/// form every ARN shape in either model pins — so another service's ARN, or a string
/// that merely has a sixth `:`-field, names nothing. Reading that field and cutting it
/// at `/` also handles a client that sent the ARN's separators raw, because the ARN's
/// own `/` then just starts the next path segment.
fn tagging_target(path: &str) -> TaggingTarget {
    let mut segments = path.trim_start_matches('/').split('/');
    if segments.next() != Some(SHARED_TAGGING_PATH_SEGMENT) {
        return TaggingTarget::NotShared;
    }
    let Some(label) = segments.next() else {
        return TaggingTarget::Unrecognised;
    };
    let arn = smithy_server_runtime::percent_decode(label);
    let fields: Vec<&str> = arn.split(':').collect();
    // arn : <partition> : <service> : <region> : <account> : <resource-type>/<id>
    let [prefix, _partition, service, region, account, resource, ..] = fields.as_slice() else {
        return TaggingTarget::Unrecognised;
    };
    let kind = resource
        .split('/')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !prefix.eq_ignore_ascii_case("arn")
        || !service.eq_ignore_ascii_case(SHARED_ARN_NAMESPACE)
        || region.is_empty()
        || account.is_empty()
        || kind.is_empty()
    {
        return TaggingTarget::Unrecognised;
    }
    TaggingTarget::Resource(kind)
}

/// Whether this service claims a request on the `/tags/{resourceArn}` path space both
/// `ivs`-signing services publish.
///
/// The ARN in the path names the resource, and a resource belongs to exactly one of the
/// two endpoints, so the resource type decides: this endpoint claims every tagging
/// request EXCEPT one naming a resource type the sibling `ivsrealtime` endpoint owns.
/// Stated as a complement rather than as "my own types" on purpose — it makes this
/// service, whose `endpointPrefix` is the shared signing name `ivs`, the single claimant
/// for a label that names no recognisable IVS resource, so a malformed or unknown ARN is
/// answered by a modelled 4xx from a real service instead of the router's
/// `404 UnknownService`.
fn claims_shared_tagging_path(parts: &http::request::Parts) -> bool {
    match tagging_target(parts.uri.path()) {
        TaggingTarget::NotShared | TaggingTarget::Unrecognised => true,
        TaggingTarget::Resource(kind) => {
            !SIBLING_ENDPOINT_RESOURCE_TYPES.contains(&kind.as_str())
        }
    }
}

/// The `com-amazonaws-ivs-amazoninteractivevideoservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `ivs` — a signing name it SHARES with IVS RealTime
        // (`services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime`),
        // whose endpoint is `ivsrealtime` while this one's is `ivs`. The router resolves a
        // request's routing key from its SigV4 credential scope first, so BOTH services
        // see the key `ivs`: claiming on the signing name alone makes each one claim the
        // other's traffic, which the router answers `500 AmbiguousService` instead of
        // serving either.
        //
        // So the claim is narrowed to this service's own request space, in the two steps
        // the request itself can be read by: its modelled URI path space
        // (`claims_route_key_and_path`), and — for the one path space both models publish,
        // `/tags/{resourceArn}` — the resource the ARN in that path names
        // (`claims_shared_tagging_path`).
        claims_route_key_and_path(parts) && claims_shared_tagging_path(parts)
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

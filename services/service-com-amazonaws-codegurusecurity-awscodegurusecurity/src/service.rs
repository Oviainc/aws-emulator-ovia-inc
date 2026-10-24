// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-codegurusecurity-awscodegurusecurity` service: how it claims requests and how it words the
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
//! ## The upload plane
//!
//! `CreateUploadUrl` publishes an upload target as its `s3Url`, and such a URL is a
//! promise that a client can perform the documented upload to it. The target is served
//! here, through [`Service::can_handle`] (which also claims the upload host this service
//! publishes) and [`Service::override_routing_error`] (which serves the `PUT`, a request
//! that names no modelled operation by construction). See [`upload_plane`].

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

mod upload_plane;

/// The error code this service answers a resource ARN that is not an ARN with.
///
/// NOT a shape this service's model declares — which is why it is a string here and
/// not a modelled error variant: the recorded service answers a malformed
/// `resourceArn` label on the tagging operations with HTTP 400 and
/// `x-amzn-errortype: BadRequestException`, the front-end ARN parser's own answer,
/// rather than the modelled `ValidationException` the member-constraint validator
/// would report. Restated here because `override_refusal` replaces the code and the
/// message TOGETHER.
pub(crate) const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// What this service says when a resource ARN does not parse.
///
/// A CONSTANT that interpolates no part of the request: an ARN parser reports that the
/// label was not an ARN, and nothing about which member or which rule it failed — so
/// there is nothing of the request to state.
pub(crate) const MALFORMED_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// Whether `violation` is a value that failed the `@pattern` of this service's
/// `ScanNameArn` shape — i.e. a resource ARN that is not an ARN at all.
///
/// Keyed on the pattern the value failed ([`crate::state::SCAN_NAME_ARN_PATTERN`], the
/// `ScanNameArn` shape's own), not on a member name or an operation: every INPUT
/// member of this service that targets `ScanNameArn` is a tagging operation's
/// `resourceArn` label, and every other constrained member of this service carries a
/// different pattern, so none of them is caught here.
fn is_malformed_resource_arn(violation: &Violation) -> bool {
    matches!(
        &violation.kind,
        ViolationKind::Pattern(pattern) if pattern == crate::state::SCAN_NAME_ARN_PATTERN
    )
}

/// The `com-amazonaws-codegurusecurity-awscodegurusecurity` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the upload plane is served from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own; it must read and write the one store the operations use,
    /// not a second copy.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
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
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request addressed to the UPLOAD host this service publishes its upload
        // targets under, whose first label names no service at all: without this the
        // router answers `UnknownService` for a URL this service handed out. See the
        // module docs.
        request_host(parts).is_some_and(crate::state::is_upload_host)
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
        // CodeGuru Security PARSES the `resourceArn` @httpLabel of a tagging call
        // before it validates anything else, so a label that is not an ARN is a
        // malformed REQUEST — answered with HTTP 400 `BadRequestException`, the
        // parser's own code — and not a member that failed a modelled constraint,
        // which is what `ValidationException` reports. The status is 400 either way;
        // the code is what a client branches on, so both it and the message are
        // replaced together, and the ValidationException-style "N validation error(s)
        // detected" prose goes with the code.
        //
        // `any`, not "every violation": the ARN parse runs first, so it decides the
        // answer even when the same request also breaks another rule.
        //
        // Every OTHER refusal keeps the framework's default wording and the modelled
        // `ValidationException` code: an absent required member, a value outside a
        // declared bound (including a `resourceArn` that parses as an ARN but is
        // longer than the shape's `@length` allows — a size limit, not a syntax
        // failure) and an authentication refusal are all reported as this service's
        // model declares them.
        match refusal {
            Refusal::Invalid(violations) if violations.iter().any(is_malformed_resource_arn) => {
                Some(Refused {
                    code: BAD_REQUEST_EXCEPTION.to_string(),
                    message: MALFORMED_ARN_MESSAGE.to_string(),
                })
            }
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A request to an upload target this service published is exactly that case by
        // construction — the path is not in the model — so this is where the documented
        // upload is received. The UPLOAD host decides: a request to this service's own
        // endpoint that named no operation is a routing failure, whatever its path looks
        // like, and keeps the framework's default.
        if !upload_host_of(req).is_some_and(crate::state::is_upload_host) {
            return None;
        }
        let target = crate::state::parse_upload_path(request_path(req.uri()))?;
        Some(upload_plane::serve(&*self.engine, req, &target))
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

/// The `Host` header of a buffered request, or the URI authority when the request is
/// HTTP/2 (which carries `:authority` on the URI instead of a `Host` header).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

/// The host a runtime-API request is addressed to: its `Host` header, or the authority of
/// its URI when the request carries none (HTTP/2 names the endpoint there).
fn upload_host_of(req: &aws_smithy_runtime_api::http::Request) -> Option<&str> {
    match req.headers().get("host") {
        Some(host) => Some(host),
        None => request_authority(req.uri()),
    }
}

/// The authority of an absolute request URI, or `None` when the URI carries none.
fn request_authority(uri: &str) -> Option<&str> {
    let (_scheme, authority_and_path) = uri.split_once("://")?;
    Some(match authority_and_path.find('/') {
        Some(path_start) => &authority_and_path[..path_start],
        None => authority_and_path,
    })
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sustainability-awssustainabilityapiservice` service: how it claims requests and how it words the
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

/// The `com-amazonaws-sustainability-awssustainabilityapiservice` emulated service: the generated [`ServiceDefinition`] plus
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

/// The SigV4 credential-scope terminator every signing flavour ends its scope with.
const SCOPE_TERMINATOR: &str = "aws4_request";

/// The prefix of every SigV4-family signing algorithm: `AWS4-HMAC-SHA256` (SigV4) and
/// `AWS4-ECDSA-P256-SHA256` (SigV4A) today.
const SIGV4_ALGORITHM_PREFIX: &str = "AWS4-";

/// The label an AWS endpoint host may carry AHEAD of the service name, as this
/// service's own endpoint rules publish it
/// (`https://api.sustainability.{Region}.{dualStackDnsSuffix}` — see
/// `smithy_query(query="service_info")`, `endpointResolution.urlTemplates`).
const ENDPOINT_HOST_PREFIX: &str = "api";

/// The FIPS marker an endpoint host appends to the service name
/// (`https://sustainability-fips.global.{dualStackDnsSuffix}`, same source).
const FIPS_SUFFIX: &str = "-fips";

/// The signing name the SigV4 credential scope of `parts` names, for ANY signing
/// flavour of the SigV4 family — lower-cased, as the framework's own route keys are.
///
/// A credential scope is `<access key id>/<date>[/<region>]/<signing name>/aws4_request`:
/// the SigV4A form (`AWS4-ECDSA-P256-SHA256`) omits the region segment, because a
/// SigV4A signature is valid across a SET of regions, which the request carries in
/// `X-Amz-Region-Set` instead. The signing name is therefore read as the field BEFORE
/// the `aws4_request` terminator rather than as a fixed fourth field, which reads both
/// forms and is what makes a SigV4A-signed request routable at all.
///
/// The scope is read from the `Authorization` header, and failing that from a presigned
/// URL's `X-Amz-Credential` query parameter (whose `/` separators arrive percent-encoded),
/// so the reader does not depend on WHERE the protocol carries the scope.
fn credential_scope_signing_name(parts: &http::request::Parts) -> Option<String> {
    let scope = authorization_scope(&parts.headers).or_else(|| presigned_scope(&parts.uri))?;
    let mut fields = scope.split('/').rev();
    let terminator = fields.next()?;
    let signing_name = fields.next()?;
    // `[access key id, date]` for SigV4A, `[access key id, date, region]` for SigV4:
    // the region is optional, the other two are not, and no flavour carries more.
    let leading = fields.count();
    if terminator != SCOPE_TERMINATOR || signing_name.is_empty() || !(2..=3).contains(&leading) {
        return None;
    }
    Some(signing_name.trim().to_ascii_lowercase())
}

/// The credential scope of an `Authorization` header signed by any SigV4-family
/// algorithm: the value of its `Credential=` field.
fn authorization_scope(headers: &http::HeaderMap) -> Option<String> {
    let header = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let (algorithm, fields) = header.split_once(' ')?;
    if !algorithm.starts_with(SIGV4_ALGORITHM_PREFIX) {
        return None;
    }
    let field = fields
        .split(',')
        .find_map(|field| field.trim().strip_prefix("Credential="))?;
    Some(field.trim().to_string())
}

/// The credential scope of a presigned URL, whose `X-Amz-Credential` query parameter
/// carries the same scope with its `/` separators percent-encoded.
fn presigned_scope(uri: &http::Uri) -> Option<String> {
    let encoded = uri.query()?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == "X-Amz-Credential").then_some(value)
    })?;
    Some(encoded.replace("%2F", "/").replace("%2f", "/"))
}

/// The service name the host-style endpoint of `parts` addresses, lower-cased.
///
/// An AWS endpoint host names its service in the first label, optionally behind the
/// `api.` prefix its endpoint rules publish, and optionally with the `-fips` marker of
/// a FIPS endpoint appended; the remaining labels name the region and the DNS suffix.
/// The framework's own host reader takes the FIRST label only, so a request addressed
/// to a host of the `api.<service>.…` form resolves to `api` and names no service.
fn endpoint_host_service_name(parts: &http::request::Parts) -> Option<String> {
    let host = match parts.headers.get(http::header::HOST) {
        Some(host) => host.to_str().ok()?,
        None => parts.uri.host()?,
    };
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let mut labels = host.split('.');
    let first = labels.next()?;
    let named = if first.eq_ignore_ascii_case(ENDPOINT_HOST_PREFIX) {
        labels.next()?
    } else {
        first
    };
    let named = named.strip_suffix(FIPS_SUFFIX).unwrap_or(named);
    (!named.is_empty()).then(|| named.to_ascii_lowercase())
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service's model lists `aws.auth#sigv4a` FIRST, so an SDK configured for
        // it signs with SigV4A (`AWS4-ECDSA-P256-SHA256`), whose credential scope has
        // no region segment; and it is addressed on `api.sustainability.<region>.…`,
        // whose FIRST host label is the `api` prefix rather than the service. The
        // framework's `resolve_route_key` reads a plain `AWS4-HMAC-SHA256` scope, then
        // `X-Amz-Target` (which `restJson1` never sends), then the first host label, so
        // a SigV4A-signed request to this service resolves to NO key at all and is left
        // unclaimed — which the router answers `404 UnknownService` in its own
        // awsJson1_1 envelope, an error code this service's model does not declare.
        //
        // So the claim is widened by the two signals the model does publish, each
        // naming this service's own signing name: a credential scope of ANY
        // SigV4-family flavour, and a host-style endpoint of the forms this service's
        // endpoint rules resolve to. Both name one service, so neither can make a peer
        // claim the same request.
        let signing_name = self.entry.identity().signing_name();
        let names_this_service =
            |name: &String| name.trim().eq_ignore_ascii_case(signing_name.trim());
        smithy_server_runtime::resolve_route_key(parts)
            .as_ref()
            .is_some_and(names_this_service)
            || credential_scope_signing_name(parts)
                .as_ref()
                .is_some_and(names_this_service)
            || endpoint_host_service_name(parts)
                .as_ref()
                .is_some_and(names_this_service)
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

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ssooidc-awsssooidcservice` service: how it claims requests and how it words the
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
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The routing key a SIGNED request to this service carries: the SigV4 signing name the
/// model declares (`aws.auth#sigv4` `name`), as the generated crate publishes it.
const ROUTE_KEY: &str = crate::ROUTE_KEY;

/// The endpoint this service is addressed by, which a host-addressed request resolves to
/// instead of the signing name: the model's `aws.api#service` `endpointPrefix`
/// (`smithy_query(query="service_info")` reports `endpointPrefix: oidc` beside
/// `sigv4Name: sso-oauth`, so the two differ and both name this one endpoint).
const ENDPOINT_PREFIX: &str = "oidc";

/// Whether the routing key the framework resolves for `parts` is one of `route_keys` —
/// the keys this one endpoint answers on.
///
/// `emulator_server::can_handle_based_on_route_key`'s rule, over the `Parts` the
/// `can_handle` seam is handed rather than the whole buffered request that helper takes,
/// and over the SEVERAL keys an endpoint answers on: the framework resolves the key from
/// the credential scope first (the signing name) and falls back to the `Host` /
/// `:authority` label (the `endpointPrefix`) for a request that carries no scope, so both
/// name this endpoint. Comparison is case-insensitive on a trimmed name, as there.
fn names_this_endpoint(parts: &http::request::Parts, route_keys: &[&str]) -> bool {
    match smithy_server_runtime::resolve_route_key(parts) {
        Some(key) => route_keys
            .iter()
            .any(|route_key| key.eq_ignore_ascii_case(route_key.trim())),
        None => false,
    }
}

/// Whether `parts` names NO target service at all and addresses a route this service's
/// own model binds.
///
/// The claim rule for the traffic the tiered routing key cannot attribute. The framework
/// resolves a request's target from a SigV4 credential scope (header, then presigned
/// query), then an RPC target header, then a service-naming endpoint host. A request for
/// an operation whose model serves it ANONYMOUSLY carries neither a scope nor — on a REST
/// protocol — a target header, and a client pointed at the emulator addresses a loopback
/// name or an IP literal, which names no service either: so nothing in the request names
/// one, every tier answers `None`, and no claim that starts from the routing key can see
/// it. What attributes it is the one thing left, and the one AWS itself dispatches a REST
/// request on once the endpoint has chosen the service: the modelled route — the method
/// and the URI template — read through this service's OWN protocol and routing table, so
/// nothing here restates what the model already says.
///
/// A claim of LAST resort, deliberately conjoined with "no routing key at all": a request
/// that DOES name a service belongs to the service it names, even where this model binds
/// its route too, and a nameless request on a route this model does not bind is still
/// claimed by nobody (the router's `404 UnknownService`). So this service answers for its
/// own published surface and for nothing else, and whether the request then needs a
/// signature stays with the router's authenticator, which reads the modelled operation's
/// own auth schemes.
fn claims_unattributed_route(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
) -> bool {
    if smithy_server_runtime::resolve_route_key(parts).is_some() {
        return false;
    }
    // The three things a REST routing key is read from — the method, the URI and the
    // headers — re-presented to the protocol as the bodiless runtime-API request it
    // reads, which is the same lookup the router performs once a service has claimed a
    // request (its protocol resolves the key, the model-derived table holds the
    // operation). No body is needed: `restJson1` routes on the request line.
    let mut request = aws_smithy_runtime_api::http::Request::new(
        aws_smithy_types::body::SdkBody::empty(),
    );
    if request.set_uri(parts.uri.to_string().as_str()).is_err() {
        return false;
    }
    if request.set_method(parts.method.as_str()).is_err() {
        return false;
    }
    for (name, value) in parts.headers.iter() {
        if let Ok(value) = value.to_str() {
            request
                .headers_mut()
                .append(name.as_str().to_string(), value.to_string());
        }
    }
    match definition.protocol().routing_key(&request) {
        Ok(key) => definition.routing_table().get(key).is_some(),
        Err(_) => false,
    }
}

/// The `com-amazonaws-ssooidc-awsssooidcservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // Every operation of this service but `CreateTokenWithIAM` is `@optionalAuth`
        // with an empty auth list, so an SDK sends it UNSIGNED: there is no credential
        // scope naming `sso-oauth`, `restJson1` carries no `X-Amz-Target`, and a client
        // pointed at the emulator addresses a loopback name or an IP literal, which names
        // no service either. Nothing in such a request names a service, so the routing
        // key the default claim compares is absent and the whole device-authorization
        // flow reached nobody.
        //
        // So the claim is stated twice over, for the two ways a request can arrive:
        // by NAME, for a request that carries one — the signing name a signed request's
        // scope carries, or the endpoint prefix the model publishes, which an unsigned
        // request addressed to `oidc.<region>.amazonaws.com` resolves to; and, for a
        // request that names nothing at all, by the modelled route it addresses, the one
        // discriminator left and the one AWS dispatches a REST request on once the
        // endpoint has chosen the service. Both halves read the model rather than
        // restating it: see the two functions above.
        names_this_endpoint(parts, &[ROUTE_KEY, ENDPOINT_PREFIX])
            || claims_unattributed_route(parts, self.definition())
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

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-lightsail-lightsail-20161128` service: how it claims requests and how it words the
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
//! ## The container service public endpoint
//!
//! `ContainerService$url` is "The publicly accessible URL of the container service",
//! so it is a promise that a caller can follow it: reading the field and making an
//! HTTP request to it is what an AWS client does with it. The address this emulator
//! announces is therefore built from the endpoint the request arrived at
//! (`crate::state::container_service_url`), under the path space
//! `crate::state::CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`, which names no modelled
//! operation. Two seams carry it, the same pair any emulated data plane uses:
//!
//! - [`Service::can_handle`] also claims a request addressed to that published path,
//!   which carries no routing key at all (the emulator's own endpoint host is an IP
//!   literal or `localhost`, and a browser or `curl` following the URL signs
//!   nothing), so the router does not answer `UnknownService` for a URL this service
//!   handed out;
//! - [`Service::override_routing_error`] serves it. A public-endpoint request names
//!   no modelled operation by construction, so the router reaches exactly this seam
//!   for it, and reaches it BEFORE authentication — which is what lets a public URL
//!   be followed unsigned, as a public URL is.
//!
//! What it answers is what this emulator knows: the modelled `404` for a service
//! whose current deployment declares no `publicEndpoint` ("If no public endpoint is
//! specified in the currentDeployment, this URL returns a 404 response"), and `501`
//! where one IS declared — this emulator runs no container runtime, so the deployed
//! image's own response is not something it can produce, and synthesising a body
//! that image never served would be inventing content.
//!
//! ## The container image registry address
//!
//! `ContainerServiceRegistryLogin$registry` is the same kind of promise — "The
//! address to use to push container images to the container image registry of a
//! Lightsail account" — so `CreateContainerServiceRegistryLogin` announces it
//! beneath the endpoint the request arrived at too
//! (`crate::state::container_image_registry_url`, under
//! `crate::state::CONTAINER_IMAGE_REGISTRY_PATH_PREFIX`), and the same two seams
//! carry it: [`Service::can_handle`] claims a request addressed there (it is
//! unsigned — a registry client authenticates with the HTTP Basic credentials that
//! operation handed out, not with SigV4) and [`Service::override_routing_error`]
//! serves it.
//!
//! What it answers is again what this emulator knows: `401` for a client that
//! presents no live set of log in credentials for that registry, and `501` for one
//! that does — the push itself is a container image registry protocol this
//! emulator does not serve, which is also why `RegisterContainerImage` finds no
//! pushed digest to register.

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// The `com-amazonaws-lightsail-lightsail-20161128` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because a container service's public endpoint is served from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own; it must read the one store the operations write,
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
        let route_key = smithy_server_runtime::resolve_route_key(parts);
        if route_key
            .as_deref()
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request addressed to the public URL of a container service this
        // service published, which carries NO routing key of its own: the emulator's
        // own endpoint host is an IP literal or `localhost` (neither names a
        // service) and a client following a public URL signs nothing. Without this
        // the router answers `UnknownService` for a URL this service handed out. See
        // the module docs.
        //
        // The absence of a routing key is part of the claim, not incidental: a
        // request that DOES name a service names which service is to answer it, and
        // a second claimant for it would only make the router refuse both as
        // `AmbiguousService`.
        if route_key.is_some() {
            return false;
        }
        // `http::Uri::path` already strips the scheme, the authority and the query,
        // for an origin-form request line and an absolute-form one alike.
        let path = parts.uri.path();
        crate::state::parse_container_service_public_path(path).is_some()
            // And a request addressed to the container image registry address this
            // service announced, which carries no routing key either: a registry
            // client authenticates with HTTP Basic credentials and signs nothing.
            || crate::state::parse_container_image_registry_path(path).is_some()
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A request to the public URL of a container service is exactly that case by
        // construction — the path is not in the model — so this is where that URL is
        // served. Anything else keeps the framework's default.
        let path = request_path(req.uri());
        if let Some(target) = crate::state::parse_container_service_public_path(path) {
            return Some(self.serve_container_service_endpoint(req.method(), &target));
        }
        // The announced container image registry address is the same case: not in
        // the model, so this is where it is served.
        let region = crate::state::parse_container_image_registry_path(path)?;
        Some(self.serve_container_image_registry(req.method(), &region, req.headers()))
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

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Serve one request to a container service's published public URL.
    ///
    /// A fetch of that URL is a pure READ of the control plane: it reports what the
    /// addressed service's CURRENT deployment declares and writes nothing, so
    /// following the URL twice changes nothing a later caller sees. (The one write
    /// behind it is the clock materialising the lifecycle phases it already owed,
    /// which `crate::state::with_state` applies for every reader alike.)
    ///
    /// The region, the name and the per-service token the URL carries must all name
    /// one stored service — a URL whose service this store no longer holds, or whose
    /// token belongs to another service of that name, addresses nothing.
    fn serve_container_service_endpoint(
        &self,
        method: &str,
        target: &crate::state::ContainerServicePublicTarget,
    ) -> RtResponse {
        let Some(serves_a_public_endpoint) =
            crate::state::with_state(&*self.engine, |store| {
                store
                    .container_service(&target.region, &target.name)
                    .filter(|service| service.domain_token == target.domain_token)
                    .map(|service| {
                        service
                            .current_deployment
                            .as_ref()
                            .is_some_and(|deployment| deployment.public_endpoint.is_some())
                    })
            })
        else {
            return endpoint_response(method, 503, STORE_UNAVAILABLE);
        };
        match serves_a_public_endpoint {
            // The modelled answer, from `ContainerService$url`'s own
            // documentation: "If no public endpoint is specified in the
            // currentDeployment, this URL returns a 404 response." A URL that names
            // no stored service at all is the same 404: nothing is published there,
            // and the two are not told apart, so a caller cannot probe for services
            // it was not handed the URL of.
            None | Some(false) => endpoint_response(method, 404, NO_PUBLIC_ENDPOINT),
            // A public endpoint IS declared, and what belongs here is the response
            // of the container image behind it. This emulator runs no container
            // runtime, so that response is not something it can produce, and a body
            // the image never served would be invented content: the honest answer is
            // that this part is not implemented.
            Some(true) => endpoint_response(method, 501, NO_CONTAINER_RUNTIME),
        }
    }

    /// Serve one request to the announced address of this account's container
    /// image registry in `region`.
    ///
    /// A registry client logs in with the credentials
    /// `CreateContainerServiceRegistryLogin` created, presented as HTTP Basic
    /// credentials, so the request is authenticated against the live sets this
    /// service's store holds for that region — a set the clock has expired is
    /// already gone from it, so credentials past their 12 hours are refused like
    /// any others that match nothing.
    ///
    /// This is a pure READ of that store: logging in neither creates nor consumes
    /// anything, so a client that tries twice changes nothing a later caller
    /// sees. (The one write behind it is the clock forgetting the sets it had
    /// already expired, which `crate::state::with_state` applies for every reader
    /// alike.)
    fn serve_container_image_registry(
        &self,
        method: &str,
        region: &str,
        headers: &aws_smithy_runtime_api::http::Headers,
    ) -> RtResponse {
        // Every log in attempt is attributable in the emulator's log, whichever way
        // it goes: the address reached, the region whose registry it names, the
        // method, the username presented (none, where the client presented no
        // credentials at all) and the outcome. The presented PASSWORD and the stored
        // digest are never logged — only the caller ever holds the password.
        let audit = |outcome: &'static str, username: Option<&str>| {
            ::tracing::info!(
                audit = true,
                operation = "ContainerImageRegistryLogin",
                outcome,
                region = %region,
                method = %method,
                source_endpoint = headers.get("host").unwrap_or("<none>"),
                presented_username = username.unwrap_or("<none>"),
                "container image registry log in attempt"
            );
        };

        let Some((username, password)) = basic_credentials(headers) else {
            audit("refused", None);
            return registry_unauthorized(method);
        };
        let Some(recognised) = crate::state::with_state(&*self.engine, |store| {
            store.registry_login(region, &username, &password).is_some()
        }) else {
            audit("unavailable", Some(&username));
            return endpoint_response(method, 503, REGISTRY_STORE_UNAVAILABLE);
        };
        if !recognised {
            audit("refused", Some(&username));
            return registry_unauthorized(method);
        }
        audit("accepted", Some(&username));
        // The credentials are this account's, and what belongs here now is the
        // container image registry protocol itself: a `docker push` of image
        // layers and a manifest. This emulator serves no such data plane, so
        // answering anything but "not implemented" would be claiming a push that
        // nothing stored.
        endpoint_response(method, 501, NO_CONTAINER_IMAGE_REGISTRY)
    }
}

/// The HTTP Basic challenge a registry address answers an unauthenticated
/// request with: RFC 9110 requires a `WWW-Authenticate` challenge on a `401`, and
/// it is what tells a registry client to log in with the credentials
/// `CreateContainerServiceRegistryLogin` hands out.
fn registry_unauthorized(method: &str) -> RtResponse {
    let mut resp = endpoint_response(method, 401, REGISTRY_LOGIN_REQUIRED);
    resp.headers_mut().insert(
        "www-authenticate",
        format!("Basic realm=\"{REGISTRY_REALM}\""),
    );
    resp
}

/// The `user:password` pair an HTTP Basic `Authorization` header carries, or
/// `None` when the request presents no Basic credentials at all.
///
/// The scheme name is compared case-insensitively and separated from the
/// credentials on any run of whitespace, as RFC 9110's `credentials = scheme
/// 1*SP token68` admits, and the credentials are base64 with the standard
/// alphabet — the same codec the wire serializer uses for every Smithy blob.
fn basic_credentials(
    headers: &aws_smithy_runtime_api::http::Headers,
) -> Option<(String, String)> {
    let value = headers.get("authorization")?.trim();
    let (scheme, encoded) = value.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("Basic") {
        return None;
    }
    let decoded = ::aws_smithy_types::base64::decode(encoded.trim()).ok()?;
    let (user, password) = String::from_utf8(decoded).ok()?.split_once(':').map(
        |(user, password)| (user.to_string(), password.to_string()),
    )?;
    Some((user, password))
}

/// What the announced registry address answers when this service's store cannot be
/// reached at all — the engine is not the shared in-memory one. The registry's own
/// wording of [`STORE_UNAVAILABLE`], and the same `503`: a client that gets here
/// has reached the registry's front door while its backing store is momentarily
/// not there, which is what `503` says and what makes the attempt retryable.
const REGISTRY_STORE_UNAVAILABLE: &str =
    "The Lightsail container image registry store is unavailable.\n";

/// The realm the registry address names in its `WWW-Authenticate` challenge: a
/// constant of this module, so it carries nothing from the request.
const REGISTRY_REALM: &str = "Lightsail container image registry";

/// What the announced registry address answers a client that presents no live set
/// of log in credentials. One sentence carrying nothing from the request, so a
/// caller cannot probe with it.
const REGISTRY_LOGIN_REQUIRED: &str =
    "Log in to this container image registry with the credentials \
     CreateContainerServiceRegistryLogin creates.\n";

/// What it answers a client that IS logged in: the push itself is a container
/// image registry protocol this emulator does not serve.
const NO_CONTAINER_IMAGE_REGISTRY: &str =
    "This container image registry is not implemented: the emulator serves no container \
     image registry protocol, so a pushed image cannot be stored.\n";

/// What the published URL of a container service that fronts no container answers.
/// One sentence carrying nothing from the request, so a caller cannot probe with it.
const NO_PUBLIC_ENDPOINT: &str =
    "No public endpoint is specified in the current deployment of this container service.\n";

/// What it answers when a public endpoint IS declared: this emulator serves no
/// container runtime, so the deployed image's own response cannot be produced here.
const NO_CONTAINER_RUNTIME: &str =
    "This container service's public endpoint is not implemented: the emulator runs no \
     container runtime, so the deployed image's response cannot be served.\n";

/// What it answers when this service's store cannot be reached at all — the engine is
/// not the shared in-memory one. A server fault, never a fresh engine and never a
/// guess at what is deployed.
///
/// `503` rather than the `500` the artifact document plane answers for the same
/// condition: this is the address a container is reached ON, so a caller that gets
/// here has reached the front door of a service whose backing store is momentarily
/// not there, which is what `503` says and what makes the fetch retryable.
const STORE_UNAVAILABLE: &str = "The Lightsail container service store is unavailable.\n";

/// One response from a container service's public endpoint.
///
/// The public endpoint is plain HTTP(S) rather than a modelled operation — it is the
/// address the deployed container is reached on — so it answers in its own plain-text
/// form instead of the service's awsJson1_1 envelope. Every message is a constant of
/// this module, never built from the request.
///
/// A `HEAD` carries the metadata of what a `GET` of the same address would answer —
/// its status and its length — without the body, which is what the method means (the
/// same rule the artifact document plane follows).
fn endpoint_response(method: &str, status: u16, message: &'static str) -> RtResponse {
    let body = if method == "HEAD" {
        SdkBody::empty()
    } else {
        SdkBody::from(message)
    };
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        body,
    );
    resp.headers_mut()
        .insert("content-type", "text/plain; charset=utf-8");
    resp.headers_mut()
        .insert("content-length", message.len().to_string());
    resp
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan
    // over the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

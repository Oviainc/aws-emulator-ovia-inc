// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sso-swbportalservice` service: how it claims requests and how it words the
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

use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;
use std::time::SystemTime;

/// This service's modeled endpoint prefix (`portal.sso`), as `OPERATIONS.md` and the
/// model's `awsService` trait give it. Used by [`EmulatedService::can_handle`] to claim
/// an unsigned request addressed to the portal endpoint.
const PORTAL_ENDPOINT_PREFIX: &str = "portal.sso";

/// The host a request is addressed to, from the `Host` header or (HTTP/2) the URI
/// authority, lowercased with any `:port` removed. `None` when the request names none.
fn endpoint_host(parts: &http::request::Parts) -> Option<String> {
    let host = parts
        .headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| parts.uri.authority().map(|a| a.as_str().to_string()))?;
    let without_userinfo = host.rsplit('@').next().unwrap_or(&host).to_string();
    let without_port = without_userinfo
        .split(':')
        .next()
        .unwrap_or(&without_userinfo)
        .to_ascii_lowercase();
    (!without_port.is_empty()).then_some(without_port)
}

/// Whether `host` is this service's own endpoint: the modelled prefix itself, or that
/// prefix followed by a dot (`portal.sso.<region>.amazonaws.com`). A dot is required so
/// a host that merely STARTS with the prefix (`portal.ssoother.example`) is not this
/// endpoint.
///
/// What follows that dot is DELIBERATELY unconstrained: the model publishes the endpoint
/// prefix, not the region labels or the partition's DNS suffix, so pinning a suffix here
/// would be invented data, and an emulator is reached through whatever endpoint its
/// operator configured. A host naming another partition is served; only the fidelity of
/// endpoint resolution — which this emulator does not model — would refuse it.
fn is_portal_endpoint(host: &str) -> bool {
    host == PORTAL_ENDPOINT_PREFIX
        || host
            .strip_prefix(PORTAL_ENDPOINT_PREFIX)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Where an http-bound member rides on the wire.
enum BoundIn {
    Header,
    Query,
}

/// One `@required` http-bound member of a route: the Smithy member name, where it
/// rides, and the name it rides under.
struct RequiredMember {
    member: &'static str,
    bound_in: BoundIn,
    name: &'static str,
}

const ACCESS_TOKEN: RequiredMember = RequiredMember {
    member: "accessToken",
    bound_in: BoundIn::Header,
    name: "x-amz-sso_bearer_token",
};
const ACCOUNT_ID: RequiredMember = RequiredMember {
    member: "accountId",
    bound_in: BoundIn::Query,
    name: "account_id",
};
const ROLE_NAME: RequiredMember = RequiredMember {
    member: "roleName",
    bound_in: BoundIn::Query,
    name: "role_name",
};

/// One route this service serves: the method and the path of an `@http` binding, and the
/// `@required` http-bound members of that operation.
type Route = (&'static str, &'static str, &'static [RequiredMember]);

/// Every route this service serves and the `@required` http-bound members of each, as
/// the model declares them (`OPERATIONS.md` lists the same four).
///
/// `restJson1` selects a route only when every one of those members is on the request,
/// so a request that omits one matches no operation at all — including the one that
/// omits the bearer credential, which is why the portal's authentication gate has to be
/// answered in [`Service::override_routing_error`] as well as in every handler. This
/// table is what lets that seam tell a request addressed to one of these routes — one
/// presenting no session, or a readable request missing a required parameter — from a
/// path this service does not serve, and it is also the request SIGNATURE
/// [`serves_route`] claims an anonymous portal request by.
///
/// DRIFT: this is model-derived data held by hand, because the generated
/// `ServiceDefinition`'s routing table exposes no required-member set to read it from.
/// A route or required member the model gains and this table misses is not answered
/// WRONGLY, just unreworded — the request falls back to the framework's routing
/// answer. The invalid-request test in `tests/all/portal_session.rs` carries one row
/// per (route, required member) pair here, so a row that disagrees with the model
/// fails there, and `tests/all/endpoint_agnostic_routing.rs` carries one row per
/// route.
const ROUTES: &[Route] = &[
    ("GET", "/assignment/accounts", &[ACCESS_TOKEN]),
    ("GET", "/assignment/roles", &[ACCESS_TOKEN, ACCOUNT_ID]),
    (
        "GET",
        "/federation/credentials",
        &[ACCESS_TOKEN, ACCOUNT_ID, ROLE_NAME],
    ),
    ("POST", "/logout", &[ACCESS_TOKEN]),
];

/// The route in [`ROUTES`] that `method` and `uri` address, or `None` when they address
/// none of them.
///
/// The ONE place this service turns a request into a route, for both callers below, so
/// that claiming a request and wording its refusal cannot disagree about what path it
/// named. The path is whatever `uri` reports as its path component — no scheme, no
/// authority, no query string — whichever form the request arrived in: origin
/// (`/assignment/accounts?max_result=1`) or absolute
/// (`https://host/assignment/accounts?max_result=1`). Nothing is split by hand here; the
/// URI type the `http` crate already parses this request into answers it.
///
/// Matching is EXACT on both halves, never a prefix: a path this model does not bind
/// matches no route (so it stays the router's own `UnknownService`) rather than being
/// swallowed here. It recognises one of this service's own four literal routes, never
/// binds a value out of the path.
fn matched_route(method: &str, uri: &http::Uri) -> Option<&'static Route> {
    let path = uri.path();
    ROUTES
        .iter()
        .find(|(route_method, route, _)| *route_method == method && *route == path)
}

/// Whether the request's query string carries `name` as a key, with or without a value
/// — the same presence test the REST router applies to a required query binding.
fn query_has(uri: &http::Uri, name: &str) -> bool {
    uri.query().is_some_and(|query| {
        query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .any(|pair| pair.split_once('=').map_or(pair, |(key, _)| key) == name)
    })
}

/// Whether `parts` addresses one of this service's own modelled routes: its method and
/// its path are an `@http` binding in [`ROUTES`].
///
/// This is the request's own SIGNATURE — the same (method, path) pair `restJson1` routes
/// on — and for a portal request it is the only service-identifying signal there is.
/// Every portal operation is `@optionalAuth` with an empty `@auth`, so an SDK sends it
/// unsigned: there is no SigV4 credential scope, `restJson1` carries no `X-Amz-Target`,
/// and a client pointed at an endpoint that does not name a service (the emulator's own
/// `localhost:9297`, an IP literal, a proxy) leaves nothing for the host-style tier
/// either. Claiming by this table is what makes the service reachable there.
///
/// Matching is [`matched_route`]'s, which is also what the refusal wording below reads,
/// so a request claimed here is a request that seam recognises.
fn serves_route(parts: &http::request::Parts) -> bool {
    matched_route(parts.method.as_str(), &parts.uri).is_some()
}

/// The `@required` members of `req`'s route that are absent from it, or `None` when
/// `req` names no route this service serves.
fn absent_required_members(req: &aws_smithy_runtime_api::http::Request) -> Option<Vec<String>> {
    // The runtime-API request carries its URI as a string, so it is read back through the
    // same URI type [`serves_route`] is handed: one spelling of "this request's path" for
    // both seams. A string that is no URI names no route, and the framework's own routing
    // answer stands.
    let uri: http::Uri = req.uri().parse().ok()?;
    let (.., required) = matched_route(req.method(), &uri)?;
    Some(
        required
            .iter()
            .filter(|member| match member.bound_in {
                BoundIn::Header => req.headers().get(member.name).is_none(),
                BoundIn::Query => !query_has(&uri, member.name),
            })
            .map(|member| member.member.to_string())
            .collect(),
    )
}

/// The bearer credential `req` presents: the value of the header [`ACCESS_TOKEN`] names,
/// the one the model binds the access token to. `None` when the request does not carry
/// that header at all — a request presenting NO credential.
fn presented_access_token(req: &aws_smithy_runtime_api::http::Request) -> Option<&str> {
    debug_assert!(matches!(ACCESS_TOKEN.bound_in, BoundIn::Header));
    req.headers().get(ACCESS_TOKEN.name)
}

/// The `com-amazonaws-sso-swbportalservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threaded into every operation, kept
    /// here too because this file's authentication gate has to resolve a presented token
    /// against the one store the handlers read (see [`Self::presents_a_usable_session`]).
    /// Never a store of its own: it is the `Arc` the router handed to `new`.
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

    /// Whether `req` presents a sign-in session this service still serves at the moment
    /// it is asked.
    ///
    /// `false` covers every way a request can fail to present one — no access-token
    /// header at all, or one naming a session that is unknown, logged out or expired —
    /// because the portal authenticates a caller by that session and distinguishes none
    /// of them. It is the same question [`crate::state::ComAmazonawsSsoSwbportalserviceState::session`]
    /// answers for a handler, asked of a request that has not reached one.
    fn presents_a_usable_session(&self, req: &aws_smithy_runtime_api::http::Request) -> bool {
        let Some(token) = presented_access_token(req) else {
            return false;
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // UNREACHABLE with the engine the router mounts (see the note in
            // `ops/list_accounts.rs`): with no store to resolve the token against there
            // is nothing to authenticate it with, so the gate claims nothing here and
            // the framework's own answer stands.
            return true;
        };
        let state = state.lock();
        state.session(token, SystemTime::now()).is_some()
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
        let resolved = smithy_server_runtime::resolve_route_key(parts);
        // Claim a request whose resolved route key is this service's signing name
        // (`awsssoportal`).
        if resolved.as_deref() == Some(self.entry.identity().signing_name()) {
            return true;
        }
        match resolved {
            // SOMETHING named a service, and it was not this one's signing name. The
            // only such signal that still belongs here is the host naming this
            // service's own ENDPOINT, which the model does express but the routing key
            // cannot carry: every portal operation is `@optionalAuth` with an empty
            // `@auth`, so an SDK sends it unsigned and the host-style fallback yields
            // only the endpoint's first label (`portal`), which is not the signing
            // name. Anything else — a SigV4 credential scope or an `X-Amz-Target`
            // naming another service, or another service's endpoint host — is somebody
            // else's request, and two claimants are an error to the router.
            // The first conjunct is the "the resolved key came from this HOST, not from
            // something stronger" guard: `is_portal_endpoint` already forces the host's
            // first label to be `portal`, so the comparison only ever rejects the case
            // where a credential scope or an `X-Amz-Target` named a DIFFERENT service
            // while the host was this endpoint.
            Some(key) => endpoint_host(parts).is_some_and(|host| {
                Some(key.as_str()) == host.split('.').next() && is_portal_endpoint(&host)
            }),
            // NOTHING in the request named a service at all: no credential scope, no
            // `X-Amz-Target` (`restJson1` has none), and a host that names no service
            // (the emulator's own endpoint, a bare IP, a proxy). That is the normal
            // shape of a portal request from a client pointed at this emulator, so the
            // request is attributed by its own modelled signature instead — the method
            // and path of one of this service's four `@http` bindings. Nothing weaker
            // is claimed: an unmodelled path reaches no service, exactly as before.
            None => serves_route(parts),
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
        // This service states a request it will not read as `InvalidRequestException`,
        // the error its model declares for exactly that — "a required parameter might
        // be missing or out of range" — rather than the framework's generic
        // `ValidationException`. The status (400) is the protocol's and matches the
        // modelled `@httpError`. Both the code and the message are replaced together.
        if let Refusal::Invalid(violations) = refusal {
            return Some(Refused {
                code: "InvalidRequestException".to_string(),
                message: aws_smithy_schema_server::envelope(violations)
                    .unwrap_or_else(|| "the request is not valid".to_string()),
            });
        }
        // Every other refusal class (an unauthenticated caller) keeps the
        // framework's default wording: this service's model declares no error for it.
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // A request that named no operation but DOES name one of this service's routes
        // got there by omitting a required, http-bound member: the REST router selects a
        // route only when every one of them is present.
        let absent = absent_required_members(req)?;

        // AUTHENTICATION FIRST. The portal has no other caller identity: everything it
        // answers comes out of the sign-in session the request's bearer token names, so
        // a request presenting no usable session is an unauthenticated request and is
        // refused as one — `UnauthorizedException`, with the status its model carries —
        // whatever else the request did or did not carry. That covers the request whose
        // absent member IS the credential: an absent credential is not a malformed
        // parameter, and it is exactly the case no handler's own gate can answer,
        // because the router never selects a route without it.
        if !self.presents_a_usable_session(req) {
            return crate::state::unauthorized_response(self.entry.protocol());
        }

        // The caller is authenticated, so what is left is a missing required parameter,
        // which this service's model has an error for (`InvalidRequestException`, "a
        // required parameter might be missing or out of range"), so it is stated as that
        // rather than as "not implemented".
        let violations: Vec<aws_smithy_schema_server::Violation> = absent
            .into_iter()
            .map(|member| aws_smithy_schema_server::Violation {
                path: member,
                kind: aws_smithy_schema_server::ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            })
            .collect();
        // Nothing was absent after all, so the miss is not about a required member and
        // the framework's own routing answer stands.
        let message = aws_smithy_schema_server::envelope(&violations)?;
        Some(self.entry.refusal_response(
            aws_smithy_schema_server::ProtocolError::Invalid(violations),
            Some(Refused {
                code: "InvalidRequestException".to_string(),
                message,
            }),
            req,
        ))
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

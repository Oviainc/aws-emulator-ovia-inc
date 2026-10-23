// SPDX-License-Identifier: CC0-1.0

//! The ACME HTTP surface an ACM ACME endpoint publishes: the wire
//! `AcmeEndpoint$EndpointUrl` advertises.
//!
//! # Not a modelled operation
//!
//! `smithy_query(query="service_info")` reports this service as `awsJson1_1`, and every
//! modelled operation is control plane: it creates, describes, tags and deletes ACME
//! endpoints, accounts and external account bindings. What an ACME client (certbot,
//! lego, acme.sh) then does with the URL `CreateAcmeEndpoint`/`DescribeAcmeEndpoint`
//! hand back is the ACME protocol itself (RFC 8555) — `GET <EndpointUrl>` for the
//! directory, `HEAD <newNonce>` for an anti-replay nonce, a JWS `POST` to the
//! resources the directory names. None of those is a modelled operation and none
//! carries an `X-Amz-Target`: they are addressed by host, path and HTTP method, so
//! nothing in the generated routing table can name them and they are reached through
//! the two seams [`crate::service::EmulatedService`] has for a request the model's
//! routing key cannot name.
//!
//! An ACME client holds no AWS credential, so the anonymous path matters: the router
//! reaches [`smithy_server_runtime::Service::override_routing_error`] BEFORE it
//! authenticates anything, which is what lets an unsigned ACME client be answered at
//! all. A request to the same URL that DOES present a SigV4 credential is named as an
//! operation ([`ErasedOperation`] below) so the emulator's own authenticator judges
//! that signature first — the same verification a control-plane request gets, rather
//! than a second signature check written into this service.
//!
//! # What is served
//!
//! * `GET <base>/directory` — the ACME directory resource (RFC 8555 §7.1.1): the
//!   resource map a client starts from, every URL in it minted by
//!   [`crate::state::acme_resource_url`] on the Region's enrollment host.
//! * `HEAD <base>/new-nonce` -> `200`, `GET <base>/new-nonce` -> `204` — a fresh
//!   anti-replay nonce in the `Replay-Nonce` header (RFC 8555 §7.2).
//!
//! The resources a client reaches with a JWS `POST` (`newAccount`, `newOrder`,
//! `revokeCert`, `keyChange`, and an account's own URL) are answered `501` with an ACME
//! problem document: this emulator verifies no JWS, so it can neither register an
//! account nor issue a certificate through ACME, and a `200` there would look like an
//! interaction nothing performed. Each is its own gap, to be implemented against its
//! own recorded behaviour.
//!
//! # The error envelope
//!
//! An ACME server states its failures as a problem document (RFC 8555 §6.7, RFC 7807),
//! not as the control plane's awsJson error shape: an ACME client reads the `type` urn
//! to decide what to do. [`problem`] is the one builder of that body here.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use crate::state;

/// The media type the ACME directory is served as (RFC 8555 §7.1.1 answers a JSON
/// object).
const JSON_CONTENT_TYPE: &str = "application/json";

/// The media type an ACME problem document travels as (RFC 8555 §6.7).
const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

/// The ACME error type for a request that does not address a resource this server
/// publishes: "the request message was malformed" (RFC 8555 §6.7 error registry).
const MALFORMED: &str = "urn:ietf:params:acme:error:malformed";

/// The ACME error type for a failure of the server itself: "the server experienced an
/// internal error" (RFC 8555 §6.7 error registry). It is also what an interaction this
/// emulator does not implement answers with, because that is a fact about the server
/// and not about the client's request.
const SERVER_INTERNAL: &str = "urn:ietf:params:acme:error:serverInternal";

/// The detail of a refusal of a path this server publishes nothing at. It carries
/// nothing from the request: what a client branches on is the status and the type urn.
const NO_SUCH_RESOURCE: &str = "This ACME server has no such resource";

/// The detail of a refusal of a URL no ACTIVE ACME endpoint serves.
///
/// One answer for an endpoint that does not exist, one that is still `CREATING`, and
/// one that is `DELETING` or `FAILED`, so probing the data plane tells a caller nothing
/// about another account's endpoints: "after creation, ACME clients can use the
/// endpoint URL" (`CreateAcmeEndpoint` doc), so before that there is nothing here.
const NO_ACTIVE_ENDPOINT: &str = "No active ACME endpoint serves this URL";

/// The detail of a failure to reach this service's own store.
const STORE_UNAVAILABLE: &str = "The ACM ACME endpoint store is unavailable";

/// The auth schemes a request to a published ACME URL that PRESENTS a credential is
/// verified under.
///
/// SigV4 is the only scheme this service's model declares, and ACM hands out no
/// data-plane credential of another kind. This is NOT a modelled operation, so there is
/// no modelled operation for the router to read schemes off: without this the router
/// would find no scheme for such a request and could not verify the credential it
/// carries.
const DATA_PLANE_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// One endpoint's ACME HTTP surface, as the one [`ErasedOperation`] the service holds
/// for it.
///
/// Owns the shared engine exactly as a generated operation does, so every answer is
/// computed from the SAME per-service store the control plane's handlers write: the
/// directory of an endpoint is served only while that endpoint's own record says it is
/// `ACTIVE`, and an endpoint `DeleteAcmeEndpoint` removed serves nothing.
pub struct AcmeHttpSurface<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> AcmeHttpSurface<Eng> {
    /// Build the ACME surface from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for AcmeHttpSurface<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req) })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(DATA_PLANE_AUTH_SCHEMES)
    }
}

impl<Eng: Send + Sync + 'static> AcmeHttpSurface<Eng> {
    /// Serve one request to a published ACME URL.
    ///
    /// The host names the Region, the path names the endpoint and the resource, and the
    /// method names the interaction — which is how an ACME server is addressed; there
    /// is no operation name on this wire.
    pub fn serve(&self, req: &RtRequest) -> RtResponse {
        let Some(region) = enrollment_region_of_request(req) else {
            // Unreachable: the service claims a request for this surface only when its
            // host IS an enrollment host. Answering the ACME envelope rather than
            // panicking keeps the handler total.
            return problem(404, MALFORMED, NO_SUCH_RESOURCE);
        };
        let Some(route) = state::parse_acme_request_path(request_path(req.uri())) else {
            return problem(404, MALFORMED, NO_SUCH_RESOURCE);
        };
        let Some(resource) = resource_of(&route.resource) else {
            return problem(404, MALFORMED, NO_SUCH_RESOURCE);
        };
        let Some(store) = state::service_state(&*self.engine) else {
            return problem(500, SERVER_INTERNAL, STORE_UNAVAILABLE);
        };
        // Serving an endpoint's ACME surface means serving it from the endpoint's own
        // record, up to date with the clock: the `CREATING` -> `ACTIVE` transition the
        // create owed it was due at this instant whether or not anybody called, which
        // is what makes a client that waited for activation able to enrol.
        let serving = {
            let mut store = store.lock();
            store.materialize_due(SystemTime::now());
            store
                .acme_endpoint_in(region, &route.endpoint_id)
                .is_some_and(|endpoint| endpoint.status == state::ACME_ENDPOINT_STATUS_ACTIVE)
        };
        if !serving {
            return problem(404, MALFORMED, NO_ACTIVE_ENDPOINT);
        }

        let directory_url = state::acme_endpoint_url(region, &route.endpoint_id);
        match (req.method(), resource) {
            // The directory is the one resource an ACME client fetches with a plain
            // `GET` (RFC 8555 §7.1.1); everything else on this wire is a JWS `POST`.
            ("GET", AcmeResource::Directory) => directory(region, &route.endpoint_id),
            // `newNonce`: "the server MUST respond with a 200 (OK) to HEAD requests and
            // 204 (No Content) to GET requests" (RFC 8555 §7.2).
            ("HEAD", AcmeResource::NewNonce) => nonce(200, &directory_url),
            ("GET", AcmeResource::NewNonce) => nonce(204, &directory_url),
            ("POST", AcmeResource::Jws) => not_implemented(&route.resource, &directory_url),
            (_, resource) => method_not_allowed(resource),
        }
    }
}

/// Which ACME resource of an endpoint a path's remainder names, or `None` when the
/// endpoint publishes no such resource.
///
/// The names are the ones [`crate::state::acme_resource_url`] mints the directory's
/// own URLs from, so what is recognised here is exactly what the directory advertises.
fn resource_of(resource: &str) -> Option<AcmeResource> {
    match resource {
        state::ACME_DIRECTORY_RESOURCE => Some(AcmeResource::Directory),
        state::ACME_NEW_NONCE_RESOURCE => Some(AcmeResource::NewNonce),
        state::ACME_NEW_ACCOUNT_RESOURCE
        | state::ACME_NEW_ORDER_RESOURCE
        | state::ACME_REVOKE_CERTIFICATE_RESOURCE
        | state::ACME_KEY_CHANGE_RESOURCE => Some(AcmeResource::Jws),
        // An account's own URL, which a client reaches with a POST-as-GET (RFC 8555
        // §7.3) — the same JWS wire the resources above are on.
        other => account_url_resource(other).map(|_| AcmeResource::Jws),
    }
}

/// The account id in an account resource path (`acct/<id>`), or `None` when the path is
/// not one [`crate::state::acme_account_url`] mints.
fn account_url_resource(resource: &str) -> Option<&str> {
    let id = resource.strip_prefix(state::ACME_ACCOUNT_RESOURCE)?;
    let id = id.strip_prefix('/')?;
    // An account id is minted by this service and carries no `/`, so a deeper path is
    // not an account URL.
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// One resource of an endpoint's ACME surface, as the HTTP wire addresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcmeResource {
    /// The directory resource, fetched with `GET` (RFC 8555 §7.1.1).
    Directory,
    /// The `newNonce` resource (RFC 8555 §7.2).
    NewNonce,
    /// A resource a client reaches with a JWS `POST`: `newAccount`, `newOrder`,
    /// `revokeCert`, `keyChange`, or an account's own URL.
    Jws,
}

impl AcmeResource {
    /// The methods this resource is addressed with, as the `Allow` header of a refusal
    /// of any other method lists them (RFC 9110 §15.5.6 requires that header).
    fn allowed_methods(self) -> &'static str {
        match self {
            Self::Directory => "GET",
            Self::NewNonce => "HEAD, GET",
            Self::Jws => "POST",
        }
    }
}

/// The ACME directory resource of one endpoint: the map of the resources a client
/// starts from (RFC 8555 §7.1.1).
///
/// Every URL in it is built by [`crate::state::acme_resource_url`] from the endpoint's
/// own id and Region — the same function that minted the `EndpointUrl` the client
/// fetched this document from — so the map a client follows stays on the host this
/// service serves, and nothing in it comes from the request.
///
/// The five members RFC 8555 §7.1.1 requires are present. The optional ones are not
/// advertised: `newAuthz` belongs to pre-authorization, which no modelled operation
/// configures, and `meta` (terms of service, website, CAA identities, whether an
/// external account binding is required) would be claims about this CA that neither
/// ACM's model nor its documentation states.
fn directory(region: &str, endpoint_id: &str) -> RtResponse {
    let resource = |name: &str| state::acme_resource_url(region, endpoint_id, name);
    let document = ::serde_json::json!({
        "newNonce": resource(state::ACME_NEW_NONCE_RESOURCE),
        "newAccount": resource(state::ACME_NEW_ACCOUNT_RESOURCE),
        "newOrder": resource(state::ACME_NEW_ORDER_RESOURCE),
        "revokeCert": resource(state::ACME_REVOKE_CERTIFICATE_RESOURCE),
        "keyChange": resource(state::ACME_KEY_CHANGE_RESOURCE),
    });
    response(200, JSON_CONTENT_TYPE, document.to_string())
}

/// The `newNonce` answer: an empty body carrying a fresh nonce.
///
/// RFC 8555 §7.2 fixes all three headers: the `Replay-Nonce` field carries the nonce,
/// the response "MUST include a Cache-Control header field with the no-store
/// directive", and §7.1 asks for the `Link` field pointing at the directory.
fn nonce(status: u16, directory_url: &str) -> RtResponse {
    let mut answer = response(status, JSON_CONTENT_TYPE, String::new());
    answer.headers_mut().insert("replay-nonce", mint_nonce());
    answer.headers_mut().insert("cache-control", "no-store");
    index_link(&mut answer, directory_url);
    answer
}

/// A fresh anti-replay nonce.
///
/// 128 bits of fresh randomness per call, so no two requests are answered the same
/// nonce, rendered as hex — which is within the base64url alphabet RFC 8555 §6.5.1
/// requires a nonce to be spelled in, and whose length is a multiple of four, so a
/// client that base64url-decodes what it was handed can.
///
/// Nothing stores it. A nonce is spent by the JWS of a POST to one of the resources
/// the directory names, and every one of those answers `501` here, so no request can
/// ever present a nonce back: a retained nonce set would be write-only state that
/// nothing could retire. Whatever implements those interactions brings the store that
/// makes each nonce single-use with it.
fn mint_nonce() -> String {
    ::hex::encode(::uuid::Uuid::new_v4().as_bytes())
}

/// The refusal of an ACME interaction this emulator does not implement.
///
/// `501` with a problem document, never an empty `200`: registering an account,
/// ordering a certificate, revoking one or rolling an account key over all require
/// verifying the client's JWS signature over the request, which this emulator does
/// not do — so there is no account, order or certificate behind such a call, and a
/// success would report an interaction nothing performed.
fn not_implemented(resource: &str, directory_url: &str) -> RtResponse {
    let detail = format!(
        "This ACME server does not implement the {} interaction yet",
        bounded(resource)
    );
    let mut answer = problem(501, SERVER_INTERNAL, &detail);
    index_link(&mut answer, directory_url);
    answer
}

/// The refusal of a method the addressed resource is not reached with, naming the
/// methods it is (RFC 9110 §15.5.6).
fn method_not_allowed(resource: AcmeResource) -> RtResponse {
    let detail = format!(
        "This ACME resource is addressed with {}",
        resource.allowed_methods()
    );
    let mut answer = problem(405, MALFORMED, &detail);
    answer
        .headers_mut()
        .insert("allow", resource.allowed_methods());
    answer
}

/// Add the `Link` field pointing at the endpoint's directory, which RFC 8555 §7.1 asks
/// every response on this wire to carry so a client can always find the resource map.
fn index_link(answer: &mut RtResponse, directory_url: &str) {
    answer
        .headers_mut()
        .insert("link", format!("<{directory_url}>;rel=\"index\""));
}

/// An ACME failure, as the problem document an ACME server answers with (RFC 8555
/// §6.7): the `type` urn a client branches on and a `detail` sentence for a human.
fn problem(status: u16, problem_type: &str, detail: &str) -> RtResponse {
    let body = ::serde_json::json!({
        "type": problem_type,
        "detail": detail,
        "status": status,
    });
    response(status, PROBLEM_CONTENT_TYPE, body.to_string())
}

/// One response on this wire: the status, the media type, and the body.
fn response(status: u16, content_type: &str, body: String) -> RtResponse {
    let mut answer = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    answer
        .headers_mut()
        .insert("content-type", content_type.to_string());
    answer
}

/// The longest run of a request's own path this surface quotes back in a message.
const MAX_QUOTED_LEN: usize = 64;

/// `value` as a message quotes it: ASCII graphic characters only and at most
/// [`MAX_QUOTED_LEN`] of them, so a message cannot carry control characters or an
/// unbounded run of a caller's own text.
fn bounded(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(MAX_QUOTED_LEN)
        .collect()
}

/// Whether `req` addresses an ACME enrollment host, i.e. one of the hostnames this
/// service PUBLISHES in `AcmeEndpoint$EndpointUrl`.
pub fn addresses_acme_surface(req: &RtRequest) -> bool {
    enrollment_region_of_request(req).is_some()
}

/// The Region whose ACME enrollment host a runtime-API request addresses, or `None`
/// for any other host.
fn enrollment_region_of_request(req: &RtRequest) -> Option<&str> {
    let host = req
        .headers()
        .get("host")
        .or_else(|| authority_of(req.uri()))?;
    state::acme_enrollment_region(host)
}

/// The Region whose ACME enrollment host a buffered request addresses, or `None` for
/// any other host.
///
/// Reads the `Host` header an HTTP/1 request carries, else the authority an HTTP/2
/// request carries on its URI (`http` parses `:authority` onto the URI rather than
/// exposing it as a header).
pub fn enrollment_region_of_parts(parts: &http::request::Parts) -> Option<&str> {
    let host = match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => host,
        None => parts.uri.authority().map(http::uri::Authority::as_str)?,
    };
    state::acme_enrollment_region(host)
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization`
/// header a signed SDK request carries, or in the `X-Amz-Credential` query parameter a
/// presigned URL carries.
///
/// Only PRESENCE is read. Whether the signature over that credential is correct is the
/// emulator's own check, never a service's.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    if req
        .headers()
        .get("authorization")
        .is_some_and(|value| value.contains("Credential="))
    {
        return true;
    }
    request_query(req.uri())
        .split('&')
        .any(|pair| pair.starts_with("X-Amz-Credential="))
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
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

/// The query component of a request URI: everything after the first `?`.
fn request_query(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

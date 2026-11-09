// SPDX-License-Identifier: CC0-1.0

//! The MicroVM endpoint: the ADDRESS a MicroVM is published on, and what this
//! emulator answers there.
//!
//! `RunMicrovmResponse$endpoint` and `GetMicrovmResponse$endpoint` are documented as
//! "The HTTPS endpoint URL for communicating with the MicroVM. Include a valid
//! authentication token in the X-aws-proxy-auth header when sending requests." A
//! published endpoint is therefore an ADDRESS A CLIENT USES, so this emulator
//! publishes one it answers on itself — the core's own loopback HTTPS listener
//! (`engine.listeners().http(..)`, which owns the socket, the bind interface and the
//! TLS) with the MicroVM named in the path — rather than a hostname that resolves off
//! this machine. The address is opened once per store and reported by the control
//! plane, so every MicroVM of one emulator shares the listener and differs in its path.
//!
//! What the address answers:
//!
//! * a path that names no MicroVM this store holds, or one that can no longer be
//!   connected to ([`super::is_connectable_microvm_state`]) — nothing is published
//!   there, so `404`;
//! * no `X-aws-proxy-auth` token, or one this store does not hold live for the MicroVM
//!   the path names (another MicroVM's, one the clock has expired, or one whose
//!   `allowedPorts` do not cover the port the MicroVM's image publishes its listener
//!   on — or name particular ports where that image publishes none, a scope this
//!   emulator cannot establish) — `403`, the answer `CreateMicrovmAuthToken`'s token is
//!   what avoids;
//! * an authenticated request — the GUEST's own HTTP response belongs here, and this
//!   emulator runs no MicroVM guest (a guest is the image's code artifact, which needs
//!   a container runtime this emulator does not have), so the honest answer is `501`.
//!   Nothing is synthesised in the guest's name.
//!
//! A token is held as a DIGEST, never as the token itself: the caller keeps the only
//! copy of the secret, and this store keeps just enough to recognise it again.
//!
//! `Hooks$port` ("the port number on which the hooks listener runs") is the ONE port
//! this API records for a guest, so it is the only port a token's `allowedPorts` can be
//! established against here: the ports a guest actually binds are a property of the
//! image's code artifact, which no operation of this service reports and which nothing
//! in this emulator runs. A token scoped to ports that cannot be established is
//! refused rather than treated as granted.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::{
    is_connectable_microvm_state, ComAmazonawsLambdamicrovmsLambdamicrovmsState as ServiceState,
    MAX_MAXIMUM_DURATION_IN_SECONDS,
};
use crate::ops::types::PortSpecification;

/// The header a client presents its authentication token in, as
/// `CreateMicrovmAuthTokenResponse$authToken` documents: "Use the value at key
/// \"X-aws-proxy-auth\" as the header value when connecting to the MicroVM endpoint."
pub const AUTH_TOKEN_HEADER: &str = "X-aws-proxy-auth";

/// The first path segment of a published endpoint, ahead of the `microvmId` — so one
/// listener addresses every MicroVM of the store and a guest path appends to the
/// endpoint exactly as it does against AWS (`<endpoint><path>`).
pub const MICROVM_PATH_SEGMENT: &str = "microvms";

/// How many live authentication tokens one store keeps.
///
/// `CreateMicrovmAuthToken` mints a token per call and the model puts no ceiling on
/// how often it may be called, so the digests are capped as well as expired: at the
/// cap the token closest to expiry makes room, the same way an idempotency record does
/// ([`ServiceState::record_minted`]).
pub const MAX_LIVE_AUTH_TOKENS: usize = 1_000;

/// What a published endpoint answers for a path that names no MicroVM of this store.
/// One sentence carrying nothing from the request, so a caller cannot probe with it.
const NO_MICROVM_AT_THIS_ADDRESS: &str = "No MicroVM is published at this address.\n";

/// What it answers a request that presents no live authentication token for the
/// MicroVM the address names. One sentence for every way the token fails, so the
/// answer does not say which.
const AUTH_TOKEN_REQUIRED: &str = "Present a valid authentication token for this \
                                   MicroVM in the X-aws-proxy-auth header. Create one \
                                   with CreateMicrovmAuthToken.\n";

/// What it answers an AUTHENTICATED request: the guest's own response belongs here,
/// and no guest ran — see this module's header.
const NO_MICROVM_GUEST: &str = "This MicroVM endpoint is not implemented: the \
                                emulator runs no MicroVM guest, so the guest's own \
                                response cannot be served.\n";

/// One live authentication token for one MicroVM's endpoint, held in
/// [`ServiceState::auth_tokens`] under [`auth_token_digest`] of the token.
#[derive(Debug, Clone)]
pub struct MicrovmAuthToken {
    /// The REGION whose endpoint authorised the mint — the partition holding the MicroVM
    /// the token grants access to. Every resource of this service is regional
    /// ([`super::regions`]), so the identity a presented token is matched on is this
    /// region AND [`Self::microvm_id`] together: a token can authenticate nothing in
    /// another region's partition.
    pub region: String,
    /// The `microvmId` the token grants access to — "The ID of the MicroVM to create
    /// an authentication token for", so it authenticates at that MicroVM's address and
    /// at no other.
    pub microvm_id: String,
    /// `allowedPorts`: "The ports on the MicroVM endpoint the token grants access to."
    pub allowed_ports: Vec<PortSpecification>,
    /// When `expirationInMinutes` runs out, never later than the MicroVM's own
    /// lifetime ([`ServiceState::record_auth_token`]).
    pub expires_at: Instant,
}

/// The digest a token is recognised by. The token itself is the caller's secret and is
/// never stored, logged or returned again, so the store keeps only this.
pub fn auth_token_digest(token: &str) -> String {
    ::hex::encode(Sha256::digest(token.as_bytes()))
}

/// This store's MicroVM endpoint listener: the one address every MicroVM of the store
/// is published on.
///
/// The state OWNS the listener: dropping it closes the port, so it must outlive every
/// endpoint handed out, which is the whole life of the state.
pub struct MicrovmProxy {
    /// `https://<loopback host>:<port>`, the base of every published endpoint.
    base_url: String,
    /// Held only to keep the port open; the core owns the socket and its TLS.
    _listener: ::emulator_server::ResourceListener,
}

impl MicrovmProxy {
    /// The base every endpoint of this store is built on.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

/// Written by hand rather than derived: [`MicrovmProxy`] holds a core
/// `ResourceListener`, which is not `Debug`. The listener is reported by the address it
/// announced, which is what a reader needs.
impl std::fmt::Debug for MicrovmProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicrovmProxy")
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// The base URL of this store's MicroVM endpoint listener, opening it on first use.
///
/// Opening it changes no MicroVM and is idempotent: the first listener is kept, so a
/// MicroVM's published endpoint is the same address for the life of the store.
/// `None` when the listener could not be opened at all, which leaves the caller to
/// answer with a modelled server error rather than to publish an address nothing
/// serves.
pub async fn ensure_base_url<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ServiceState>,
) -> Option<String> {
    // The guard is taken and dropped before the await below: a lock held across an
    // await would make this future non-`Send`.
    let existing = {
        let store = state.lock();
        store
            .microvm_proxy
            .as_ref()
            .map(|proxy| proxy.base_url.clone())
    };
    if let Some(base_url) = existing {
        return Some(base_url);
    }

    let listener = super::shared_engine(engine)?
        .listeners()
        .http(
            MicrovmEndpointServer {
                state: state.clone(),
            },
            ::emulator_server::HttpPolicy::default(),
        )
        .await
        .ok()?;
    let base_url = format!("https://{}", authority(&listener));

    let mut store = state.lock();
    // Another request may have opened one while this one was binding; keep the first,
    // so a published endpoint never changes under a client. Dropping the loser closes
    // its port.
    if let Some(proxy) = &store.microvm_proxy {
        return Some(proxy.base_url.clone());
    }
    store.microvm_proxy = Some(MicrovmProxy {
        base_url: base_url.clone(),
        _listener: listener,
    });
    Some(base_url)
}

/// The endpoint the MicroVM `microvm_id` is published on, on the base
/// [`ensure_base_url`] returned.
pub fn endpoint_url(base_url: &str, microvm_id: &str) -> String {
    format!("{base_url}/{MICROVM_PATH_SEGMENT}/{microvm_id}")
}

/// The `host:port` an endpoint addresses the listener by, with an IPv6 host bracketed
/// as RFC 3986 requires (`[::1]:8443`), so the URL parses wherever the operator bound
/// it.
fn authority(listener: &::emulator_server::ResourceListener) -> String {
    let host = listener.host();
    let port = listener.port();
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Serves every MicroVM endpoint of one store on the core's listener. The core runs
/// the browser checks and owns CORS; this answers the address the path names.
struct MicrovmEndpointServer {
    state: ::engine::ServiceState<ServiceState>,
}

impl ::emulator_server::HttpHandler for MicrovmEndpointServer {
    fn handle(
        &self,
        req: ::http::Request<Vec<u8>>,
    ) -> Pin<Box<dyn Future<Output = ::http::Response<Vec<u8>>> + Send>> {
        let state = self.state.clone();
        Box::pin(async move { serve_request(&state, &req) })
    }
}

/// One request to a published MicroVM endpoint, answered from this store — the
/// decision the listener's handler serves, and the one this service's tests drive.
///
/// The MicroVM is resolved FIRST and the token second, which is the order the real
/// address works in: only an address that names a MicroVM answers at all, and then the
/// token decides. The one write is the clock's own
/// ([`ServiceState::materialize_due`]), owed at this instant whether or not anybody
/// connected, so reaching an endpoint changes nothing a later caller sees.
pub fn serve_request(
    state: &::engine::ServiceState<ServiceState>,
    req: &::http::Request<Vec<u8>>,
) -> ::http::Response<Vec<u8>> {
    let head = req.method() == ::http::Method::HEAD;
    let presented = req
        .headers()
        .get(AUTH_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(auth_token_digest);

    let Some(addressed) = addressed_microvm(req.uri().path()) else {
        return endpoint_response(head, 404, NO_MICROVM_AT_THIS_ADDRESS);
    };

    let now = Instant::now();
    let mut store = state.lock();
    store.materialize_due(now);

    // The address carries the MicroVM's own id rather than a region, so the record is
    // resolved wherever this store holds it (see `super::regions`) — and the region that
    // holds it is half the identity a presented token has to match.
    let Some((region, microvm_id)) = store
        .microvm_anywhere(&addressed)
        .filter(|(_, vm)| is_connectable_microvm_state(&vm.state))
        .map(|(region, vm)| (region.to_string(), vm.microvm_id.clone()))
    else {
        return endpoint_response(head, 404, NO_MICROVM_AT_THIS_ADDRESS);
    };

    let guest_port = store.microvm_guest_port(&region, &microvm_id);
    let authenticated = presented
        .and_then(|digest| store.auth_tokens.get(&digest))
        .is_some_and(|token| {
            // The token's identity is the REGION and the `microvmId` together: a token
            // minted against one region's partition grants nothing at another's.
            token.region == region
                && token.microvm_id == microvm_id
                && token.expires_at > now
                && grants_port(&token.allowed_ports, guest_port)
        });
    drop(store);

    if !authenticated {
        // Every attempt on a published endpoint is attributable: the MicroVM addressed
        // and the outcome. The presented token is NEVER logged — only the caller holds
        // it — and neither is the path, which carries whatever the client sent.
        ::tracing::info!(
            audit = true,
            operation = "MicrovmEndpointRequest",
            outcome = "refused",
            microvm_id = %microvm_id,
            "MicroVM endpoint request without a live authentication token"
        );
        return endpoint_response(head, 403, AUTH_TOKEN_REQUIRED);
    }
    endpoint_response(head, 501, NO_MICROVM_GUEST)
}

/// The `microvmIdentifier` a request path addresses, or `None` when the path names no
/// MicroVM at all. The segments after it are the guest's own path, which only the
/// guest could answer.
fn addressed_microvm(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix('/')?
        .strip_prefix(MICROVM_PATH_SEGMENT)?
        .strip_prefix('/')?;
    let identifier = match rest.split_once('/') {
        Some((identifier, _guest_path)) => identifier,
        None => rest,
    };
    (!identifier.is_empty()).then(|| identifier.to_string())
}

/// Whether `allowed` grants access to the port a MicroVM publishes its guest listener
/// on — each arm of the `PortSpecification` union as the model declares it (`port`, a
/// `range` inclusive of both endpoints, or `allPorts`).
///
/// Where the MicroVM's image declares no listener port there is no port to match the
/// scope against, so a token scoped to particular ports grants nothing that can be
/// established and is refused; only a token for EVERY port covers such a request. A
/// scope this emulator cannot establish is never read as a granted one.
fn grants_port(allowed: &[PortSpecification], guest_port: Option<i32>) -> bool {
    allowed.iter().any(|spec| match (spec, guest_port) {
        (PortSpecification::AllPorts(_), _) => true,
        (PortSpecification::Port(allowed), Some(port)) => *allowed == port,
        (PortSpecification::Range(range), Some(port)) => {
            range.start_port <= port && port <= range.end_port
        }
        (PortSpecification::Port(_) | PortSpecification::Range(_), None) => false,
    })
}

/// One response from a published MicroVM endpoint.
///
/// The endpoint is plain HTTPS rather than a modelled operation — it is the address the
/// MicroVM is reached on — so it answers in its own plain-text form instead of this
/// service's restJson1 envelope. Every message is a constant of this module, never
/// built from the request.
///
/// A `HEAD` carries the metadata of what a `GET` of the same address would answer — its
/// status and its length — without the body, which is what the method means.
fn endpoint_response(head: bool, status: u16, message: &'static str) -> ::http::Response<Vec<u8>> {
    let body = if head {
        Vec::new()
    } else {
        message.as_bytes().to_vec()
    };
    ::http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("content-length", message.len().to_string())
        .body(body)
        // Every argument is this function's own constant, so the builder cannot fail;
        // answer a bare 500 rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Vec::new());
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}

impl ServiceState {
    /// Record the token `CreateMicrovmAuthToken` just minted for `microvm_id`: its
    /// digest, what it grants and when it stops being accepted.
    ///
    /// `expirationInMinutes` is "the duration in minutes before the authentication
    /// token expires" and the model gives it no maximum, so the stored expiry is capped
    /// by the MicroVM's OWN lifetime: a MicroVM is terminated by the platform at
    /// `maximumDurationInSeconds` and a terminated MicroVM accepts no connection, so a
    /// token cannot grant access past it.
    pub fn record_auth_token(
        &mut self,
        region: &str,
        microvm_id: &str,
        token: &str,
        expiration_in_minutes: i32,
        allowed_ports: Vec<PortSpecification>,
        now: Instant,
    ) -> Instant {
        let requested_seconds = u64::try_from(expiration_in_minutes)
            .unwrap_or_default()
            .saturating_mul(60);
        let platform_limit = Duration::from_secs(
            u64::try_from(MAX_MAXIMUM_DURATION_IN_SECONDS).unwrap_or_default(),
        );
        let microvm_end = self
            .microvm(region, microvm_id)
            .and_then(|vm| vm.expiry_due_at)
            .or_else(|| now.checked_add(platform_limit))
            .unwrap_or(now);
        let expires_at = now
            .checked_add(Duration::from_secs(requested_seconds))
            .map_or(microvm_end, |requested| requested.min(microvm_end));

        if self.auth_tokens.len() >= MAX_LIVE_AUTH_TOKENS {
            let evict = self
                .auth_tokens
                .iter()
                .min_by_key(|(_, token)| token.expires_at)
                .map(|(digest, _)| digest.clone());
            if let Some(digest) = evict {
                self.auth_tokens.remove(&digest);
            }
        }
        self.auth_tokens.insert(
            auth_token_digest(token),
            MicrovmAuthToken {
                region: region.to_string(),
                microvm_id: microvm_id.to_string(),
                allowed_ports,
                expires_at,
            },
        );
        expires_at
    }

    /// The port a MicroVM's guest publishes its listener on: `Hooks$port`, "the port
    /// number on which the hooks listener runs", of the image version the MicroVM runs.
    /// `None` when that image version declares none.
    ///
    /// Both records are read in `region`: a MicroVM and the image it runs are resources
    /// of the same regional partition.
    pub fn microvm_guest_port(&self, region: &str, microvm_id: &str) -> Option<i32> {
        let vm = self.microvm(region, microvm_id)?;
        let image = self.image(region, &vm.image_arn)?;
        image
            .versions
            .get(&vm.image_version)?
            .hooks
            .as_ref()?
            .port
    }

    /// Forget every authentication token the clock has expired, and every one whose
    /// MicroVM can no longer be connected to — both owed at the same instant whether
    /// or not anybody called. Applied by [`ServiceState::materialize_due`].
    ///
    /// A token is kept only while the MicroVM of ITS OWN region is still connectable, so
    /// an identically-named MicroVM of another partition keeps nothing alive.
    pub(super) fn expire_auth_tokens(&mut self, now: Instant) {
        let connectable: BTreeSet<(String, String)> = self
            .regions
            .iter()
            .flat_map(|(region, regional)| {
                regional
                    .microvms
                    .values()
                    .filter(|vm| is_connectable_microvm_state(&vm.state))
                    .map(|vm| (region.clone(), vm.microvm_id.clone()))
            })
            .collect();
        self.auth_tokens.retain(|_, token| {
            token.expires_at > now
                && connectable.contains(&(token.region.clone(), token.microvm_id.clone()))
        });
    }
}

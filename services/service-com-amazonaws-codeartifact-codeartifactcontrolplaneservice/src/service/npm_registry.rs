// SPDX-License-Identifier: CC0-1.0

//! The npm REGISTRY a repository's npm endpoint serves — the package data plane behind the
//! URL `GetRepositoryEndpoint` hands out.
//!
//! A CodeArtifact repository is reached by a package manager, not by an AWS SDK: the
//! endpoint that operation returns speaks the native protocol of the format it was asked
//! for ("the URL of a repository endpoint … for the specified package format"), and for npm
//! that protocol is an npm registry — `GET /-/ping`, `PUT /<package>` to publish,
//! `GET /<package>` for the packument, and a `GET` of a version's tarball path for its
//! bytes (AWS CodeArtifact user guide, *Use CodeArtifact with npm*). The host that URL
//! names is the DOMAIN's, not this service's endpoint, so the emulator's front door could
//! resolve no target service from such a request and answered `404 UnknownService` — for
//! the one address this service told the client to use. This module is what claims that
//! traffic and answers it.
//!
//! Two addressings, because a client on the machine this emulator runs on resolves neither
//! the domain's published host nor its DNS:
//!
//! * HOST-STYLE — the published endpoint host itself,
//!   `<domain>-<account>.d.codeartifact.<region>.<suffix>`, with the endpoint's own
//!   `/<format>/<repository>/` path. The domain and the account are read out of the host's
//!   first label ([`endpoint_host_domain`]), so the request addresses exactly the domain
//!   whose endpoint it was sent to.
//! * PATH-STYLE — the same `/<format>/<repository>/` path on the emulator's own endpoint,
//!   which is the registry URL a client configures when it cannot resolve the published
//!   host. The path carries no domain, so it is read only when exactly ONE domain this
//!   emulator holds has a repository of that name: two would make the address ambiguous,
//!   and a request is left to whatever else it might have been addressed to rather than
//!   claimed on a guess.
//!
//! Everything it serves comes out of the SAME records the control plane writes
//! ([`crate::state`]): a version published over this endpoint is the version
//! `DescribePackageVersion` describes and `ListPackageVersionAssets` lists, with the npm
//! manifest and the tarball as its two assets, and a version whose status the control plane
//! moved is served (or refused) here accordingly. There is no second store.
//!
//! ## What authenticates a request
//!
//! The credential an npm client presents is the bearer token `GetAuthorizationToken` minted
//! (`//<host>/<path>:_authToken=<token>` in an npmrc), so this module resolves it against
//! the tokens this service remembers minting
//! ([`crate::state::ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState::authorization_token`]):
//! a token this service did not mint, one the clock has expired, one scoped to another
//! domain, and no token at all are each `401`. The token is domain-scoped — "The name of
//! the domain that is in scope for the generated authorization token" — so it authenticates
//! every repository of that domain and no repository outside it.
//!
//! ## What it does not serve yet
//!
//! * Only the `npm` format. The other `PackageFormat` values publish over their own
//!   protocols (a PyPI index, a Maven repository), each of which is its own body of
//!   behaviour; a request under another format's path is not claimed here at all rather
//!   than answered in npm's envelope.
//! * No EXTERNAL-connection resolution: a package this domain holds nowhere is not fetched
//!   from a public registry, which is the behaviour of `AssociateExternalConnection` and
//!   belongs with the issue that owns it. UPSTREAM repositories inside the domain ARE
//!   resolved, and what they resolve is retained here ([`retain_from_upstreams`]).

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::serde_json::{json, Map, Value};
use ::std::sync::Arc;
use ::std::time::SystemTime;

use ::service_common::data_plane_addressing::{first_segment, host_only, names_a_service};

use crate::state::package_version_events::{VersionChange, VersionChanges, OPERATION_CREATED};
use crate::state::{
    service_state, Asset, ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState as CaState,
    Package, PackageVersion,
};

/// The one `PackageFormat` this plane serves ([`crate::state::PACKAGE_FORMAT_NPM`]). It is
/// also the first segment of the endpoint path CodeArtifact publishes for it.
pub const PACKAGE_FORMAT_NPM: &str = crate::state::PACKAGE_FORMAT_NPM;

/// The infix every published repository-endpoint host carries between the domain label and
/// the region: `<domain>-<account>.d.codeartifact.<region>.<suffix>` (AWS CodeArtifact user
/// guide). Reading it is what distinguishes a repository endpoint's host from this
/// service's own API endpoint.
const ENDPOINT_HOST_INFIX: &str = "d.codeartifact.";

/// How many digits an account id carries (`AccountId` is modelled `^[0-9]{12}$`), which is
/// what makes the domain label of an endpoint host splittable: everything before the last
/// `-` is the domain name, and those digits are the owning account.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The extension of the file name npm's own tarball URL carries: the URL names
/// `<package>-<version>.tgz`, while the ASSET the version is stored as carries the format's
/// fixed name ([`crate::state::NPM_TARBALL_ASSET`]).
const NPM_TARBALL_EXTENSION: &str = ".tgz";

/// The segment npm reserves for the registry's own routes, ahead of a route name
/// (`/-/ping`) and inside a package's path ahead of its tarball (`/<package>/-/<file>`).
const NPM_RESERVED_SEGMENT: &str = "-";

/// The registry route an npm client checks a registry (and its credential) with:
/// `GET /-/ping`, which answers `200` for a credential the registry accepts.
const PING_ROUTE: &str = "ping";

/// The media type a packument and every error body carry.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The media type a tarball carries: opaque bytes, which is what npm fetches it as.
const TARBALL_MEDIA_TYPE: &str = "application/octet-stream";

/// The longest body npm's dist-tag route carries: the version a tag is to name, as a JSON
/// string. `PackageVersion` is modelled length 1..255, so a body past this names no version
/// this service could hold.
const MAX_DIST_TAG_BODY_BYTES: usize = 512;

/// The longest METADATA document this plane retains for one version.
///
/// A version's `package.json` is kilobytes in practice; the bound is the emulator's own,
/// like every other ceiling in this store, and is what keeps the text a publish retains
/// from being the caller's choice of size. The aggregate of what is retained is bounded
/// separately ([`crate::state::MAX_STORED_ASSET_BYTES`], checked over
/// `stored_content_bytes`).
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// The longest publish document this plane reads.
///
/// A publish document carries its tarballs base64-encoded, which is 4 characters per 3
/// bytes, so the ceiling is the per-asset ceiling
/// ([`crate::state::MAX_ASSET_BYTES`]) with room for that encoding and for the manifest
/// beside it. The emulator's OWN bound, like every other ceiling in this service's store:
/// it is what stops one request materialising unbounded bytes before any of them is
/// checked against a quota.
const MAX_PUBLISH_DOCUMENT_BYTES: usize = crate::state::MAX_ASSET_BYTES * 2;

/// Whether this plane serves the request these `parts` came from — read by
/// `Service::can_handle`, so a request addressed to a repository endpoint this service
/// holds reaches this service instead of the front door's `UnknownService`.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::http::request::Parts,
) -> bool {
    resolve(engine, &Addressed::of_parts(parts)).is_some()
}

/// [`claims_parts`] for the runtime-API request the serving seam is handed, resolved by the
/// same rule.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let uri = parsed_uri(req.uri());
    resolve(engine, &Addressed::of_request(req, &uri)).is_some()
}

/// Answer a request this plane claimed, out of the records the control plane holds.
///
/// Reached from the ONE operation this service serves a repository endpoint's traffic with
/// ([`NpmRegistryEndpoint`]), which the routing seam resolves before the request is
/// authenticated — which is what lets a client carrying an npm bearer token rather than a
/// SigV4 signature be answered at all.
///
/// Every state change it makes is RECORDED on the store
/// ([`CaState::record_version_change`]); the operation announces them once this function
/// has returned and the store's lock is released.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let uri = parsed_uri(req.uri());
    let addressed = Addressed::of_request(req, &uri);
    let Some(resolved) = resolve(engine, &addressed) else {
        // The addressing resolved for the claim and no longer does (the repository was
        // deleted by another request in between), so there is no registry at this address.
        return rendered(&not_found(
            "the repository endpoint was not found".to_string(),
        ));
    };
    let Some(state) = service_state(engine) else {
        return rendered(&Served::json(
            503,
            &json!({ "error": "the CodeArtifact store is unavailable" }),
        ));
    };
    let now = SystemTime::now();

    // The credential first, for every route including `/-/ping`: a registry that answered
    // a packument or a publish to an unauthenticated client would be a registry no
    // credential gates. The principal the token speaks for is carried out of the record,
    // because what that principal may do HERE is what this repository's policy states.
    let presented = bearer_token(req.headers().get("authorization"));
    let principal = presented.and_then(|token| {
        state
            .lock()
            .authorization_token(token, now)
            .filter(|held| {
                held.domain == resolved.domain && held.domain_owner == resolved.domain_owner
            })
            .map(|held| held.principal.clone())
    });
    let Some(principal) = principal else {
        return rendered(&unauthorized());
    };

    let Some(request) = NpmRequest::of(method_of(req), &resolved.path) else {
        return rendered(&not_found("the registry route was not found".to_string()));
    };
    // What this request ASKS to do in the repository, which is the action its policy is
    // evaluated for.
    let action = match request {
        NpmRequest::Publish(_) | NpmRequest::SetDistTag(..) | NpmRequest::DeleteDistTag(..) => {
            PUBLISH_ACTION
        }
        NpmRequest::Ping
        | NpmRequest::Packument(_)
        | NpmRequest::Tarball(..)
        | NpmRequest::ReadDistTags(_)
        | NpmRequest::UnencodedScope(_) => READ_ACTION,
    };
    if let Some(refusal) = policy_refusal(&state.lock(), &resolved, &principal, action) {
        return rendered(&refusal);
    }

    // A READ of a package the addressed repository does not hold is served from the
    // repository's upstreams, and what they resolve is retained here before it is read, so
    // every answer below comes out of the addressed repository's own records.
    match &request {
        NpmRequest::Packument(package)
        | NpmRequest::Tarball(package, _)
        | NpmRequest::ReadDistTags(package) => {
            retain_from_upstreams(&mut state.lock(), &resolved, package, now);
        }
        NpmRequest::Ping
        | NpmRequest::Publish(_)
        | NpmRequest::SetDistTag(..)
        | NpmRequest::DeleteDistTag(..)
        | NpmRequest::UnencodedScope(_) => {}
    }

    let served = match request {
        // A ping carries no body of its own: it reports that the registry answered this
        // credential, which is the status.
        NpmRequest::Ping => Served::json(200, &json!({})),
        NpmRequest::Packument(package) => packument(
            &state.lock(),
            &resolved,
            &package,
            &endpoint_url(&addressed, &resolved),
        ),
        NpmRequest::Tarball(package, file_name) => {
            tarball(&state.lock(), &resolved, &package, &file_name)
        }
        NpmRequest::Publish(package) => {
            let body = req.body().bytes().unwrap_or_default();
            publish(&mut state.lock(), &resolved, &package, body, now)
        }
        NpmRequest::ReadDistTags(package) => dist_tags(&state.lock(), &resolved, &package),
        NpmRequest::SetDistTag(package, tag) => {
            let body = req.body().bytes().unwrap_or_default();
            set_dist_tag(&mut state.lock(), &resolved, &package, &tag, body)
        }
        NpmRequest::DeleteDistTag(package, tag) => {
            delete_dist_tag(&mut state.lock(), &resolved, &package, &tag)
        }
        // A scoped package addressed without encoding its scope separator on a route that
        // carries the package in one segment: what the path named is a scope, so the
        // refusal names it as the package it could not resolve.
        NpmRequest::UnencodedScope(named) => bad_request(format!(
            "the package '{named}' is not a valid npm package name; a scoped package's name \
             must be URL-encoded in one path segment (@scope%2fname)"
        )),
    };
    rendered(&served)
}

/// The IAM action a READ of this repository's packages asks for, as the IAM Service
/// Authorization Reference names it for CodeArtifact: the action a repository policy grants
/// a package manager that resolves and downloads from the repository.
const READ_ACTION: &str = "codeartifact:ReadFromRepository";

/// The IAM action a PUBLISH into this repository asks for, named the same way.
const PUBLISH_ACTION: &str = "codeartifact:PublishPackageVersion";

/// The refusal this repository's own resource policy states for `principal` performing
/// `action` on it, or `None` when the policy does not refuse it.
///
/// Only an explicit `Deny` refuses here. That is IAM's evaluation order within one account:
/// a deny in the resource policy wins over anything an identity policy allows, while a
/// resource policy that merely states nothing withholds no access — the caller's identity
/// policies may still permit it, and this plane holds no session to read them for: it
/// authenticates a BEARER credential, whose principal is the one recorded at the mint, not a
/// signing session ([`crate::service::authorization`] is the decision for a control-plane
/// call, which does have one). A repository carrying no policy refuses nothing.
fn policy_refusal(
    store: &CaState,
    resolved: &Resolved,
    principal: &str,
    action: &str,
) -> Option<Served> {
    use ::service_common::iam_authorization as iam;

    let repository = store
        .domain(&resolved.domain, Some(&resolved.domain_owner))?
        .repositories
        .get(&resolved.repository)?;
    let document = repository.policy.as_ref()?.document.clone()?;
    // The spellings a statement's `Principal` element may name this caller by: the
    // principal itself and the account forms that stand for it.
    let mut principals = vec![principal.to_string()];
    principals.extend(iam::account_principals(principal));
    let request = iam::Request::new(action, &repository.arn, &principals);
    if iam::resource_decision(&document, &request) != iam::Decision::Denied {
        return None;
    }
    Some(Served::json(
        403,
        &json!({
            "error": iam::not_authorized_by_resource_policy(principal, action, &repository.arn)
        }),
    ))
}

// ---------------------------------------------------------------------------
// Addressing
// ---------------------------------------------------------------------------

/// What a data-plane request ADDRESSES, as the wire carries it: the host it was sent to —
/// absent on a request that carries neither a `Host` header nor an absolute URI — and its
/// path.
///
/// Built from BOTH shapes the router hands this module, the buffered parts the claim reads
/// and the runtime-API request the serving seam reads, so one rule resolves both and the
/// claim cannot resolve a different repository than the answer does.
#[derive(Debug)]
struct Addressed<'a> {
    host: Option<&'a str>,
    path: &'a str,
}

impl<'a> Addressed<'a> {
    /// The addressing of the buffered request these parts came from.
    fn of_parts(parts: &'a ::http::request::Parts) -> Self {
        Self {
            host: parts
                .headers
                .get("host")
                .and_then(|host| host.to_str().ok())
                .or_else(|| parts.uri.authority().map(::http::uri::Authority::as_str)),
            path: parts.uri.path(),
        }
    }

    /// The addressing of a runtime-API request, whose URI arrives as a string and is parsed
    /// by the `http` crate rather than split by hand.
    fn of_request(req: &'a RtRequest, uri: &'a ::http::Uri) -> Self {
        Self {
            host: req
                .headers()
                .get("host")
                .or_else(|| uri.authority().map(::http::uri::Authority::as_str)),
            path: uri.path(),
        }
    }
}

/// What a request resolves to: the repository whose endpoint it addresses, and the path
/// WITHIN that endpoint — the part the npm protocol is read from.
#[derive(Debug)]
struct Resolved {
    domain: String,
    domain_owner: String,
    repository: String,
    /// The `/<format>/<repository>` prefix the request addressed, kept so a URL this plane
    /// publishes back (a tarball's) is under the address the client actually reached.
    endpoint_prefix: String,
    /// The path within the endpoint, leading `/` kept.
    path: String,
}

/// Which repository endpoint `addressed` addresses, or `None` when this plane serves the
/// request not at all.
///
/// The two addressings are the module documentation's, in that order. Each resolves only to
/// a repository this service HOLDS, read from the one shared store, so a request keeps one
/// claimant.
fn resolve<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> Option<Resolved> {
    let (format, rest) = first_segment(addressed.path)?;
    if format != PACKAGE_FORMAT_NPM {
        return None;
    }
    let (repository, path) = first_segment(rest)?;
    if repository.is_empty() {
        return None;
    }
    let state = service_state(engine)?;
    let store = state.lock();
    let endpoint_prefix = format!("/{format}/{repository}");

    // HOST-STYLE: the published endpoint host names the domain and the account that owns
    // it, so the request addresses exactly that domain's repository.
    if let Some((domain, account)) = addressed
        .host
        .map(host_only)
        .and_then(endpoint_host_domain)
    {
        let held = store.domain(domain, Some(account))?;
        if !held.repositories.contains_key(repository) {
            return None;
        }
        return Some(Resolved {
            domain: held.name.clone(),
            domain_owner: held.owner.clone(),
            repository: repository.to_string(),
            endpoint_prefix,
            path: path.to_string(),
        });
    }

    // PATH-STYLE on the emulator's OWN endpoint, which is the only address a client on this
    // machine can reach: the repository name alone, read only when one domain holds a
    // repository of that name (see the module docs). A host that names some SERVICE — this
    // service's own API endpoint, or another service's — keeps answering as that service,
    // so a path under it is not claimed here however it is spelled.
    if addressed.host.is_some_and(names_a_service) {
        return None;
    }
    let mut holding = store
        .domains
        .values()
        .filter(|domain| domain.repositories.contains_key(repository));
    let only = holding.next()?;
    if holding.next().is_some() {
        return None;
    }
    Some(Resolved {
        domain: only.name.clone(),
        domain_owner: only.owner.clone(),
        repository: repository.to_string(),
        endpoint_prefix,
        path: path.to_string(),
    })
}

/// The domain and the owning account a published repository-endpoint host names, or `None`
/// for every other host — this service's own API endpoint among them.
///
/// The published form is `<domain>-<account>.d.codeartifact.<region>.<suffix>`
/// ([`crate::state::repository_endpoint`] mints it, with `amazonaws.com` for the IPv4
/// endpoint and `on.aws` for the dual-stack one), so the account is the last `-`-separated
/// field of the first label and the domain is everything before it. A `DomainName` is
/// modelled `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$` and so may itself contain `-`, which is why
/// the split is from the RIGHT and why the account field has to be
/// [`ACCOUNT_ID_DIGITS`] digits to be read as one.
fn endpoint_host_domain(host: &str) -> Option<(&str, &str)> {
    let (label, rest) = host.split_once('.')?;
    let region_and_suffix = rest.strip_prefix(ENDPOINT_HOST_INFIX)?;
    if region_and_suffix.is_empty() {
        return None;
    }
    let (domain, account) = label.rsplit_once('-')?;
    let account_shaped =
        account.len() == ACCOUNT_ID_DIGITS && account.bytes().all(|byte| byte.is_ascii_digit());
    (!domain.is_empty() && account_shaped).then_some((domain, account))
}

/// The request URI of a runtime-API request, parsed by the `http` crate.
///
/// A URI this parse rejects is one the server would not have accepted, and it resolves to
/// `http::Uri`'s own default (`/`), which names no repository and claims nothing.
fn parsed_uri(uri: &str) -> ::http::Uri {
    uri.parse::<::http::Uri>().unwrap_or_default()
}

/// The request method, upper-cased as the wire carries it.
fn method_of(req: &RtRequest) -> String {
    req.method().to_ascii_uppercase()
}

/// The absolute URL of the endpoint this request reached, so a tarball URL this plane
/// publishes is one the SAME client can fetch: the host it addressed and the endpoint path
/// it used.
///
/// The host is read exactly as the claim read it ([`Addressed::of_request`]), so a request
/// that resolved through the URI authority publishes the authority it arrived on. `https`
/// because that is the scheme every endpoint of this emulator is served on, and the scheme
/// the published endpoint URL names.
fn endpoint_url(addressed: &Addressed<'_>, resolved: &Resolved) -> String {
    let host = addressed.host.map(host_with_port).unwrap_or_default();
    format!("https://{host}{}", resolved.endpoint_prefix)
}

/// A `Host` header value with its port kept (the port is part of the address a client on
/// this machine reaches) and its userinfo removed.
fn host_with_port(host: &str) -> &str {
    host.rsplit('@').next().unwrap_or(host)
}

/// The bearer credential an `Authorization` header presents, or `None` when it presents
/// none — including a header carrying another scheme, which is not a token this registry
/// resolves.
fn bearer_token(authorization: Option<&str>) -> Option<&str> {
    let (scheme, token) = authorization?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

// ---------------------------------------------------------------------------
// The npm protocol: what a path and a method ask for
// ---------------------------------------------------------------------------

/// One npm package as the registry addresses it: the scope it sits in, if any, its name,
/// and whether the path carried the whole name in ONE segment.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NpmPackage {
    /// The scope WITHOUT its `@`, which is the `namespace` coordinate CodeArtifact stores
    /// an npm package under ("The namespace of an npm or Swift package version is its
    /// scope").
    namespace: Option<String>,
    name: String,
    /// Set when the path named this package in ONE segment — a plain name, or a scope whose
    /// separator was percent-encoded (`@scope%2fname`). Clear when the scope arrived as a
    /// path segment of its own (`@scope/name`), which only a tarball's path carries.
    encoded: bool,
}

impl NpmPackage {
    /// The package as npm writes it: `@<scope>/<name>`, or the bare name.
    fn display_name(&self) -> String {
        crate::state::package_display_name(
            PACKAGE_FORMAT_NPM,
            self.namespace.as_deref(),
            &self.name,
        )
    }

    /// The path a registry addresses this package at: `@<scope>/<name>` for a scoped
    /// package, which is the path npm's own tarball URL carries (a client PUBLISHES to the
    /// percent-encoded form, and a registry serves both — see [`addressed_package`]).
    fn url_path(&self) -> String {
        self.display_name()
    }

    /// The file name npm's tarball URL names the version `version` of this package by:
    /// `<name>-<version>.tgz`, with the package name written WITHOUT its scope, which is
    /// how npm writes a tarball file name.
    fn tarball_file_name(&self, version: &str) -> String {
        format!("{}-{version}{NPM_TARBALL_EXTENSION}", self.name)
    }
}

/// What one request of the npm protocol asks for.
#[derive(Debug)]
enum NpmRequest {
    /// `GET /-/ping`: does this registry answer this credential?
    Ping,
    /// `GET /<package>`: the package's packument.
    Packument(NpmPackage),
    /// `GET /<package>/-/<file>`: one version's tarball bytes.
    Tarball(NpmPackage, String),
    /// `PUT /<package>`: an npm publish document.
    Publish(NpmPackage),
    /// `GET /-/package/<package>/dist-tags`: every distribution tag the package carries.
    ReadDistTags(NpmPackage),
    /// `PUT /-/package/<package>/dist-tags/<tag>`: point `tag` at the version the body
    /// names.
    SetDistTag(NpmPackage, String),
    /// `DELETE /-/package/<package>/dist-tags/<tag>`: take `tag` off the package.
    DeleteDistTag(NpmPackage, String),
    /// A route that addresses a SCOPED package without encoding its scope separator, on a
    /// route that carries the package in ONE segment: the first segment names a scope and
    /// no package, which is a bad request rather than a missing resource. Carries that
    /// segment, so the refusal can name the package the path actually addressed.
    UnencodedScope(String),
}

impl NpmRequest {
    /// What `method` on the endpoint path `path` asks for, or `None` for a route this
    /// registry does not serve.
    fn of(method: String, path: &str) -> Option<Self> {
        let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        match segments.as_slice() {
            [reserved, route]
                if *reserved == NPM_RESERVED_SEGMENT && *route == PING_ROUTE && method == "GET" =>
            {
                return Some(Self::Ping);
            }
            // The dist-tag routes, which address the package UNDER the reserved segment:
            // `/-/package/<package>/dist-tags[/<tag>]`. A path carrying anything past the
            // tag names no route this registry serves.
            [reserved, route, package, tags]
                if *reserved == NPM_RESERVED_SEGMENT
                    && *route == PACKAGE_ROUTE
                    && *tags == DIST_TAGS_ROUTE =>
            {
                let named = single_segment_package(package)?;
                return (method == "GET").then_some(Self::ReadDistTags(named));
            }
            [reserved, route, package, tags, tag]
                if *reserved == NPM_RESERVED_SEGMENT
                    && *route == PACKAGE_ROUTE
                    && *tags == DIST_TAGS_ROUTE =>
            {
                let named = single_segment_package(package)?;
                let tag = percent_decoded(tag)?;
                return match method.as_str() {
                    "PUT" => Some(Self::SetDistTag(named, tag)),
                    "DELETE" => Some(Self::DeleteDistTag(named, tag)),
                    _ => None,
                };
            }
            // Every other registry-own route (`/-/whoami`, the audit endpoints, an
            // unpublish `/-/rev/…`) is not served here.
            [reserved, ..] if *reserved == NPM_RESERVED_SEGMENT => return None,
            _ => {}
        }
        let (package, rest) = addressed_package(&segments)?;
        match (method.as_str(), rest) {
            // The packument and the publish carry the package in ONE segment, so a scope's
            // separator has to be encoded there; a tarball's path carries the scope as a
            // segment of its own, which is how npm writes the URL this registry publishes.
            ("GET" | "PUT", []) if package.namespace.is_some() && !package.encoded => {
                Some(Self::UnencodedScope(format!(
                    "@{}",
                    package.namespace.unwrap_or_default()
                )))
            }
            ("GET", []) => Some(Self::Packument(package)),
            ("PUT", []) => Some(Self::Publish(package)),
            ("GET", [reserved, file]) if *reserved == NPM_RESERVED_SEGMENT => {
                Some(Self::Tarball(package, percent_decoded(file)?))
            }
            _ => None,
        }
    }
}

/// The segment npm's dist-tag routes carry ahead of the package name.
const PACKAGE_ROUTE: &str = "package";

/// The segment that names the dist-tag collection of a package.
const DIST_TAGS_ROUTE: &str = "dist-tags";

/// The package ONE path segment names, scope encoded (`@scope%2fname`) or not at all.
fn single_segment_package(segment: &str) -> Option<NpmPackage> {
    let decoded = percent_decoded(segment)?;
    if let Some((scope, name)) = scoped_name(&decoded) {
        return Some(NpmPackage {
            namespace: Some(scope),
            name,
            encoded: true,
        });
    }
    (!decoded.is_empty() && !decoded.starts_with('@')).then_some(NpmPackage {
        namespace: None,
        name: decoded,
        encoded: true,
    })
}

/// The package the leading segments of an endpoint path address, and the segments left
/// after it.
///
/// npm writes a scoped package BOTH ways, and a registry serves both: percent-encoded in
/// one segment (`@scope%2fname`, which is what `npm publish` sends) and as two path
/// segments (`@scope/name`, which is what a tarball URL carries). The `@` is what says a
/// scope is present, and the reserved segment is what says the package's own path has
/// ended.
fn addressed_package<'p>(segments: &'p [&'p str]) -> Option<(NpmPackage, &'p [&'p str])> {
    let (first, rest) = segments.split_first()?;
    let decoded = percent_decoded(first)?;
    if let Some((scope, name)) = scoped_name(&decoded) {
        return Some((
            NpmPackage {
                namespace: Some(scope),
                name,
                encoded: true,
            },
            rest,
        ));
    }
    if decoded.starts_with('@') {
        // A scope with its name in the NEXT segment; a reserved segment there would mean
        // the path named a scope and no package.
        let (name, rest) = rest.split_first()?;
        if *name == NPM_RESERVED_SEGMENT {
            return None;
        }
        let name = percent_decoded(name)?;
        return Some((
            NpmPackage {
                namespace: Some(decoded.trim_start_matches('@').to_string()),
                name,
                encoded: false,
            },
            rest,
        ));
    }
    (!decoded.is_empty()).then_some((
        NpmPackage {
            namespace: None,
            name: decoded,
            encoded: true,
        },
        rest,
    ))
}

/// The scope and the name a single `@scope/name` value carries, or `None` when the value is
/// not one.
fn scoped_name(value: &str) -> Option<(String, String)> {
    let scoped = value.strip_prefix('@')?;
    let (scope, name) = scoped.split_once('/')?;
    (!scope.is_empty() && !name.is_empty() && !name.contains('/'))
        .then(|| (scope.to_string(), name.to_string()))
}

/// One path segment with its percent-escapes resolved, or `None` when they do not decode to
/// UTF-8 — a segment that addresses no name this service could hold.
fn percent_decoded(segment: &str) -> Option<String> {
    ::percent_encoding::percent_decode_str(segment)
        .decode_utf8()
        .ok()
        .map(::std::borrow::Cow::into_owned)
}

// ---------------------------------------------------------------------------
// The answers
// ---------------------------------------------------------------------------

/// One answer this plane renders: the status, the media type it carries, the body, and the
/// challenge an authentication refusal states.
#[derive(Debug, Default)]
struct Served {
    status: u16,
    content_type: Option<String>,
    authenticate: Option<String>,
    body: Vec<u8>,
}

impl Served {
    /// An answer carrying `document` as JSON.
    fn json(status: u16, document: &Value) -> Self {
        Self {
            status,
            content_type: Some(JSON_MEDIA_TYPE.to_string()),
            authenticate: None,
            body: ::serde_json::to_vec(document).unwrap_or_default(),
        }
    }

    /// An answer carrying opaque asset bytes.
    fn bytes(content: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: Some(TARBALL_MEDIA_TYPE.to_string()),
            authenticate: None,
            body: content,
        }
    }
}

/// The refusal npm reads as "this registry does not have that": the status, with the
/// `error` member npm renders. The sentence states what was not found, in the same words
/// this service's control plane states them in, so one client reads one wording.
fn not_found(reason: String) -> Served {
    Served::json(404, &json!({ "error": reason }))
}

/// The refusal a request presenting no usable bearer credential earns.
///
/// `401` with the challenge, which is what tells an npm client to present (or refresh) a
/// token rather than to retry the same request. The reason states what was wrong with the
/// credential in the terms of the credential, never quoting the value presented.
fn unauthorized() -> Served {
    let mut served = Served::json(
        401,
        &json!({
            "error": "unauthorized: an authorization token for this domain is required"
        }),
    );
    served.authenticate = Some("Bearer".to_string());
    served
}

/// The refusal a request the registry understood but will not perform earns: a publish
/// origin control blocks, or a repository policy denies.
fn forbidden(reason: String) -> Served {
    Served::json(403, &json!({ "error": reason }))
}

/// The refusal a download of a version whose STATUS withholds its content earns.
///
/// A client asked for a version this repository holds and will not serve, which is a
/// request it cannot make rather than a resource that is missing: the version is listed by
/// the control plane, its assets are not downloadable, and a `404` would say it does not
/// exist. The sentence names the status, so a client can tell a blocked download from a
/// missing one.
fn blocked_download(reason: String) -> Served {
    Served::json(400, &json!({ "error": reason }))
}

/// The refusal a publish into a package whose origin configuration BLOCKS new versions
/// earns: `400`, with npm's `success`/`error` envelope and the sentence CodeArtifact's npm
/// endpoint states for that control.
///
/// The sentence states the CONTROL and interpolates nothing — it is the same for whatever
/// package was addressed — and the status is `400` rather than the `403` an authorization
/// refusal carries: the credential was accepted and the repository reachable; what the
/// request asked for is a version this package does not accept.
fn publish_blocked() -> Served {
    Served::json(
        400,
        &json!({
            "success": false,
            "error": "This package is configured to block new versions from being published."
        }),
    )
}

/// The refusal a publish document the registry cannot read earns.
fn bad_request(reason: String) -> Served {
    Served::json(400, &json!({ "error": reason }))
}

/// The packument of `package` in the addressed repository: what an npm client resolves a
/// version range and a dist-tag against.
///
/// Read out of the addressed repository's OWN records: whatever its upstream chain resolves
/// has already been retained there ([`retain_from_upstreams`]) by the time this runs.
///
/// Only a version in [`crate::state::PACKAGE_VERSION_STATUS_PUBLISHED`] is listed. That is
/// the one status in which a version is both listed and resolvable: an `Unlisted` version
/// is by its own definition hidden from the version list while its content stays
/// downloadable ([`tarball`] serves it), and an `Archived`, `Disposed` or `Unfinished` one
/// is not a version a client can resolve at all. A package with no listed version is `404`,
/// the answer a client reads as "this registry has no such package".
fn packument(store: &CaState, resolved: &Resolved, package: &NpmPackage, endpoint: &str) -> Served {
    let Some(held) = held_package(store, resolved, package) else {
        return not_found(package_not_found(package));
    };
    let display_name = package.display_name();
    let listed: Vec<&PackageVersion> = held
        .versions
        .values()
        .filter(|version| version.status == crate::state::PACKAGE_VERSION_STATUS_PUBLISHED)
        .collect();
    if listed.is_empty() {
        return not_found(package_not_found(package));
    }

    let mut versions = Map::new();
    let mut times = Map::new();
    for version in &listed {
        versions.insert(
            version.version.clone(),
            version_manifest(package, version, endpoint),
        );
        if let Some(stamp) = rfc3339(version.published_time) {
            times.insert(version.version.clone(), Value::String(stamp));
        }
    }
    // `created`/`modified` are the publish instants of the earliest and the latest listed
    // version, which is what they are: a packument's own clock, derived from the records.
    let published: Vec<SystemTime> = listed.iter().map(|version| version.published_time).collect();
    if let Some(stamp) = published.iter().min().copied().and_then(rfc3339) {
        times.insert("created".to_string(), Value::String(stamp));
    }
    if let Some(stamp) = published.iter().max().copied().and_then(rfc3339) {
        times.insert("modified".to_string(), Value::String(stamp));
    }

    // A tag resolves only to a version a client can actually fetch, so a tag pointing at a
    // version that is not listed is left out — including `latest`, which is why unlisting
    // the version `latest` named leaves the packument carrying no tag at all rather than
    // re-pointing it at some other version the publisher never tagged.
    let mut dist_tags = Map::new();
    for (tag, version) in &held.dist_tags {
        if versions.contains_key(version) {
            dist_tags.insert(tag.clone(), Value::String(version.clone()));
        }
    }

    Served::json(
        200,
        &json!({
            "_id": display_name,
            "name": display_name,
            "dist-tags": Value::Object(dist_tags),
            "versions": Value::Object(versions),
            "time": Value::Object(times),
        }),
    )
}

/// One version's entry in a packument: the metadata document that was published, with the
/// `dist` object the registry owns written over it.
///
/// The document is the version's stored manifest ([`crate::state::PackageVersion::manifest`]),
/// so what a client reads back is what was published — its dependencies in their declared
/// order included, because the text is the published text. `dist` is the registry's own: the
/// URL this repository serves the tarball at, and the digests of the tarball AS STORED, so a
/// client verifying integrity verifies against the bytes it will actually receive.
fn version_manifest(package: &NpmPackage, version: &PackageVersion, endpoint: &str) -> Value {
    let mut manifest = version
        .manifest
        .as_deref()
        .and_then(|document| ::serde_json::from_str::<Value>(document).ok())
        .and_then(|document| match document {
            Value::Object(fields) => Some(fields),
            _ => None,
        })
        .unwrap_or_default();
    manifest.insert("name".to_string(), Value::String(package.display_name()));
    manifest.insert(
        "version".to_string(),
        Value::String(version.version.clone()),
    );
    if let Some(asset) = version.assets.get(crate::state::NPM_TARBALL_ASSET) {
        manifest.insert(
            "dist".to_string(),
            json!({
                "tarball": format!(
                    "{endpoint}/{}/{NPM_RESERVED_SEGMENT}/{}",
                    package.url_path(),
                    package.tarball_file_name(&version.version)
                ),
                "shasum": asset.sha1_hex(),
                "integrity": format!("sha512-{}", asset.sha512_base64()),
            }),
        );
    }
    Value::Object(manifest)
}

/// The bytes of the version `file_name` names, for `package`.
///
/// The file name is npm's own ([`NpmPackage::tarball_file_name`]), so the version it names
/// is resolved from the stored records rather than from an asset name: the ASSET every npm
/// version's content is stored under is the format's fixed one
/// ([`crate::state::NPM_TARBALL_ASSET`]).
///
/// Read out of the addressed repository's OWN records, like every other answer here: a
/// version its upstream chain resolves was retained in it before this ran
/// ([`retain_from_upstreams`]).
///
/// A version's status decides whether its CONTENT is served, independently of whether the
/// version is listed: `Published` and `Unlisted` are the two statuses in which a version's
/// assets are downloadable ("an unlisted version … can still be downloaded"), and every
/// other status withholds them — an `Archived` version's content is retained but its
/// download is blocked, and a `Disposed` one's is gone.
fn tarball(
    store: &CaState,
    resolved: &Resolved,
    package: &NpmPackage,
    file_name: &str,
) -> Served {
    let Some(held) = held_package(store, resolved, package) else {
        return not_found(package_not_found(package));
    };
    let asset_not_found = || {
        not_found(format!(
            "Package version not found in repository. Asset = {{format={PACKAGE_FORMAT_NPM}, \
             name={}, asset={file_name}}}",
            package.display_name()
        ))
    };
    let Some(version) = held
        .versions
        .values()
        .find(|version| package.tarball_file_name(&version.version) == file_name)
    else {
        return asset_not_found();
    };
    if !downloadable(&version.status) {
        return blocked_download(format!(
            "the package version {} of {} is {} and its assets cannot be downloaded",
            version.version,
            package.display_name(),
            version.status
        ));
    }
    let Some(content) = version.assets.get(crate::state::NPM_TARBALL_ASSET) else {
        return asset_not_found();
    };
    Served::bytes(content.content().to_vec())
}

/// The sentence this registry states for a package the addressed repository does not hold:
/// the one the control plane states for the same absence
/// ([`crate::state::package_not_found_message`]), so a client reading both reads one
/// wording.
fn package_not_found(package: &NpmPackage) -> String {
    crate::state::package_not_found_message(
        PACKAGE_FORMAT_NPM,
        package.namespace.as_deref(),
        &package.name,
    )
}

/// Every distribution tag `package` carries that a client can resolve: the tag map npm's
/// `/-/package/<package>/dist-tags` route answers.
///
/// Read out of the addressed repository's own records, like every other answer here: a tag
/// its upstream chain resolves arrived with the retention ([`retain_from_upstreams`]). A tag
/// pointing at a version that is not listed is left out, for the same reason the packument
/// leaves it out: a tag resolves to a version a client can fetch, or to nothing.
fn dist_tags(store: &CaState, resolved: &Resolved, package: &NpmPackage) -> Served {
    let Some(held) = held_package(store, resolved, package) else {
        return not_found(package_not_found(package));
    };
    let mut tags = Map::new();
    for (tag, version) in &held.dist_tags {
        let listed = held
            .versions
            .get(version)
            .is_some_and(|held| held.status == crate::state::PACKAGE_VERSION_STATUS_PUBLISHED);
        if listed {
            tags.insert(tag.clone(), Value::String(version.clone()));
        }
    }
    Served::json(200, &Value::Object(tags))
}

/// Point `tag` at the version the request body names — npm's
/// `PUT /-/package/<package>/dist-tags/<tag>`, whose body is the version as a JSON string
/// (`npm dist-tag add`).
///
/// A tag may only name a version the package HOLDS and a client can resolve: a tag pointing
/// anywhere else would resolve to nothing, so it is refused rather than stored.
fn set_dist_tag(
    store: &mut CaState,
    resolved: &Resolved,
    package: &NpmPackage,
    tag: &str,
    body: &[u8],
) -> Served {
    // The body names ONE version, which is modelled `PackageVersion` (length 1..255), so a
    // longer body names no version this service could hold and is refused before anything
    // is read out of it.
    if body.len() > MAX_DIST_TAG_BODY_BYTES {
        return bad_request(format!(
            "the version named for the tag is {} bytes, more than the {MAX_DIST_TAG_BODY_BYTES} \
             a version name can be",
            body.len()
        ));
    }
    let Ok(text) = ::std::str::from_utf8(body) else {
        return bad_request("the version named for the tag is not text".to_string());
    };
    // The body is the version as a JSON string; a client that sends the bare version is
    // read the same way, because the version itself is never quoted text.
    let version = ::serde_json::from_str::<String>(text).unwrap_or_else(|_| text.trim().to_string());
    if version.is_empty() {
        return bad_request("the request names no version for the tag".to_string());
    }
    let Some(held) = held_package_mut(store, resolved, package) else {
        return not_found(package_not_found(package));
    };
    let resolvable = held
        .versions
        .get(&version)
        .is_some_and(|version| downloadable(&version.status));
    if !resolvable {
        return not_found(crate::state::package_version_not_found_message(
            &crate::state::repository_id(
                &resolved.domain_owner,
                &resolved.domain,
                &resolved.repository,
            ),
            PACKAGE_FORMAT_NPM,
            package.namespace.as_deref(),
            &package.name,
            &version,
        ));
    }
    // A tag the package does not already carry is a new entry in its tag map, which is
    // bounded like every other per-request insert in this store; moving a tag the package
    // carries adds nothing.
    if !held.dist_tags.contains_key(tag)
        && held.dist_tags.len() >= crate::state::MAX_DIST_TAGS_PER_PACKAGE
    {
        return forbidden(format!(
            "the package already carries the maximum of {} distribution tags",
            crate::state::MAX_DIST_TAGS_PER_PACKAGE
        ));
    }
    held.dist_tags.insert(tag.to_string(), version.clone());
    // A tag is state a later caller resolves through, so the write is attributable in the
    // emulator's own log: which repository, which package, which tag and which version.
    ::tracing::info!(
        domain = %resolved.domain,
        repository = %resolved.repository,
        package = %package.display_name(),
        tag = %tag,
        version = %version,
        outcome = "set",
        "a package manager set a distribution tag over the repository endpoint"
    );
    Served::json(200, &json!({ "ok": true, tag: version }))
}

/// Take `tag` off `package` — npm's `DELETE /-/package/<package>/dist-tags/<tag>`.
///
/// Idempotent: a tag the package does not carry is already off it, so the answer is the
/// same as for one that was just removed.
fn delete_dist_tag(
    store: &mut CaState,
    resolved: &Resolved,
    package: &NpmPackage,
    tag: &str,
) -> Served {
    let Some(held) = held_package_mut(store, resolved, package) else {
        return not_found(package_not_found(package));
    };
    held.dist_tags.remove(tag);
    ::tracing::info!(
        domain = %resolved.domain,
        repository = %resolved.repository,
        package = %package.display_name(),
        tag = %tag,
        outcome = "removed",
        "a package manager removed a distribution tag over the repository endpoint"
    );
    Served::json(200, &json!({ "ok": true }))
}

/// [`held_package`] for the two routes that WRITE a package's tags.
fn held_package_mut<'s>(
    store: &'s mut CaState,
    resolved: &Resolved,
    package: &NpmPackage,
) -> Option<&'s mut Package> {
    let key = crate::state::package_key(
        PACKAGE_FORMAT_NPM,
        package.namespace.as_deref(),
        &package.name,
    );
    store
        .domain_mut(&resolved.domain, Some(&resolved.domain_owner))?
        .repositories
        .get_mut(&resolved.repository)?
        .packages
        .get_mut(&key)
}

/// Whether a version in `status` serves its assets. See [`tarball`].
fn downloadable(status: &str) -> bool {
    status == crate::state::PACKAGE_VERSION_STATUS_PUBLISHED
        || status == crate::state::PACKAGE_VERSION_STATUS_UNLISTED
}

/// The package record the addressed repository holds for `package`, or `None`.
fn held_package<'s>(
    store: &'s CaState,
    resolved: &Resolved,
    package: &NpmPackage,
) -> Option<&'s Package> {
    store
        .domain(&resolved.domain, Some(&resolved.domain_owner))?
        .repositories
        .get(&resolved.repository)?
        .package(
            PACKAGE_FORMAT_NPM,
            package.namespace.as_deref(),
            &package.name,
        )
}

// ---------------------------------------------------------------------------
// Upstream repositories
// ---------------------------------------------------------------------------

/// Retain in the ADDRESSED repository every version of `package` that a repository it
/// reaches through its upstream chain holds and it does not.
///
/// A CodeArtifact repository resolves a package through its upstream repositories, and a
/// version found in one is COPIED into the repository that was asked for it, where it is
/// retained — so the next request is served by that repository itself, and keeps being
/// served from it after the upstream is detached (AWS CodeArtifact user guide, *Working
/// with upstream repositories*). Retaining is therefore what a request for a package the
/// repository does not hold DOES, not a side effect of reading one: it runs before the
/// answer, and every answer is then read out of the addressed repository's own records.
///
/// The search is breadth first over the chain, each repository in its own upstream order,
/// so the nearest copy is the one retained; the copy is made in the repository the request
/// addressed, and an intermediate hop retains nothing because nothing was asked of it.
///
/// Nothing is retained when the addressed repository already holds the versions, when its
/// own origin control blocks upstream ingestion for the package
/// ([`Package::upstream_allowed`]), when no reachable repository holds a downloadable
/// version, or when the copy would take this emulator past the content it may retain
/// ([`crate::state::MAX_STORED_ASSET_BYTES`]). In each case the answer is read out of the
/// addressed repository alone, as it was.
fn retain_from_upstreams(
    store: &mut CaState,
    resolved: &Resolved,
    package: &NpmPackage,
    now: SystemTime,
) {
    let key = crate::state::package_key(
        PACKAGE_FORMAT_NPM,
        package.namespace.as_deref(),
        &package.name,
    );

    // What there is to retain, read as OWNED records so the store can be taken mutably
    // below without this reading borrow still being alive.
    let Some(domain) = store.domain(&resolved.domain, Some(&resolved.domain_owner)) else {
        return;
    };
    let Some(addressed) = domain.repositories.get(&resolved.repository) else {
        return;
    };
    let held = addressed.packages.get(&key);
    if held.is_some_and(|held| !held.upstream_allowed()) {
        return;
    }
    let already: ::std::collections::BTreeSet<String> = held
        .map(|held| held.versions.keys().cloned().collect())
        .unwrap_or_default();
    let resolved_upstream = domain
        .upstream_chain(&resolved.repository)
        .into_iter()
        .filter_map(|name| domain.repositories.get(&name))
        .find_map(|upstream| {
            let candidate = upstream.packages.get(&key)?;
            // Only a version whose content this repository would serve is worth a copy:
            // an `Archived` or `Disposed` version's assets are not downloadable, so a copy
            // of one would answer nothing.
            let versions: Vec<PackageVersion> = candidate
                .versions
                .values()
                .filter(|version| {
                    downloadable(&version.status) && !already.contains(&version.version)
                })
                .cloned()
                .collect();
            let tags: Vec<(String, String)> = candidate
                .dist_tags
                .iter()
                .map(|(tag, version)| (tag.clone(), version.clone()))
                .collect();
            (!versions.is_empty()).then_some((upstream.name.clone(), versions, tags))
        });
    let Some((upstream_name, versions, tags)) = resolved_upstream else {
        return;
    };
    let incoming: i64 = versions
        .iter()
        .map(|version| {
            version
                .asset_size_bytes()
                .saturating_add(version.manifest_size_bytes())
        })
        .sum();
    if store
        .stored_content_bytes()
        .saturating_add(incoming)
        > crate::state::MAX_STORED_ASSET_BYTES
    {
        ::tracing::warn!(
            domain = %resolved.domain,
            repository = %resolved.repository,
            package = %package.display_name(),
            "this emulator already retains the package content it may hold, so the \
             versions resolved from an upstream repository are not retained"
        );
        return;
    }

    let Some(repository) = store
        .domain_mut(&resolved.domain, Some(&resolved.domain_owner))
        .and_then(|domain| domain.repositories.get_mut(&resolved.repository))
    else {
        return;
    };
    if !repository.packages.contains_key(&key)
        && repository.packages.len() >= crate::state::MAX_PACKAGES_PER_REPOSITORY
    {
        return;
    }
    // A package record this retention CREATES carries the controls CodeArtifact sets for a
    // package that came to exist by upstream ingestion: ingestion allowed, publishing
    // blocked.
    let destination = repository.packages.entry(key).or_insert_with(|| {
        Package::ingested(
            PACKAGE_FORMAT_NPM,
            package.namespace.as_deref(),
            &package.name,
        )
    });
    let room = crate::state::MAX_PACKAGE_VERSIONS_PER_PACKAGE
        .saturating_sub(destination.versions.len());
    let mut retained: Vec<String> = Vec::new();
    for version in versions.into_iter().take(room) {
        let number = version.version.clone();
        destination.versions.insert(
            number.clone(),
            // The copy keeps the ORIGIN of the version it copies: a version's origin is
            // where it entered the DOMAIN, which is the repository it was published into,
            // not the one that retained it. Its own change count starts here, because this
            // is the first change of this repository's copy.
            PackageVersion {
                sequence_number: 1,
                ..version
            },
        );
        retained.push(number);
    }
    // The distribution tags the upstream carries for the versions that were retained: a
    // tag is how a client resolves "the latest version", and it resolves it against the
    // repository it asked.
    for (tag, version) in tags {
        let new_tag = !destination.dist_tags.contains_key(&tag);
        if new_tag && destination.dist_tags.len() >= crate::state::MAX_DIST_TAGS_PER_PACKAGE {
            break;
        }
        if destination.versions.contains_key(&version) {
            destination.dist_tags.insert(tag, version);
        }
    }
    ::tracing::info!(
        domain = %resolved.domain,
        repository = %resolved.repository,
        upstream = %upstream_name,
        package = %package.display_name(),
        versions = retained.len(),
        "a repository retained package versions resolved from an upstream repository"
    );
    record_created(store, resolved, package, &retained, now);
}

/// Record the state change each version in `versions` just underwent in the addressed
/// repository: it came to exist there, whether by a publish into it or by a retention from
/// an upstream.
///
/// Read back off the STORED record, so the revision, status and sequence number the event
/// announces are the ones a later `DescribePackageVersion` reports for the same version.
/// A version the store does not hold contributes nothing: there is no change to announce.
fn record_created(
    store: &mut CaState,
    resolved: &Resolved,
    package: &NpmPackage,
    versions: &[String],
    now: SystemTime,
) {
    let mut changes: Vec<VersionChange> = Vec::new();
    if let Some(repository) = store
        .domain(&resolved.domain, Some(&resolved.domain_owner))
        .and_then(|domain| domain.repositories.get(&resolved.repository))
    {
        if let Some(held) = repository.package(
            PACKAGE_FORMAT_NPM,
            package.namespace.as_deref(),
            &package.name,
        ) {
            for version in versions {
                if let Some(stored) = held.versions.get(version) {
                    changes.push(VersionChange::of(
                        repository,
                        held,
                        stored,
                        OPERATION_CREATED,
                        VersionChanges::published(stored.assets.len(), stored.manifest.is_some()),
                        now,
                    ));
                }
            }
        }
    }
    for change in changes {
        store.record_version_change(change);
    }
}

/// `SystemTime` as RFC 3339, the spelling a packument's `time` map carries.
fn rfc3339(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// One version an npm publish document carries: its number, the metadata declared for it,
/// and the tarball attached to it.
#[derive(Debug)]
struct PublishedVersion {
    version: String,
    manifest: String,
    tarball: Vec<u8>,
}

/// Store the versions an npm publish document carries, into the same records
/// `PublishPackageVersion` writes.
///
/// Every refusal is taken BEFORE anything is stored, so a rejected publish leaves nothing
/// behind: the document has to be readable and to name the package the request addressed,
/// each attached tarball has to be inside the per-asset ceiling, the package's origin
/// control has to permit a direct publish, and a version the repository already holds in a
/// closed status cannot be republished (npm's own `cannot modify pre-existing version`).
fn publish(
    store: &mut CaState,
    resolved: &Resolved,
    package: &NpmPackage,
    document: &[u8],
    now: SystemTime,
) -> Served {
    let published_document = match read_publish_document(package, document) {
        Ok(document) => document,
        Err(refusal) => return refusal,
    };
    let versions = published_document.versions;

    // What the service already retains, read before the records are borrowed mutably:
    // the ceiling is on the TOTAL, so accepted publishes cannot accumulate past it. Both
    // the tarballs and the metadata documents this publish would retain are counted,
    // because both are caller content the store keeps.
    let stored_content_bytes = store.stored_content_bytes();
    let incoming: i64 = versions
        .iter()
        .map(|version| {
            i64::try_from(version.tarball.len().saturating_add(version.manifest.len()))
                .unwrap_or(i64::MAX)
        })
        .sum();
    if stored_content_bytes.saturating_add(incoming) > crate::state::MAX_STORED_ASSET_BYTES {
        return forbidden(format!(
            "this emulator already retains {stored_content_bytes} bytes of package content \
             and its ceiling is {} bytes",
            crate::state::MAX_STORED_ASSET_BYTES
        ));
    }

    let Some(repository) = store
        .domain_mut(&resolved.domain, Some(&resolved.domain_owner))
        .and_then(|domain| domain.repositories.get_mut(&resolved.repository))
    else {
        return not_found("the repository endpoint was not found".to_string());
    };
    let repository_name = repository.name.clone();
    let key = crate::state::package_key(
        PACKAGE_FORMAT_NPM,
        package.namespace.as_deref(),
        &package.name,
    );

    // Origin control decides whether this package accepts a direct publish at all, checked
    // against the STORED package: one that does not exist yet is created with publishing
    // allowed, while one whose first version arrived by ingestion is created with publishing
    // blocked, and `PutPackageOriginConfiguration` is what moves either.
    if repository
        .packages
        .get(&key)
        .is_some_and(|existing| !existing.publish_allowed())
    {
        return publish_blocked();
    }
    if !repository.packages.contains_key(&key)
        && repository.packages.len() >= crate::state::MAX_PACKAGES_PER_REPOSITORY
    {
        return forbidden(format!(
            "repository {repository_name} already holds the maximum of {} packages",
            crate::state::MAX_PACKAGES_PER_REPOSITORY
        ));
    }

    let held = repository.packages.entry(key).or_insert_with(|| {
        // A package record this publish CREATES carries the controls CodeArtifact sets for
        // a package that came to exist by a direct publish: publishing allowed, upstream
        // ingestion blocked.
        Package::published(
            PACKAGE_FORMAT_NPM,
            package.namespace.as_deref(),
            &package.name,
        )
    });
    // A version the repository already holds is immutable, whatever this document says
    // about it; the refusal is taken for the whole document, so a publish is all or
    // nothing.
    for version in &versions {
        if held.versions.contains_key(&version.version) {
            return Served::json(
                409,
                &json!({
                    "error": format!("cannot modify pre-existing version: {}", version.version)
                }),
            );
        }
    }
    if held.versions.len().saturating_add(versions.len())
        > crate::state::MAX_PACKAGE_VERSIONS_PER_PACKAGE
    {
        return forbidden(format!(
            "the package already holds the maximum of {} versions",
            crate::state::MAX_PACKAGE_VERSIONS_PER_PACKAGE
        ));
    }

    for published in &versions {
        held.versions.insert(
            published.version.clone(),
            PackageVersion {
                version: published.version.clone(),
                status: crate::state::PACKAGE_VERSION_STATUS_PUBLISHED.to_string(),
                published_time: now,
                // Published straight into this repository, not fetched from an upstream or
                // an external connection.
                origin_type: crate::state::PACKAGE_VERSION_ORIGIN_INTERNAL.to_string(),
                entry_point_repository: repository_name.clone(),
                // The tarball is the version's ONE asset, under the name this format stores
                // every npm version's content as.
                assets: ::std::collections::BTreeMap::from([(
                    crate::state::NPM_TARBALL_ASSET.to_string(),
                    Asset::new(
                        crate::state::NPM_TARBALL_ASSET,
                        published.tarball.clone(),
                    ),
                )]),
                // The metadata document the publisher declared for this version, kept as
                // the version's manifest rather than as an asset: it is what every member
                // read off a version's metadata is read from.
                manifest: Some(published.manifest.clone()),
                // This publish is the first change of this version.
                sequence_number: 1,
            },
        );
    }
    // The document's own dist-tags, for the versions this package actually holds: a tag is
    // a pointer the publisher set, and a pointer to a version nothing holds would resolve
    // to nothing. A tag the package does not already carry is a new entry in its tag map,
    // so the same ceiling bounds it here.
    for (tag, version) in published_document.dist_tags {
        let new_tag = !held.dist_tags.contains_key(&tag);
        if new_tag && held.dist_tags.len() >= crate::state::MAX_DIST_TAGS_PER_PACKAGE {
            break;
        }
        if held.versions.contains_key(&version) {
            held.dist_tags.insert(tag, version);
        }
    }

    ::tracing::info!(
        domain = %resolved.domain,
        repository = %repository_name,
        package = %package.display_name(),
        versions = versions.len(),
        "a package manager published npm package versions over the repository endpoint"
    );
    // Every version this publish created is a state change of that version, announced on
    // the account's default event bus once this request has released the store.
    let created: Vec<String> = versions
        .iter()
        .map(|published| published.version.clone())
        .collect();
    record_created(store, resolved, package, &created, now);
    Served::json(
        200,
        &json!({ "ok": true, "success": true, "id": package.display_name() }),
    )
}

/// What an npm publish document carries: the versions it publishes and the distribution
/// tags it sets.
#[derive(Debug)]
struct PublishDocument {
    versions: Vec<PublishedVersion>,
    /// The `dist-tags` object, as the pairs it declares.
    dist_tags: Vec<(String, String)>,
}

/// An npm publish document as the wire carries it: npm's own publish body — the package
/// `name`, a `versions` object keyed by version number, a `dist-tags` object, and an
/// `_attachments` object carrying each version's tarball base64-encoded (npm registry
/// publish API).
///
/// Each version's metadata is captured as the RAW text the document declared it with
/// (`RawValue`), never as a re-serialised value: a dependency list is reported in the order
/// it was declared, and re-serialising through a map would be re-ordering it.
#[derive(Debug, ::serde::Deserialize)]
struct WirePublishDocument<'a> {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, borrow)]
    versions: Option<::std::collections::BTreeMap<String, &'a ::serde_json::value::RawValue>>,
    #[serde(default, rename = "dist-tags")]
    dist_tags: Option<::std::collections::BTreeMap<String, String>>,
    #[serde(default, rename = "_attachments")]
    attachments: Option<::std::collections::BTreeMap<String, WireAttachment>>,
}

/// One attachment of a publish document: the tarball's bytes, base64-encoded.
#[derive(Debug, ::serde::Deserialize)]
struct WireAttachment {
    #[serde(default)]
    data: Option<String>,
}

/// The version a manifest declares for itself, which must agree with the key it is written
/// under.
#[derive(Debug, ::serde::Deserialize)]
struct WireManifestVersion {
    #[serde(default)]
    version: Option<String>,
}

/// What an npm publish document carries, or the refusal it earns.
///
/// Everything is read from the document — nothing is read out of the tarball — and each
/// version's metadata is kept as the text it was declared with, so what a later packument
/// and a later dependency listing report is what was published.
fn read_publish_document(
    package: &NpmPackage,
    document: &[u8],
) -> Result<PublishDocument, Served> {
    use ::base64::Engine as _;

    if document.len() > MAX_PUBLISH_DOCUMENT_BYTES {
        return Err(Served::json(
            413,
            &json!({
                "error": format!(
                    "the publish document is {} bytes, more than this emulator's maximum of \
                     {MAX_PUBLISH_DOCUMENT_BYTES} bytes",
                    document.len()
                )
            }),
        ));
    }
    let Ok(wire) = ::serde_json::from_slice::<WirePublishDocument<'_>>(document) else {
        return Err(bad_request(
            "the publish document is not a JSON object npm's publish body".to_string(),
        ));
    };
    let declared = wire.name.unwrap_or_default();
    if declared != package.display_name() {
        return Err(bad_request(format!(
            "the publish document names {declared:?}, which is not the package {:?} the \
             request addressed",
            package.display_name()
        )));
    }
    let versions = wire.versions.unwrap_or_default();
    if versions.is_empty() {
        return Err(bad_request(
            "the publish document carries no version".to_string(),
        ));
    }
    // The COUNT of versions is bounded before any attachment is decoded: each version the
    // document declares costs a few bytes of the document budget and would materialise a
    // whole decoded tarball, so a document naming more versions than a package may ever
    // hold is refused here rather than after the copies exist. The same ceiling the store
    // enforces per package, enforced on the way in.
    if versions.len() > crate::state::MAX_PACKAGE_VERSIONS_PER_PACKAGE {
        return Err(forbidden(format!(
            "the publish document carries {} versions, more than the maximum of {} a package \
             holds",
            versions.len(),
            crate::state::MAX_PACKAGE_VERSIONS_PER_PACKAGE
        )));
    }
    let attachments = wire.attachments.unwrap_or_default();
    // What this document has decoded so far, so a multi-version document cannot materialise
    // more content than the store could ever retain: the budget is checked as each version
    // is decoded, so the first version past it refuses the request instead of the check
    // happening once every copy is already in memory.
    let mut decoded_bytes: i64 = 0;

    let mut published = Vec::new();
    for (version, manifest) in &versions {
        let manifest_text = manifest.get();
        let declared_version = ::serde_json::from_str::<WireManifestVersion>(manifest_text)
            .ok()
            .and_then(|manifest| manifest.version);
        if let Some(declared) = declared_version {
            if &declared != version {
                return Err(bad_request(format!(
                    "the manifest under version {version:?} declares version {declared:?}"
                )));
            }
        }
        let Some(attachment) = attachment_of(&attachments, package, version, versions.len())
        else {
            return Err(bad_request(format!(
                "the publish document attaches no tarball for version {version:?}"
            )));
        };
        let Some(encoded) = attachment.data.as_deref() else {
            return Err(bad_request(format!(
                "the tarball attached for version {version:?} carries no data"
            )));
        };
        let Ok(tarball) = ::base64::engine::general_purpose::STANDARD.decode(encoded) else {
            return Err(bad_request(format!(
                "the tarball attached for version {version:?} is not base64"
            )));
        };
        if tarball.len() > crate::state::MAX_ASSET_BYTES {
            return Err(Served::json(
                413,
                &json!({
                    "error": format!(
                        "the tarball for version {version:?} is {} bytes, more than this \
                         emulator's maximum of {} bytes",
                        tarball.len(),
                        crate::state::MAX_ASSET_BYTES
                    )
                }),
            ));
        }
        // The metadata document is retained beside the tarball, so it carries its own
        // ceiling: an npm `package.json` is kilobytes, and a document past this bound is
        // not metadata this registry stores.
        if manifest_text.len() > MAX_MANIFEST_BYTES {
            return Err(Served::json(
                413,
                &json!({
                    "error": format!(
                        "the metadata document for version {version:?} is {} bytes, more than \
                         this emulator's maximum of {MAX_MANIFEST_BYTES} bytes",
                        manifest_text.len()
                    )
                }),
            ));
        }
        // Counted BEFORE the copy is kept, so the budget bounds what this document has
        // materialised rather than what it managed to retain.
        decoded_bytes = decoded_bytes.saturating_add(
            i64::try_from(tarball.len().saturating_add(manifest_text.len())).unwrap_or(i64::MAX),
        );
        if decoded_bytes > crate::state::MAX_STORED_ASSET_BYTES {
            return Err(forbidden(format!(
                "the publish document carries {decoded_bytes} bytes of package content, more \
                 than this emulator's ceiling of {} bytes",
                crate::state::MAX_STORED_ASSET_BYTES
            )));
        }
        published.push(PublishedVersion {
            version: version.clone(),
            manifest: manifest_text.to_string(),
            tarball,
        });
    }
    Ok(PublishDocument {
        versions: published,
        dist_tags: wire.dist_tags.unwrap_or_default().into_iter().collect(),
    })
}

/// The attachment carrying the tarball of `version`.
///
/// npm keys an attachment by the tarball's file name, which it writes with the package's
/// FULL name — `@scope/name-1.0.0.tgz` for a scoped package and `name-1.0.0.tgz` otherwise
/// — so both spellings are looked for. A document carrying exactly one attachment and
/// naming it some third way is read as that version's ONLY when the document declares a
/// single version (`declared_versions`), because that is the document a single-version
/// publish sends: in a multi-version document the same attachment would otherwise be read as
/// every version's, which both stores one version's content as another's and makes one
/// attachment decode once per version.
fn attachment_of<'a>(
    attachments: &'a ::std::collections::BTreeMap<String, WireAttachment>,
    package: &NpmPackage,
    version: &str,
    declared_versions: usize,
) -> Option<&'a WireAttachment> {
    let candidates = [
        format!(
            "{}-{version}{NPM_TARBALL_EXTENSION}",
            package.display_name()
        ),
        package.tarball_file_name(version),
    ];
    candidates.iter().find_map(|name| attachments.get(name)).or_else(|| {
        (declared_versions == 1 && attachments.len() == 1)
            .then(|| attachments.values().next())?
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// One rendered answer: the status, its headers and its body, as the wire carries them.
fn rendered(served: &Served) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(served.status).unwrap_or_else(|_| {
            StatusCode::try_from(500).expect("500 is a status")
        }),
        SdkBody::from(served.body.clone()),
    );
    if let Some(content_type) = &served.content_type {
        if let Err(refused) = response
            .headers_mut()
            .try_insert("content-type", content_type.clone())
        {
            ::tracing::warn!(
                error = %refused,
                "the media type cannot be sent on the wire and is left off the response"
            );
        }
    }
    if let Some(challenge) = &served.authenticate {
        if let Err(refused) = response
            .headers_mut()
            .try_insert("www-authenticate", challenge.clone())
        {
            ::tracing::warn!(
                error = %refused,
                "the authentication challenge cannot be sent on the wire and is left off the \
                 response"
            );
        }
    }
    response
}

// ---------------------------------------------------------------------------
// The operation a repository endpoint's traffic is served as
// ---------------------------------------------------------------------------

/// The ONE operation this service serves a repository endpoint's own traffic with.
///
/// A repository endpoint's traffic names no modelled operation — no modelled operation is
/// bound under a repository endpoint's path — so the service RESOLVES it to this one
/// ([`crate::service::EmulatedService::override_operation`]) rather than letting it fall
/// through to a routing refusal. Being an operation is what gives the npm plane the two
/// things a routing refusal cannot have:
///
/// * the request's own callback into this emulator, so the package version state changes a
///   publish or a retention committed are announced on the account's default event bus
///   ([`crate::state::package_version_events`]) the way every other service announces its
///   own events;
/// * `auth_schemes` of its own, which is what says this request is NOT SigV4-signed: the
///   credential an npm client presents is the bearer token `GetAuthorizationToken` minted,
///   resolved by [`serve`] against the tokens this service remembers minting, so the
///   framework has no signature to verify and refuses nothing before the handler runs.
pub struct NpmRegistryEndpoint<Eng> {
    /// The SAME shared engine every operation of this service holds: a version published
    /// over this endpoint is a version of the repository the control plane describes.
    engine: Arc<Eng>,
}

impl<Eng> NpmRegistryEndpoint<Eng> {
    /// Build the endpoint's operation from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ::smithy_server_runtime::ErasedOperation
    for NpmRegistryEndpoint<Eng>
{
    fn run<'r>(
        &'r self,
        _svc: &'r dyn ::smithy_server_runtime::Service,
        req: &'r RtRequest,
        ctx: &'r ::smithy_server_runtime::OperationContext<'r>,
    ) -> ::smithy_server_runtime::BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The answer is composed with no store lock held past it — [`serve`] takes and
            // releases the guard inside itself — so the announcement that follows cannot
            // hold this request's own store.
            let response = serve(&*self.engine, req);
            crate::state::package_version_events::publish(ctx, &*self.engine).await;
            response
        })
    }

    /// Served UNSIGNED: an npm client presents the repository's bearer token, not a SigV4
    /// signature, so there is no signature for the framework to verify. The credential is
    /// not skipped — [`serve`] resolves the presented token against the tokens this
    /// service minted and answers `401` for anything else.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the endpoint-host table: the case, the host, and the `(domain, account)`
    /// it names.
    type HostRow = (
        &'static str,
        &'static str,
        Option<(&'static str, &'static str)>,
    );

    /// RULE: a published repository-endpoint host names the domain and the owning account;
    /// every other host names none. The rows span what the reading generalises over: the
    /// IPv4 and dual-stack suffixes, a domain name that itself carries `-`, and the hosts
    /// this service's own API is reached at.
    #[test]
    fn an_endpoint_host_names_the_domain_and_the_account_that_owns_it() {
        let rows: &[HostRow] = &[
            (
                "the published IPv4 endpoint",
                "acme-000000000000.d.codeartifact.us-west-2.amazonaws.com",
                Some(("acme", "000000000000")),
            ),
            (
                "the dual-stack endpoint",
                "acme-000000000000.d.codeartifact.us-west-2.on.aws",
                Some(("acme", "000000000000")),
            ),
            (
                "a domain name carrying a hyphen",
                "sim2real-ca-repo-123456789012.d.codeartifact.eu-west-1.amazonaws.com",
                Some(("sim2real-ca-repo", "123456789012")),
            ),
            (
                "this service's own API endpoint",
                "codeartifact.us-east-1.amazonaws.com",
                None,
            ),
            ("the emulator's own address", "127.0.0.1", None),
            (
                "a label whose last field is not an account id",
                "acme-notanaccount.d.codeartifact.us-east-1.amazonaws.com",
                None,
            ),
            (
                "another service's endpoint",
                "acme-000000000000.s3.us-east-1.amazonaws.com",
                None,
            ),
        ];
        for (case, host, expected) in rows {
            let named = endpoint_host_domain(host);
            assert_eq!(
                named, *expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
    }

    /// RULE: a registry path and a method name one npm route. A scoped package is addressed
    /// in ONE percent-encoded segment on every route that carries the package name alone,
    /// and as two path segments only in a tarball's path — which is the URL this registry
    /// publishes.
    #[test]
    fn a_path_and_a_method_name_one_npm_route() {
        let scoped = NpmPackage {
            namespace: Some("acme".to_string()),
            name: "widget".to_string(),
            encoded: true,
        };
        let plain = NpmPackage {
            namespace: None,
            name: "left-pad".to_string(),
            encoded: true,
        };
        // (case, method, path, what it asks for rendered as a short description)
        let rows: &[(&str, &str, &str, Option<String>)] = &[
            ("the ping route", "GET", "/-/ping", Some("ping".to_string())),
            (
                "a packument",
                "GET",
                "/left-pad",
                Some(format!("packument {}", plain.display_name())),
            ),
            (
                "a publish",
                "PUT",
                "/left-pad",
                Some(format!("publish {}", plain.display_name())),
            ),
            (
                "a scoped publish under its encoded namespace",
                "PUT",
                "/@acme%2fwidget",
                Some(format!("publish {}", scoped.display_name())),
            ),
            (
                "a scoped packument under its encoded namespace",
                "GET",
                "/@acme%2Fwidget",
                Some(format!("packument {}", scoped.display_name())),
            ),
            (
                "a scoped packument whose scope is not encoded",
                "GET",
                "/@acme/widget",
                Some("unencoded scope @acme".to_string()),
            ),
            (
                "a scoped publish whose scope is not encoded",
                "PUT",
                "/@acme/widget",
                Some("unencoded scope @acme".to_string()),
            ),
            (
                "a tarball",
                "GET",
                "/left-pad/-/left-pad-1.0.0.tgz",
                Some("tarball left-pad-1.0.0.tgz".to_string()),
            ),
            (
                "a scoped tarball, whose path carries the scope as a segment",
                "GET",
                "/@acme/widget/-/widget-1.0.0.tgz",
                Some("tarball widget-1.0.0.tgz".to_string()),
            ),
            (
                "the dist-tag collection",
                "GET",
                "/-/package/left-pad/dist-tags",
                Some(format!("read dist-tags {}", plain.display_name())),
            ),
            (
                "setting one dist-tag",
                "PUT",
                "/-/package/@acme%2fwidget/dist-tags/beta",
                Some(format!("set beta {}", scoped.display_name())),
            ),
            (
                "deleting one dist-tag",
                "DELETE",
                "/-/package/left-pad/dist-tags/beta",
                Some(format!("delete beta {}", plain.display_name())),
            ),
            ("a registry route not served", "GET", "/-/whoami", None),
            ("a method not served", "DELETE", "/left-pad", None),
            ("the endpoint root", "GET", "/", None),
        ];
        for (case, method, path, expected) in rows {
            let described = NpmRequest::of(method.to_string(), path).map(|request| match request {
                NpmRequest::Ping => "ping".to_string(),
                NpmRequest::Packument(package) => format!("packument {}", package.display_name()),
                NpmRequest::Publish(package) => format!("publish {}", package.display_name()),
                NpmRequest::Tarball(_, file) => format!("tarball {file}"),
                NpmRequest::ReadDistTags(package) => {
                    format!("read dist-tags {}", package.display_name())
                }
                NpmRequest::SetDistTag(package, tag) => {
                    format!("set {tag} {}", package.display_name())
                }
                NpmRequest::DeleteDistTag(package, tag) => {
                    format!("delete {tag} {}", package.display_name())
                }
                NpmRequest::UnencodedScope(named) => format!("unencoded scope {named}"),
            });
            assert_eq!(
                described, *expected,
                "case {case:?}: expected {expected:?}, got {described:?}"
            );
        }
    }

    /// RULE: a version's status decides whether its assets are downloadable — `Published`
    /// and `Unlisted` serve them, every other status withholds them.
    #[test]
    fn only_a_published_or_unlisted_version_serves_its_assets() {
        let rows: &[(&str, bool)] = &[
            (crate::state::PACKAGE_VERSION_STATUS_PUBLISHED, true),
            (crate::state::PACKAGE_VERSION_STATUS_UNLISTED, true),
            (crate::state::PACKAGE_VERSION_STATUS_ARCHIVED, false),
            (crate::state::PACKAGE_VERSION_STATUS_DISPOSED, false),
            (crate::state::PACKAGE_VERSION_STATUS_UNFINISHED, false),
        ];
        for (status, expected) in rows {
            let reached = downloadable(status);
            assert_eq!(
                reached, *expected,
                "case {status:?}: expected downloadable={expected}, got {reached}"
            );
        }
    }

    /// RULE: only an `Authorization` header presenting the `Bearer` scheme carries a token
    /// this registry resolves.
    #[test]
    fn only_a_bearer_header_presents_a_token() {
        let rows: &[(&str, Option<&str>, Option<&str>)] = &[
            ("a bearer token", Some("Bearer abc.def"), Some("abc.def")),
            ("the scheme in another case", Some("bearer abc"), Some("abc")),
            ("basic authentication", Some("Basic dXNlcjpwYXNz"), None),
            ("a SigV4 signature", Some("AWS4-HMAC-SHA256 Credential=x"), None),
            ("an empty token", Some("Bearer  "), None),
            ("no header at all", None, None),
        ];
        for (case, header, expected) in rows {
            let read = bearer_token(*header);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}

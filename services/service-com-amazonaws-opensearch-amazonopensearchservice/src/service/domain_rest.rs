// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN's own REST data plane: how a request a client addresses to the endpoint a
//! domain of this service published is recognised, and how it is answered out of the
//! indexes, templates and security configuration that domain holds.
//!
//! A domain this service creates publishes the host a client sends index, search and
//! cluster requests to — `DomainStatus$Endpoint`, "the domain-specific endpoint used to
//! submit index, search, and data upload requests" — and that host is NOT this API's
//! endpoint: its first label is the domain's own, and the path it carries names no modelled
//! operation. Both halves of the front door's resolution therefore missed it, and every
//! signed request to the one address this service told the caller to use was answered `404
//! UnknownService`. Nothing served a domain's own traffic at all, so a client that
//! provisions a domain and then polls it waits for an answer that never comes.
//!
//! So this module is the second half of [`crate::service::EmulatedService`]: the claim
//! ([`claims_parts`], read by `Service::can_handle`) and the serving ([`serve`], reached
//! from `Service::override_operation` for a request carrying a SigV4 credential — so the
//! router verifies that credential before anything is served — and from
//! `Service::override_routing_error` for one carrying none).
//!
//! # What a request addresses
//!
//! A client on the machine this emulator runs on resolves a domain's endpoint host
//! nowhere at all, so two addressings resolve:
//!
//! * HOST-STYLE, the addressing `DomainStatus$Endpoint` publishes: the request's host IS
//!   the endpoint of a domain this store holds (or one of the VPC-access hosts
//!   `DomainStatus$Endpoints` publishes), and the whole path is the request path;
//! * PREFIXED PATH-STYLE: the data plane is named by the reserved namespace segment
//!   (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`) and
//!   [`DATA_PLANE_SEGMENT`], then the domain's name or its endpoint host, then the request
//!   path — `/_aws/es/{domain-or-host}/{path}`. That segment begins with `_`, which no
//!   modelled path segment of any service does, so the prefix collides with no
//!   control-plane route.
//!
//! Either way the claim is narrowed to a domain this service HOLDS, read from the one
//! shared store, so one request keeps one claimant — this service's signing name is shared
//! with the legacy Amazon Elasticsearch Service model, whose own modelled path space is
//! disjoint from both addressings above.
//!
//! # What it answers, and what decides it
//!
//! Every status, header and byte is computed from what a create, an update or a write
//! persisted: the cluster banner and the cluster health from the domain's own
//! configuration, an index's settings and mappings from the index record, a search and a
//! count from the documents the index holds. A read writes NOTHING; the writes are the
//! requests that are writes — an index create, a document write, a template or a security
//! record.
//!
//! Two authorization decisions, in the order AWS takes them, and neither re-implemented
//! here:
//!
//! 1. IAM, at the service's front door: the principal's identity policies and the domain's
//!    own access policy, against the data-plane action the request's METHOD names
//!    (`es:ESHttpGet`, `es:ESHttpPost`, …) on the domain resource its PATH names — the one
//!    decision [`crate::state::ingest::iam_decision`] owns.
//! 2. The CLUSTER: on a domain with fine-grained access control switched on, a principal
//!    that is neither the master user nor bound by a role mapping may do nothing, which is
//!    the one decision [`crate::state::fgac`] owns. What that decision does NOT narrow is
//!    which of the actions below a MAPPED principal may perform: a role's own
//!    `index_permissions`/`cluster_permissions` are stored and reported back by the
//!    security API, and are not evaluated, so a principal any role mapping binds is
//!    permitted every action this surface names.
//!
//! The REST surface served is the one this emulator can answer out of persisted state:
//! the cluster banner and health, an index's create/read/delete, a document write, read
//! and delete, a count and a search over a bounded query vocabulary ([`selector`]), index
//! templates, and the security plugin's role and role-mapping API. A path outside it is
//! answered the way the engine answers a path no handler is registered for, rather than
//! with an invented body.

use std::collections::BTreeMap;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use serde_json::{json, Map, Value};
use smithy_server_runtime::{http, OperationContext};

use ::service_common::data_plane_addressing::{
    first_segment, host_only, EMULATOR_PATH_NAMESPACE,
};

use crate::state::{
    self, fgac, ingest, service_state, ComAmazonawsOpensearchAmazonopensearchserviceState as Store,
    Domain, IndexTemplate,
};

/// The segment that names THIS service's data plane behind `EMULATOR_PATH_NAMESPACE`: the
/// `arnNamespace`/`endpointPrefix` this service publishes.
const DATA_PLANE_SEGMENT: &str = state::ARN_NAMESPACE;

/// The most bytes one data-plane request body is read as JSON.
///
/// The emulator's OWN bound, like [`crate::state::INDEX_QUOTA`]: every byte lives in this
/// process's heap, so a body past it is refused rather than parsed. A document write is
/// bounded again, more tightly, by [`ingest::MAX_DOCUMENT_BYTES`].
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

/// How many index templates one domain may hold before a write is refused — the same kind
/// of bound [`crate::state::INDEX_QUOTA`] is, for the same reason.
pub const TEMPLATE_QUOTA: usize = 100;

/// How many roles, and how many role mappings, one domain's security configuration may
/// hold before a write is refused — the same kind of bound, for the same reason: a caller
/// looping with fresh names must not grow this process's state without end.
pub const SECURITY_RECORD_QUOTA: usize = 100;

/// The most hits one search answers with when the request names no `size`, which is the
/// engine's own default.
const DEFAULT_SEARCH_SIZE: usize = 10;

/// The most hits one search answers with however large a `size` it names — the engine's own
/// `index.max_result_window` default, which bounds `from + size`.
const MAX_RESULT_WINDOW: usize = 10_000;

/// How far a `bool` query may nest before the engine refuses it — its own
/// `indices.query.bool.max_nested_depth` default.
const MAX_BOOL_NESTING: usize = 20;

/// The most shards and replica copies one index's settings may ask for, as the engine's
/// own `index.number_of_shards` limit (`cluster.max_shards_per_node` times a node) bounds
/// them. A settings value past this is clamped for the health arithmetic rather than
/// multiplied out.
const MAX_SHARDS_PER_INDEX: i64 = 1_024;

/// The engine's error type for a request whose body is not the JSON the handler expects.
const PARSE_EXCEPTION: &str = "parse_exception";

/// The engine's error type for a query clause the request names and the engine does not
/// register.
const PARSING_EXCEPTION: &str = "parsing_exception";

/// The engine's error type for an index that does not exist.
const INDEX_NOT_FOUND_EXCEPTION: &str = "index_not_found_exception";

/// The engine's error type for a create of an index that already exists.
const RESOURCE_ALREADY_EXISTS_EXCEPTION: &str = "resource_already_exists_exception";

/// The engine's error type for a name no index may carry.
const INVALID_INDEX_NAME_EXCEPTION: &str = "invalid_index_name_exception";

/// The engine's error type for a write that may not replace a document the index already
/// holds.
const VERSION_CONFLICT_ENGINE_EXCEPTION: &str = "version_conflict_engine_exception";

/// The most comma-separated index patterns one request may name.
///
/// The emulator's OWN bound on how much work one request's index resolution does, beside
/// the de-duplication below: the resolved set is bounded by the indexes a domain holds, so
/// this bounds the resolution itself.
const MAX_NAMED_INDEX_PATTERNS: usize = 100;

/// The most documents one count or search VISITS, which is the whole store at most once:
/// every index a domain may hold ([`state::INDEX_QUOTA`]) times the documents one index may
/// hold ([`ingest::INDEX_DOCUMENT_QUOTA`]). The de-duplicated index set cannot exceed it,
/// so this is a proof rather than a cut-off, and a request that would is refused rather
/// than scanning further.
const MAX_DOCUMENTS_VISITED: usize = state::INDEX_QUOTA * ingest::INDEX_DOCUMENT_QUOTA;

/// One hit a search answers with: the index the document came from, its id and its source.
type Hit = (String, String, Value);

/// How many documents a request SELECTED, and the page of hits it asked for.
type Selected = (usize, Vec<Hit>);

/// What a request ADDRESSES, as the wire carries it: the host it was sent to — absent on a
/// request that names neither a `Host` header nor an absolute URI — and its path.
///
/// Built from BOTH shapes the router hands this module, the buffered parts the claim reads
/// and the runtime-API request the serving seam reads, so one rule resolves both and the
/// claim cannot resolve a different domain than the answer does.
#[derive(Debug)]
struct Addressed<'a> {
    host: Option<&'a str>,
    path: &'a str,
}

impl<'a> Addressed<'a> {
    /// The addressing of the buffered request these parts came from.
    fn of_parts(parts: &'a http::request::Parts) -> Self {
        Self {
            host: parts
                .headers
                .get("host")
                .and_then(|host| host.to_str().ok())
                .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str)),
            path: parts.uri.path(),
        }
    }

    /// The addressing of a runtime-API request, whose URI arrives as a string and is parsed
    /// by the `http` crate rather than split by hand.
    fn of_request(req: &'a RtRequest, uri: &'a http::Uri) -> Self {
        Self {
            host: req
                .headers()
                .get("host")
                .or_else(|| uri.authority().map(http::uri::Authority::as_str)),
            path: uri.path(),
        }
    }
}

/// What a request resolves to: the domain it addresses, and the path WITHIN that domain's
/// own endpoint — which is what the REST surface is read against.
#[derive(Debug)]
struct Resolved {
    domain_name: String,
    request_path: String,
}

/// The request URI of a runtime-API request, parsed by the `http` crate. A URI this parse
/// rejects is one the server would not have accepted, and it resolves to `http::Uri`'s own
/// default (`/`), which names no domain and claims nothing.
fn parsed_uri(uri: &str) -> http::Uri {
    uri.parse::<http::Uri>().unwrap_or_default()
}

/// Whether this data plane serves the request these `parts` came from — read by
/// `Service::can_handle`, so a request addressed to a domain this service holds reaches
/// this service instead of the front door's `UnknownService`.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    resolve(engine, &Addressed::of_parts(parts)).is_some()
}

/// [`claims_parts`] for the runtime-API request the serving seams are handed, resolved by
/// the same rule.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let uri = parsed_uri(req.uri());
    resolve(engine, &Addressed::of_request(req, &uri)).is_some()
}

/// Whether `req` presents a SigV4 credential, which is what decides whether the router
/// verifies a signature before this plane serves it.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    req.headers().get("authorization").is_some_and(|value| {
        value
            .get(..SIGV4_SCHEME.len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(SIGV4_SCHEME))
    }) || req
        .uri()
        .split(['?', '&'])
        .any(|parameter| parameter.starts_with("X-Amz-Signature="))
}

/// The scheme an AWS API call's credential is presented under.
const SIGV4_SCHEME: &str = "AWS4-HMAC-SHA256";

/// Which domain `addressed` addresses, and the path within its endpoint, or `None` when
/// this data plane serves the request not at all.
fn resolve<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> Option<Resolved> {
    let state = service_state(engine)?;
    let store = state.lock();

    // PREFIXED PATH-STYLE, the one addressing that is explicit about naming this plane,
    // and so the one read whatever the request's host is.
    if let Some((named, path)) = prefixed_addressing(addressed.path) {
        let domain = addressed_domain(&store, named)?;
        return Some(Resolved {
            domain_name: domain,
            request_path: path.to_string(),
        });
    }
    // A path under the reserved namespace that did NOT name this plane names another one,
    // which serves its own requests.
    if first_segment(addressed.path).is_some_and(|(first, _)| first == EMULATOR_PATH_NAMESPACE) {
        return None;
    }
    // A path this model's own routing table serves is a CONTROL-plane request, claimed by
    // the service's modelled path space rather than by this plane, whatever host it
    // reached.
    if first_segment(addressed.path)
        .is_some_and(|(first, _)| crate::ROUTE_PATH_SEGMENTS.contains(&first))
    {
        return None;
    }
    // HOST-STYLE: the host IS a host some domain of this store publishes.
    let host = host_only(addressed.host?);
    let domain = store
        .domains
        .values()
        .find(|domain| publishes_host(domain, host))?;
    Some(Resolved {
        domain_name: domain.domain_name.clone(),
        request_path: addressed.path.to_string(),
    })
}

/// Whether `domain` publishes `host`: its public endpoint, or one of the VPC-access hosts
/// `DomainStatus$Endpoints` reports for a domain provisioned inside a VPC.
fn publishes_host(domain: &Domain, host: &str) -> bool {
    if domain.endpoint == host {
        return true;
    }
    domain
        .vpc_access_endpoints()
        .is_some_and(|endpoints| endpoints.values().any(|published| published == host))
}

/// The domain a prefixed addressing NAMES: its own `DomainName`, or a host it publishes.
fn addressed_domain(store: &Store, named: &str) -> Option<String> {
    if store.domains.contains_key(named) {
        return Some(named.to_string());
    }
    store
        .domains
        .values()
        .find(|domain| publishes_host(domain, named))
        .map(|domain| domain.domain_name.clone())
}

/// The domain a prefixed path names and the request path that follows it, or `None` when
/// the path is not of that shape.
fn prefixed_addressing(path: &str) -> Option<(&str, &str)> {
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != DATA_PLANE_SEGMENT {
        return None;
    }
    let (named, request_path) = first_segment(rest)?;
    (!named.is_empty()).then_some((named, request_path))
}

/// Answer a request this plane claimed, out of the records the control plane holds.
///
/// `ctx` is present for a request the router named an operation for, which is every request
/// carrying a credential; it is what the calling principal is resolved from.
pub fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: Option<&OperationContext<'_>>,
) -> RtResponse {
    let uri = parsed_uri(req.uri());
    let Some(resolved) = resolve(engine, &Addressed::of_request(req, &uri)) else {
        // The domain was released by another request between the claim and here, so the
        // host it published answers nothing.
        return error(503, "cluster_block_exception", "blocked by: [SERVICE_UNAVAILABLE]");
    };
    let method = req.method().to_ascii_uppercase();
    let body = req.body().bytes().unwrap_or_default();
    if body.len() > MAX_REQUEST_BODY_BYTES {
        return error(
            413,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "request body is larger than the maximum of {MAX_REQUEST_BODY_BYTES} bytes \
                 this cluster takes"
            ),
        );
    }
    // The principal the two authorization decisions are taken for. An empty ARN is a
    // request this emulator can resolve no identity for — an engine carrying no STS or IAM
    // store, or an expired session — which no role mapping binds, so fine-grained access
    // control refuses it exactly as it refuses an unmapped one.
    let principal = ctx
        .and_then(|ctx| state::authorization::request_principal_arn(engine, ctx))
        .unwrap_or_default();
    // The identity policies live in ANOTHER service's store, so they are read before this
    // one is locked: no call holds two service stores at once. `None` is a caller this
    // emulator holds no policy evidence for — the account's own credential, or a grant it
    // cannot read — which is the posture `crate::state::authorization` documents.
    let identity = ctx.and_then(|ctx| state::authorization::request_identity_documents(engine, ctx));
    let request = Request {
        method: &method,
        path: &resolved.request_path,
        query: uri.query().unwrap_or_default(),
        body,
        principal: &principal,
        identity: identity.as_ref(),
    };
    let Some(state) = service_state(engine) else {
        return error(503, "cluster_block_exception", "blocked by: [SERVICE_UNAVAILABLE]");
    };
    let now = SystemTime::now();
    let mut store = state.lock();
    // The read materialises any configuration change the clock already owes this domain,
    // which this request neither starts nor changes.
    let Some(domain) = store.settled_mut(&resolved.domain_name, now) else {
        return error(503, "cluster_block_exception", "blocked by: [SERVICE_UNAVAILABLE]");
    };
    if !domain.created {
        return error(
            503,
            "cluster_block_exception",
            "blocked by: [SERVICE_UNAVAILABLE/1/state not recovered / initialized]",
        );
    }
    serve_resolved(domain, &request, now)
}

/// One data-plane request, as the surface below reads it.
struct Request<'a> {
    method: &'a str,
    path: &'a str,
    query: &'a str,
    body: &'a [u8],
    /// The ARN of the principal the request speaks for, empty when none was resolved.
    principal: &'a str,
    /// The identity-based policy documents in force on that principal, or `None` when this
    /// emulator holds none for it.
    identity: Option<&'a Vec<String>>,
}

impl Request<'_> {
    /// The path's segments, with empty ones dropped so a trailing or doubled `/` names no
    /// segment of its own.
    fn segments(&self) -> Vec<&str> {
        self.path.split('/').filter(|part| !part.is_empty()).collect()
    }

    /// The value the query string gives `name`, or `None`.
    fn parameter(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then_some(value)
        })
    }

    /// The request body as the JSON object the handler expects, `None` for an empty body.
    ///
    /// The refusal is BOXED, as every fallible step of this module's reading is: a rendered
    /// response is far larger than what these functions answer with, and a `Result` whose
    /// error dominates its size is paid for on the success path too.
    fn json(&self) -> Result<Option<Value>, Box<RtResponse>> {
        if self.body.is_empty() {
            return Ok(None);
        }
        ::serde_json::from_slice(self.body)
            .map(Some)
            .map_err(|failed| {
                Box::new(error(
                    400,
                    PARSE_EXCEPTION,
                    &format!("failed to parse: {failed}"),
                ))
            })
    }

    /// This service's data-plane IAM action for the request's method: the `ESHttp*` actions
    /// of the IAM Service Authorization Reference, one per HTTP method.
    fn iam_action(&self) -> &'static str {
        match self.method {
            "GET" => "ESHttpGet",
            "PUT" => "ESHttpPut",
            "POST" => "ESHttpPost",
            "DELETE" => "ESHttpDelete",
            "HEAD" => "ESHttpHead",
            "PATCH" => "ESHttpPatch",
            _ => "ESHttpGet",
        }
    }
}

/// Answer the request against the domain it resolved to.
fn serve_resolved(domain: &mut Domain, request: &Request<'_>, now: SystemTime) -> RtResponse {
    // IAM first, as AWS takes it: the resource is the domain path the request addresses.
    let resource = format!("{}/{}", domain.arn, request.path.trim_start_matches('/'));
    if let Err(refusal) = ingest::iam_decision(
        domain,
        request.principal,
        request.iam_action(),
        &resource,
        request.identity,
    ) {
        return refused(&refusal);
    }
    let segments = request.segments();
    match segments.as_slice() {
        [] => match request.method {
            "GET" | "HEAD" => guarded(domain, request, fgac::ACTION_CLUSTER_STATE, |domain| {
                ok(&banner(domain))
            }),
            _ => wrong_method(request, "GET, HEAD"),
        },
        ["_cluster", "health"] => match request.method {
            "GET" => guarded(domain, request, fgac::ACTION_CLUSTER_HEALTH, |domain| {
                cluster_health(domain, request, None)
            }),
            _ => wrong_method(request, "GET"),
        },
        ["_cluster", "health", index] => match request.method {
            "GET" => guarded(domain, request, fgac::ACTION_CLUSTER_HEALTH, |domain| {
                cluster_health(domain, request, Some(index))
            }),
            _ => wrong_method(request, "GET"),
        },
        ["_index_template", name] => template_request(domain, request, name, false),
        ["_template", name] => template_request(domain, request, name, true),
        // The security plugin's own "who am I". It is NOT guarded: the plugin answers it
        // for the authenticated user whatever roles that user holds, which is what makes
        // it the route a client checks its own identity and permissions with before it
        // asks for anything. The IAM decision above still applies to it.
        ["_plugins", "_security", "authinfo"] => match request.method {
            "GET" => ok(&auth_info(domain, request)),
            _ => wrong_method(request, "GET"),
        },
        ["_plugins", "_security", "api", "roles", name] => security_role(domain, request, name),
        ["_plugins", "_security", "api", "rolesmapping", name] => {
            security_role_mapping(domain, request, name)
        }
        // A refresh with no index named is the whole cluster's, and this arm comes before
        // the single-segment one below so the reserved name is never read as an index.
        ["_refresh"] => match request.method {
            "GET" | "POST" => guarded(domain, request, fgac::ACTION_REFRESH, |domain| {
                refresh(domain, request, None)
            }),
            _ => wrong_method(request, "GET, POST"),
        },
        [index] => index_request(domain, request, index),
        // `_doc` upserts; `_create` must NOT overwrite, and neither must a `_doc` write
        // whose `op_type` asks for a create.
        [index, "_doc"] => document_write(domain, request, index, None, Creates::Upsert, now),
        [index, "_doc", id] => document_request(domain, request, index, id, requested_op_type(request), now),
        [index, "_create", id] => {
            document_request(domain, request, index, id, Creates::Only, now)
        }
        [index, "_count"] => match request.method {
            "GET" | "POST" => guarded(domain, request, fgac::ACTION_COUNT, |domain| {
                count(domain, request, index)
            }),
            _ => wrong_method(request, "GET, POST"),
        },
        [index, "_search"] => match request.method {
            "GET" | "POST" => guarded(domain, request, fgac::ACTION_SEARCH, |domain| {
                search(domain, request, index)
            }),
            _ => wrong_method(request, "GET, POST"),
        },
        [index, "_refresh"] => match request.method {
            "GET" | "POST" => guarded(domain, request, fgac::ACTION_REFRESH, |domain| {
                refresh(domain, request, Some(index))
            }),
            _ => wrong_method(request, "GET, POST"),
        },
        _ => no_handler(request),
    }
}

/// Whether a write may REPLACE a document the index already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Creates {
    /// `_doc` without an `op_type`: a write to an id the index holds replaces it.
    Upsert,
    /// `_create`, and `_doc` with `op_type=create`: a write to an id the index already
    /// holds is REFUSED rather than replacing the document.
    Only,
}

/// What `op_type` asks of a `_doc` write: `create` is the one value that forbids a
/// replacement, and every other (including none) leaves the write an upsert.
fn requested_op_type(request: &Request<'_>) -> Creates {
    match request.parameter("op_type") {
        Some("create") => Creates::Only,
        _ => Creates::Upsert,
    }
}

/// Take the cluster's own authorization decision for `action`, then `answer` the request.
///
/// Every route goes through this, so no route can serve a domain with fine-grained access
/// control switched on without the decision [`fgac`] owns having been taken for it.
fn guarded(
    domain: &mut Domain,
    request: &Request<'_>,
    action: &str,
    answer: impl FnOnce(&mut Domain) -> RtResponse,
) -> RtResponse {
    if let Some(reason) = fgac::refusal(domain, request.principal, action) {
        return error(403, ingest::SECURITY_EXCEPTION, &reason);
    }
    answer(domain)
}

/// `GET /_plugins/_security/authinfo` — who the caller is, as the security plugin knows
/// it.
///
/// The one route of the plugin that is answered for the authenticated user whatever
/// permissions it holds, which is what a client checks its own identity with: it names
/// the user, the BACKEND ROLES that user arrived with — for an AWS principal, the IAM
/// identities its credential speaks for, which is what a role mapping names it by
/// ([`fgac::backend_roles`]) — and the security roles this domain's mappings bind it to.
///
/// Every value is read from the request's principal and the domain's own security
/// configuration.
fn auth_info(domain: &Domain, request: &Request<'_>) -> Value {
    let mut roles = fgac::mapped_roles(domain, request.principal);
    // The master user holds the all-permitting role: that is the mapping the control
    // plane itself makes when `MasterUserOptions` names a principal.
    if fgac::is_master(domain, request.principal) && !roles.iter().any(|role| role == fgac::ALL_ACCESS_ROLE) {
        roles.insert(0, fgac::ALL_ACCESS_ROLE.to_string());
    }
    // "user_requested_tenant" is the tenant this request asked for, and it asked for
    // none; the tenant a user always has is its own.
    let mut tenants = ::serde_json::Map::new();
    tenants.insert(request.principal.to_string(), Value::Bool(true));
    json!({
        "user": fgac::user_description(request.principal),
        "user_name": request.principal,
        "user_requested_tenant": Value::Null,
        "backend_roles": fgac::backend_roles(request.principal),
        "custom_attribute_names": Vec::<String>::new(),
        "roles": roles,
        "tenants": Value::Object(tenants),
        "principal": Value::Null,
    })
}

/// The cluster banner a `GET /` answers: what this cluster IS, from the domain's own
/// record.
fn banner(domain: &Domain) -> Value {
    let (distribution, number) = engine_distribution(&domain.engine_version);
    json!({
        "name": domain.domain_name,
        "cluster_name": cluster_name(domain),
        "cluster_uuid": stable_uuid(&domain.arn),
        "version": { "distribution": distribution, "number": number },
        "tagline": TAGLINE,
    })
}

/// The tagline every OpenSearch cluster answers a `GET /` with.
const TAGLINE: &str = "The OpenSearch Project: https://opensearch.org/";

/// A domain's cluster name, which Amazon OpenSearch Service forms from the account that
/// owns it and the domain's own name.
fn cluster_name(domain: &Domain) -> String {
    format!("{}:{}", ::engine::emulator_account_id(), domain.domain_name)
}

/// The distribution and version number the domain's `EngineVersion` names: the value is
/// `<EngineType>_<major>.<minor>` (`OpenSearch_2.11`), so the engine type names the
/// distribution and the rest is the version, with the patch level it omits reported as 0.
fn engine_distribution(engine_version: &str) -> (String, String) {
    let (engine_type, version) = engine_version
        .split_once('_')
        .unwrap_or((state::ENGINE_TYPE_OPENSEARCH, engine_version));
    let number = if version.split('.').count() < 3 {
        format!("{version}.0")
    } else {
        version.to_string()
    };
    (engine_type.to_ascii_lowercase(), number)
}

/// A UUID that is a function of what it names, so the same resource reports the same id on
/// every read and across restarts without anything having to be remembered.
fn stable_uuid(seed: &str) -> String {
    ::uuid::Uuid::new_v5(&::uuid::Uuid::NAMESPACE_OID, seed.as_bytes()).to_string()
}

/// The cluster health a `GET /_cluster/health` answers, computed from the domain's own
/// cluster configuration and the indexes the request is about: every index the domain
/// holds, or the ONE the path names.
///
/// A shard copy needs a node of its own, so a replica of an index on a single-node cluster
/// is unassigned and the cluster is `yellow` — which is the health such a domain really
/// reports. With every copy assigned it is `green`.
///
/// `wait_for_status` is the one wait this emulator can answer without waiting: the cluster
/// is already settled, so the status it has is the status it will have — a request asking
/// for a healthier one than it reached is answered `timed_out: true` with the status it
/// really is, which is what the engine answers when the wait expires.
fn cluster_health(
    domain: &Domain,
    request: &Request<'_>,
    index_name: Option<&str>,
) -> RtResponse {
    let named: Vec<&state::Index> = match index_name {
        None => domain.indexes.values().collect(),
        Some(name) => match domain.indexes.get(name) {
            Some(index) => vec![index],
            None => return index_not_found(name),
        },
    };
    let data_nodes = domain.cluster_config.instance_count.unwrap_or(1).max(1);
    let master_nodes = if domain.cluster_config.dedicated_master_enabled == Some(true) {
        domain.cluster_config.dedicated_master_count.unwrap_or(0)
    } else {
        0
    };
    let mut primaries = 0_i64;
    let mut replicas = 0_i64;
    for index in named {
        // Both come from the index's own settings, which a caller wrote, so both are
        // clamped before they are multiplied out.
        let shards = setting_number(&index.schema, "number_of_shards")
            .unwrap_or(1)
            .clamp(1, MAX_SHARDS_PER_INDEX);
        let copies = setting_number(&index.schema, "number_of_replicas")
            .unwrap_or(1)
            .clamp(0, MAX_SHARDS_PER_INDEX);
        primaries = primaries.saturating_add(shards);
        replicas = replicas.saturating_add(shards.saturating_mul(copies));
    }
    // Each replica needs a node other than the one holding its primary.
    let assignable_replicas = if data_nodes > 1 { replicas } else { 0 };
    let unassigned = replicas.saturating_sub(assignable_replicas);
    let active = primaries.saturating_add(assignable_replicas);
    let total = primaries.saturating_add(replicas);
    let status = if unassigned > 0 { "yellow" } else { "green" };
    let timed_out = request
        .parameter("wait_for_status")
        .and_then(health_rank)
        .is_some_and(|wanted| health_rank(status).is_some_and(|reached| reached > wanted));
    let percent = if total == 0 {
        100.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            (active as f64 / total as f64) * 100.0
        }
    };
    ok(&json!({
        "cluster_name": cluster_name(domain),
        "status": status,
        "timed_out": timed_out,
        "number_of_nodes": i64::from(data_nodes) + i64::from(master_nodes),
        "number_of_data_nodes": i64::from(data_nodes),
        "discovered_master": true,
        "active_primary_shards": primaries,
        "active_shards": active,
        "relocating_shards": 0,
        "initializing_shards": 0,
        "unassigned_shards": unassigned,
        "delayed_unassigned_shards": 0,
        "number_of_pending_tasks": 0,
        "number_of_in_flight_fetch": 0,
        "task_max_waiting_in_queue_millis": 0,
        "active_shards_percent_as_number": percent,
    }))
}

/// How the three cluster health statuses order, worst last — what `wait_for_status`
/// compares against. `None` for a value that names none of them.
fn health_rank(status: &str) -> Option<u8> {
    match status {
        "green" => Some(0),
        "yellow" => Some(1),
        "red" => Some(2),
        _ => None,
    }
}

/// The number an index's `settings` section gives `name`, under the `index` sub-section or
/// directly, as both spellings a client writes it with.
fn setting_number(schema: &::aws_smithy_types::Document, name: &str) -> Option<i64> {
    let settings = state::json_from_document(schema);
    let section = settings.get("settings")?;
    let value = section
        .get("index")
        .and_then(|nested| nested.get(name))
        .or_else(|| section.get(name))?;
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

/// A `PUT`, `GET`, `HEAD` or `DELETE` on an index itself.
fn index_request(domain: &mut Domain, request: &Request<'_>, index_name: &str) -> RtResponse {
    match request.method {
        "PUT" => guarded(domain, request, fgac::ACTION_CREATE_INDEX, |domain| {
            create_index(domain, request, index_name)
        }),
        "GET" | "HEAD" => guarded(domain, request, fgac::ACTION_GET_INDEX, |domain| {
            read_index(domain, request, index_name)
        }),
        "DELETE" => guarded(domain, request, fgac::ACTION_DELETE_INDEX, |domain| {
            match domain.indexes.remove(index_name) {
                Some(_) => ok(&json!({ "acknowledged": true })),
                None => index_not_found(index_name),
            }
        }),
        _ => wrong_method(request, "PUT, GET, HEAD, DELETE"),
    }
}

/// Create the index the request names, carrying the schema its body gives and the one a
/// matching index template gives beneath it.
fn create_index(domain: &mut Domain, request: &Request<'_>, index_name: &str) -> RtResponse {
    if !state::is_index_name(index_name) {
        return indexed_error(
            400,
            INVALID_INDEX_NAME_EXCEPTION,
            &format!("Invalid index name [{index_name}]"),
            index_name,
        );
    }
    if domain.indexes.contains_key(index_name) {
        return indexed_error(
            400,
            RESOURCE_ALREADY_EXISTS_EXCEPTION,
            &format!(
                "index [{index_name}/{}] already exists",
                stable_uuid(&format!("{}/{index_name}", domain.arn))
            ),
            index_name,
        );
    }
    if domain.indexes.len() >= state::INDEX_QUOTA {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "domain [{}] already holds the maximum of {} indexes, so index \
                 [{index_name}] could not be created",
                domain.domain_name,
                state::INDEX_QUOTA
            ),
        );
    }
    let requested = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(None) => json!({}),
        Ok(_) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object",
            )
        }
        Err(refusal) => return *refusal,
    };
    // "An index template is a way to tell OpenSearch how to configure an index when it is
    // created": a matching template's schema is what the index starts from, and what the
    // create itself names is merged onto it, so an explicit section wins over the
    // template's.
    let templated = domain.templated_schema(index_name).cloned();
    let schema = match templated {
        Some(base) => state::merged_index_schema(&base, &state::document_from_json(&requested)),
        None => state::document_from_json(&requested),
    };
    let depth = state::index_schema_depth(&schema);
    if depth > state::INDEX_SCHEMA_MAX_DEPTH {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("index schema nests {depth} levels, past the maximum this cluster takes"),
        );
    }
    let values = state::index_schema_values(&schema);
    if values > state::INDEX_SCHEMA_QUOTA {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("index schema holds {values} values, past the maximum this cluster takes"),
        );
    }
    domain.indexes.insert(
        index_name.to_string(),
        state::Index {
            index_name: index_name.to_string(),
            schema,
            documents: BTreeMap::new(),
        },
    );
    ok(&json!({
        "acknowledged": true,
        "shards_acknowledged": true,
        "index": index_name,
    }))
}

/// An index's own settings and mappings, as a `GET /{index}` reports them.
fn read_index(domain: &Domain, request: &Request<'_>, index_name: &str) -> RtResponse {
    let Some(index) = domain.indexes.get(index_name) else {
        return match request.method {
            "HEAD" => status_only(404),
            _ => index_not_found(index_name),
        };
    };
    if request.method == "HEAD" {
        return status_only(200);
    }
    let schema = state::json_from_document(&index.schema);
    let mappings = schema.get("mappings").cloned().unwrap_or_else(|| json!({}));
    // A client writes the settings either directly or nested under `index`; one level of
    // that nesting is unwrapped so the report carries exactly one.
    let written = schema.get("settings").cloned().unwrap_or_else(|| json!({}));
    let mut settings = written
        .get("index")
        .cloned()
        .unwrap_or(written)
        .as_object()
        .cloned()
        .unwrap_or_default();
    settings.insert("provided_name".to_string(), json!(index_name));
    settings.insert(
        "uuid".to_string(),
        json!(stable_uuid(&format!("{}/{index_name}", domain.arn))),
    );
    ok(&json!({
        index_name: {
            "aliases": {},
            "mappings": mappings,
            "settings": { "index": Value::Object(settings) },
        }
    }))
}

/// A `GET`, `PUT`, `POST` or `DELETE` on one document addressed by its id.
fn document_request(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: &str,
    id: &str,
    creates: Creates,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "PUT" | "POST" => document_write(domain, request, index_name, Some(id), creates, now),
        "GET" => guarded(domain, request, fgac::ACTION_GET_DOCUMENT, |domain| {
            read_document(domain, index_name, id)
        }),
        "DELETE" => guarded(domain, request, fgac::ACTION_DELETE_DOCUMENT, |domain| {
            let copies = write_shards(domain, index_name);
            let Some(index) = domain.indexes.get_mut(index_name) else {
                return index_not_found(index_name);
            };
            let removed = index.documents.remove(id).is_some();
            let result = if removed { "deleted" } else { "not_found" };
            let status = if removed { 200 } else { 404 };
            rendered(
                status,
                &json!({
                    "_index": index_name,
                    "_id": id,
                    "result": result,
                    "_shards": copies,
                }),
            )
        }),
        _ => wrong_method(request, "GET, PUT, POST, DELETE"),
    }
}

/// One document, as a `GET /{index}/_doc/{id}` reports it.
fn read_document(domain: &Domain, index_name: &str, id: &str) -> RtResponse {
    let Some(index) = domain.indexes.get(index_name) else {
        return index_not_found(index_name);
    };
    match index.documents.get(id) {
        Some(document) => ok(&json!({
            "_index": index_name,
            "_id": id,
            "_version": document.version,
            "found": true,
            "_source": document.source,
        })),
        None => rendered(
            404,
            &json!({ "_index": index_name, "_id": id, "found": false }),
        ),
    }
}

/// Write one document into the index the request names, through the ONE data-plane write
/// every producer into a domain of this service goes through.
fn document_write(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: &str,
    id: Option<&str>,
    creates: Creates,
    now: SystemTime,
) -> RtResponse {
    if !matches!(request.method, "POST" | "PUT") {
        return wrong_method(request, "POST, PUT");
    }
    // A write that may not replace is refused when the index already holds the id, with
    // the version the document is at — what makes `_create` (and `op_type=create`) a
    // create rather than an upsert.
    if creates == Creates::Only {
        if let Some(version) = id
            .filter(|id| !id.is_empty())
            .and_then(|id| domain.indexes.get(index_name)?.documents.get(id))
            .map(|document| document.version)
        {
            let id = id.unwrap_or_default();
            return error(
                409,
                VERSION_CONFLICT_ENGINE_EXCEPTION,
                &format!(
                    "[{id}]: version conflict, document already exists (current version \
                     [{version}])"
                ),
            );
        }
    }
    let address = domain.arn.clone();
    let write = ingest::DocumentWrite {
        domain: &address,
        index_name,
        document_id: id.filter(|id| !id.is_empty()),
        document: request.body,
        principal_arn: request.principal,
        now,
    };
    // The write goes through the ONE data-plane write path, which owns the authorization
    // decision, the auto-created index and the mapping rules — so a document written over
    // the wire is judged exactly as a producer's delivery is.
    match ingest::index_into(domain, &write, request.identity) {
        Ok(indexed) => {
            let version = domain
                .indexes
                .get(&indexed.index_name)
                .and_then(|index| index.documents.get(&indexed.document_id))
                .map_or(1, |document| document.version);
            let created = version == 1;
            rendered(
                if created { 201 } else { 200 },
                &json!({
                    "_index": indexed.index_name,
                    "_id": indexed.document_id,
                    "_version": version,
                    "result": if created { "created" } else { "updated" },
                    "_shards": write_shards(domain, &indexed.index_name),
                }),
            )
        }
        Err(refusal) => refused(&refusal),
    }
}

/// The `_shards` summary a write answers with, computed from the index's own replica
/// setting and the domain's node count: every copy the cluster asked for is `total`, and
/// the ones a node can hold — the primary, plus the replicas only a second node can carry
/// — are `successful`.
fn write_shards(domain: &Domain, index_name: &str) -> Value {
    let copies = domain
        .indexes
        .get(index_name)
        .and_then(|index| setting_number(&index.schema, "number_of_replicas"))
        .unwrap_or(1)
        .clamp(0, MAX_SHARDS_PER_INDEX);
    let data_nodes = domain.cluster_config.instance_count.unwrap_or(1).max(1);
    let assigned = if data_nodes > 1 { copies } else { 0 };
    json!({
        "total": copies.saturating_add(1),
        "successful": assigned.saturating_add(1),
        "failed": 0,
    })
}

/// What a `_refresh` answers: the shards the refresh ran on, over the indexes it was asked
/// about — every index the domain holds, or the ONE the path names.
///
/// A refresh "makes recent operations performed on one or more indices available for
/// search": it flushes what is already written, so it changes nothing about a document and
/// reports no document. Every write this emulator takes is visible to the next read the
/// moment it lands, so what is left to answer is the shard summary, computed from each
/// index's own settings and the nodes the domain has — the same arithmetic the cluster
/// health and a write's `_shards` are computed with.
fn refresh(domain: &Domain, request: &Request<'_>, index_name: Option<&str>) -> RtResponse {
    let named: Vec<&state::Index> = match index_name {
        None => domain.indexes.values().collect(),
        Some(name) => match domain.indexes.get(name) {
            Some(index) => vec![index],
            None => return index_not_found(name),
        },
    };
    let _ = request;
    let data_nodes = domain.cluster_config.instance_count.unwrap_or(1).max(1);
    let mut total = 0_i64;
    let mut successful = 0_i64;
    for index in named {
        // Both come from the index's own settings, which a caller wrote, so both are
        // clamped before they are multiplied out.
        let shards = setting_number(&index.schema, "number_of_shards")
            .unwrap_or(1)
            .clamp(1, MAX_SHARDS_PER_INDEX);
        let copies = setting_number(&index.schema, "number_of_replicas")
            .unwrap_or(1)
            .clamp(0, MAX_SHARDS_PER_INDEX);
        // A replica copy needs a node other than the one holding its primary, so on a
        // single-node cluster only the primaries are refreshed.
        let assigned = if data_nodes > 1 { copies } else { 0 };
        total = total.saturating_add(shards.saturating_mul(copies.saturating_add(1)));
        successful = successful.saturating_add(shards.saturating_mul(assigned.saturating_add(1)));
    }
    ok(&json!({
        "_shards": { "total": total, "successful": successful, "failed": 0 },
    }))
}

/// The count a `_count` answers, over the documents the selected indexes hold.
fn count(domain: &Domain, request: &Request<'_>, index_name: &str) -> RtResponse {
    // No hit is reported by a count, so none is materialised for it.
    let (total, _) = match selected_documents(domain, request, index_name, 0, 0) {
        Ok(selected) => selected,
        Err(refusal) => return *refusal,
    };
    ok(&json!({
        "count": total,
        "_shards": { "total": 1, "successful": 1, "skipped": 0, "failed": 0 },
    }))
}

/// The hits a `_search` answers, over the documents the selected indexes hold.
fn search(domain: &Domain, request: &Request<'_>, index_name: &str) -> RtResponse {
    let body = match request.json() {
        Ok(body) => body.unwrap_or_else(|| json!({})),
        Err(refusal) => return *refusal,
    };
    let from = usize::try_from(numbered(&body, "from", request, 0)).unwrap_or(0);
    let size = usize::try_from(numbered(
        &body,
        "size",
        request,
        i64::try_from(DEFAULT_SEARCH_SIZE).unwrap_or(10),
    ))
    .unwrap_or(DEFAULT_SEARCH_SIZE);
    // The engine's own bound on how much one search may answer with, which is also what
    // bounds the hits this request materialises: past it the search is refused rather than
    // served.
    if from.saturating_add(size) > MAX_RESULT_WINDOW {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "Result window is too large, from + size must be less than or equal to: \
                 [{MAX_RESULT_WINDOW}] but was [{}].",
                from.saturating_add(size)
            ),
        );
    }
    let (total, hits) = match selected_documents(domain, request, index_name, from, size) {
        Ok(selected) => selected,
        Err(refusal) => return *refusal,
    };
    let hits: Vec<Value> = hits
        .into_iter()
        .map(|(index, id, source)| {
            json!({
                "_index": index,
                "_id": id,
                "_score": 1.0,
                "_source": source,
            })
        })
        .collect();
    ok(&json!({
        "timed_out": false,
        "_shards": { "total": 1, "successful": 1, "skipped": 0, "failed": 0 },
        "hits": {
            "total": { "value": total, "relation": "eq" },
            "max_score": if total == 0 { Value::Null } else { json!(1.0) },
            "hits": hits,
        },
    }))
}

/// The number a search names for `name`, in the body or in the query string, falling back
/// to `default`.
fn numbered(body: &Value, name: &str, request: &Request<'_>, default: i64) -> i64 {
    body.get(name)
        .and_then(Value::as_i64)
        .or_else(|| request.parameter(name).and_then(|text| text.parse().ok()))
        .unwrap_or(default)
        .max(0)
}

/// How many documents the request selects, and the ones on the page it asked for — every
/// document of the indexes its path names that the query it carries matches, in index then
/// id order.
///
/// Three bounds, so one request's work is bounded whatever its path says: the number of
/// comma-separated patterns it may name ([`MAX_NAMED_INDEX_PATTERNS`]); the resolved index
/// set is DE-DUPLICATED, so repeating a pattern cannot multiply the scan or double-count a
/// document; and the documents visited are capped at the whole store once
/// ([`MAX_DOCUMENTS_VISITED`]). Only the page is MATERIALISED, so what one request
/// allocates is bounded by the `size` it asked for and not by the documents the domain
/// holds.
fn selected_documents(
    domain: &Domain,
    request: &Request<'_>,
    index_name: &str,
    from: usize,
    size: usize,
) -> Result<Selected, Box<RtResponse>> {
    let body = request.json()?.unwrap_or_else(|| json!({}));
    let select = selector(body.get("query"), 0)?;
    let patterns: Vec<&str> = index_name.split(',').filter(|name| !name.is_empty()).collect();
    if patterns.len() > MAX_NAMED_INDEX_PATTERNS {
        return Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "a request may name at most {MAX_NAMED_INDEX_PATTERNS} indexes, and this one \
                 names {}",
                patterns.len()
            ),
        )));
    }
    // A `BTreeSet` of names, so an index named twice is scanned once and the order is the
    // store's own: two reads of one domain report the same documents in the same order.
    let mut named = ::std::collections::BTreeSet::new();
    for pattern in patterns {
        if pattern == "_all" || pattern.contains('*') || pattern.contains('?') {
            named.extend(
                domain
                    .indexes
                    .keys()
                    .filter(|name| {
                        ::service_common::iam_authorization::wildcard_match(
                            if pattern == "_all" { "*" } else { pattern },
                            name,
                            true,
                        )
                    })
                    .map(String::as_str),
            );
            continue;
        }
        if !domain.indexes.contains_key(pattern) {
            return Err(Box::new(index_not_found(pattern)));
        }
        named.insert(pattern);
    }
    let mut visited = 0_usize;
    let mut total = 0_usize;
    let mut page = Vec::new();
    for index in named.into_iter().filter_map(|name| domain.indexes.get(name)) {
        for document in index.documents.values() {
            visited = visited.saturating_add(1);
            if visited > MAX_DOCUMENTS_VISITED {
                return Err(Box::new(error(
                    400,
                    ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                    &format!(
                        "a request may visit at most {MAX_DOCUMENTS_VISITED} documents"
                    ),
                )));
            }
            if !select.matches(&document.id, &document.source) {
                continue;
            }
            if total >= from && page.len() < size {
                page.push((
                    index.index_name.clone(),
                    document.id.clone(),
                    document.source.clone(),
                ));
            }
            total = total.saturating_add(1);
        }
    }
    Ok((total, page))
}

/// Which documents a query selects.
///
/// The vocabulary is bounded and stated: `match_all`, `term`, `terms`, `match`, `ids` and
/// a `bool` composing them. A clause outside it is answered the way the engine answers a
/// query name it has no handler registered for, rather than silently selecting everything
/// — a search whose clause this emulator cannot evaluate must not look like one it did.
#[derive(Debug)]
enum Selector {
    /// Every document.
    All,
    /// A field's value equals one of these, compared as text so a number and its decimal
    /// spelling are the same value — which is how a `keyword` term and a `match` on a
    /// numeric field both behave.
    Equals { field: String, values: Vec<String> },
    /// The document's id is one of these.
    Ids(Vec<String>),
    /// Every one of these selects the document.
    All_(Vec<Selector>),
    /// None of these may select the document.
    None_(Vec<Selector>),
    /// Any one of these selects the document.
    Any(Vec<Selector>),
}

impl Selector {
    /// Whether this selector selects the document `id`/`source`.
    fn matches(&self, id: &str, source: &Value) -> bool {
        match self {
            Self::All => true,
            Self::Equals { field, values } => field_values(source, field)
                .iter()
                .any(|held| values.iter().any(|wanted| wanted == held)),
            Self::Ids(ids) => ids.iter().any(|wanted| wanted == id),
            Self::All_(clauses) => clauses.iter().all(|clause| clause.matches(id, source)),
            Self::None_(clauses) => !clauses.iter().any(|clause| clause.matches(id, source)),
            Self::Any(clauses) => clauses.iter().any(|clause| clause.matches(id, source)),
        }
    }
}

/// The texts a document holds at `field`, which may be a dotted path into nested objects
/// and may hold an array of values.
fn field_values(source: &Value, field: &str) -> Vec<String> {
    let mut node = source;
    for segment in field.split('.') {
        match node.get(segment) {
            Some(next) => node = next,
            None => return Vec::new(),
        }
    }
    match node {
        Value::Array(elements) => elements.iter().map(scalar_text).collect(),
        other => vec![scalar_text(other)],
    }
}

/// A scalar's text, which is what a term is compared against.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The selector a query clause states, `depth` levels down a `bool` composition.
fn selector(query: Option<&Value>, depth: usize) -> Result<Selector, Box<RtResponse>> {
    // The engine's own bound on how far a `bool` composition may nest
    // (`indices.query.bool.max_nested_depth`), which is what keeps the walk below from
    // recursing on a depth the caller chose.
    if depth > MAX_BOOL_NESTING {
        return Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "The nested depth of the query exceeds the maximum nested depth for bool \
                 queries [{MAX_BOOL_NESTING}]"
            ),
        )));
    }
    let Some(query) = query else {
        return Ok(Selector::All);
    };
    let Value::Object(clause) = query else {
        return Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            "failed to parse: expected a query to be a JSON object",
        )));
    };
    let Some((name, value)) = clause.iter().next() else {
        return Ok(Selector::All);
    };
    match name.as_str() {
        "match_all" => Ok(Selector::All),
        "term" | "match" | "match_phrase" => {
            let (field, wanted) = single_field(value)?;
            let wanted = wanted
                .get("value")
                .or_else(|| wanted.get("query"))
                .unwrap_or(&wanted)
                .clone();
            Ok(Selector::Equals {
                field,
                values: vec![scalar_text(&wanted)],
            })
        }
        "terms" => {
            let (field, wanted) = single_field(value)?;
            let values = match wanted {
                Value::Array(elements) => elements.iter().map(scalar_text).collect(),
                other => vec![scalar_text(&other)],
            };
            Ok(Selector::Equals { field, values })
        }
        "ids" => Ok(Selector::Ids(
            value
                .get("values")
                .and_then(Value::as_array)
                .map(|values| values.iter().map(scalar_text).collect())
                .unwrap_or_default(),
        )),
        "bool" => {
            let clauses = |name: &str| -> Result<Vec<Selector>, Box<RtResponse>> {
                let listed = match value.get(name) {
                    None => Vec::new(),
                    Some(Value::Array(elements)) => elements.clone(),
                    Some(one) => vec![one.clone()],
                };
                listed
                    .iter()
                    .map(|one| selector(Some(one), depth.saturating_add(1)))
                    .collect()
            };
            let mut required = clauses("must")?;
            required.extend(clauses("filter")?);
            let forbidden = clauses("must_not")?;
            let optional = clauses("should")?;
            let mut composed = vec![Selector::All_(required), Selector::None_(forbidden)];
            if !optional.is_empty() {
                composed.push(Selector::Any(optional));
            }
            Ok(Selector::All_(composed))
        }
        other => Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            &format!("no [query] registered for [{other}]"),
        ))),
    }
}

/// The one field a `term`/`terms`/`match` clause names, and the value it wants.
fn single_field(value: &Value) -> Result<(String, Value), Box<RtResponse>> {
    match value {
        Value::Object(members) if members.len() == 1 => members
            .iter()
            .next()
            .map(|(field, wanted)| (field.clone(), wanted.clone()))
            .ok_or_else(|| {
                Box::new(error(
                    400,
                    PARSING_EXCEPTION,
                    "[term] query does not support an empty field list",
                ))
            }),
        _ => Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            "[term] query must name exactly one field",
        ))),
    }
}

/// A `PUT`, `GET` or `DELETE` on an index template, in the composable
/// (`_index_template`) or the legacy (`_template`) form.
fn template_request(
    domain: &mut Domain,
    request: &Request<'_>,
    name: &str,
    legacy: bool,
) -> RtResponse {
    match request.method {
        "PUT" | "POST" => guarded(domain, request, fgac::ACTION_PUT_TEMPLATE, |domain| {
            write_template(domain, request, name, legacy)
        }),
        "GET" => guarded(domain, request, fgac::ACTION_GET_TEMPLATE, |domain| {
            match domain.index_templates.get(name) {
                Some(template) if legacy => ok(&json!({ name: template.definition })),
                Some(template) => ok(&json!({
                    "index_templates": [
                        { "name": name, "index_template": template.definition }
                    ]
                })),
                None => error(
                    404,
                    INDEX_NOT_FOUND_EXCEPTION,
                    &format!("index_template [{name}] missing"),
                ),
            }
        }),
        "DELETE" => guarded(domain, request, fgac::ACTION_PUT_TEMPLATE, |domain| {
            match domain.index_templates.remove(name) {
                Some(_) => ok(&json!({ "acknowledged": true })),
                None => error(
                    404,
                    INDEX_NOT_FOUND_EXCEPTION,
                    &format!("index_template [{name}] missing"),
                ),
            }
        }),
        _ => wrong_method(request, "PUT, POST, GET, DELETE"),
    }
}

/// Write the template the request carries: the patterns it applies to, its priority, and
/// the schema an index it applies to is created with.
fn write_template(
    domain: &mut Domain,
    request: &Request<'_>,
    name: &str,
    legacy: bool,
) -> RtResponse {
    let definition = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(_) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object",
            )
        }
        Err(refusal) => return *refusal,
    };
    // "index_patterns is a required field": a template that applies to no index name is
    // refused rather than stored.
    let index_patterns: Vec<String> = definition
        .get("index_patterns")
        .and_then(Value::as_array)
        .map(|patterns| patterns.iter().filter_map(|p| p.as_str()).map(str::to_string).collect())
        .unwrap_or_default();
    if index_patterns.is_empty() {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            "index_patterns is missing or empty",
        );
    }
    if !domain.index_templates.contains_key(name) && domain.index_templates.len() >= TEMPLATE_QUOTA
    {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "domain [{}] already holds the maximum of {TEMPLATE_QUOTA} index templates",
                domain.domain_name
            ),
        );
    }
    // The composable form nests the index configuration under `template`; the legacy form
    // carries it at the top level. "Of two matching templates the one with the higher
    // priority is applied", and the legacy form spells that number `order`.
    let configuration = if legacy {
        definition.clone()
    } else {
        definition.get("template").cloned().unwrap_or_else(|| json!({}))
    };
    let mut schema = Map::new();
    for section in ["mappings", "settings"] {
        if let Some(value) = configuration.get(section) {
            schema.insert(section.to_string(), value.clone());
        }
    }
    let priority = definition
        .get(if legacy { "order" } else { "priority" })
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let schema = state::document_from_json(&Value::Object(schema));
    let values = state::index_schema_values(&schema);
    if values > state::INDEX_SCHEMA_QUOTA {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("index schema holds {values} values, past the maximum this cluster takes"),
        );
    }
    domain.index_templates.insert(
        name.to_string(),
        IndexTemplate {
            name: name.to_string(),
            index_patterns,
            priority,
            schema,
            definition,
        },
    );
    ok(&json!({ "acknowledged": true }))
}

/// A request on the security plugin's role API.
fn security_role(domain: &mut Domain, request: &Request<'_>, name: &str) -> RtResponse {
    if let Some(refusal) = security_api_guard(domain, request) {
        return refusal;
    }
    match request.method {
        "PUT" | "PATCH" => {
            let definition = match request.json() {
                Ok(Some(Value::Object(members))) => Value::Object(members),
                Ok(_) => {
                    return error(
                        400,
                        PARSE_EXCEPTION,
                        "failed to parse: expected the body to be a JSON object",
                    )
                }
                Err(refusal) => return *refusal,
            };
            if !domain.security_roles.contains_key(name)
                && domain.security_roles.len() >= SECURITY_RECORD_QUOTA
            {
                return over_security_quota(domain, "roles");
            }
            let existed = domain
                .security_roles
                .insert(
                    name.to_string(),
                    fgac::SecurityRole {
                        name: name.to_string(),
                        definition,
                    },
                )
                .is_some();
            written(name, existed)
        }
        "GET" => match domain.security_roles.get(name) {
            Some(role) => ok(&json!({ name: role.definition })),
            None => rendered(
                404,
                &json!({ "status": "NOT_FOUND", "message": format!("role {name} not found.") }),
            ),
        },
        "DELETE" => match domain.security_roles.remove(name) {
            Some(_) => ok(
                &json!({ "status": "OK", "message": format!("'{name}' deleted.") }),
            ),
            None => rendered(
                404,
                &json!({ "status": "NOT_FOUND", "message": format!("role {name} not found.") }),
            ),
        },
        _ => wrong_method(request, "PUT, PATCH, GET, DELETE"),
    }
}

/// A request on the security plugin's role-mapping API — the records that decide which
/// principals hold a role, and so what any principal but the master user may do.
fn security_role_mapping(domain: &mut Domain, request: &Request<'_>, name: &str) -> RtResponse {
    if let Some(refusal) = security_api_guard(domain, request) {
        return refusal;
    }
    match request.method {
        "PUT" | "PATCH" => {
            let definition = match request.json() {
                Ok(Some(Value::Object(members))) => Value::Object(members),
                Ok(_) => {
                    return error(
                        400,
                        PARSE_EXCEPTION,
                        "failed to parse: expected the body to be a JSON object",
                    )
                }
                Err(refusal) => return *refusal,
            };
            let listed = |member: &str| -> Vec<String> {
                definition
                    .get(member)
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            if !domain.security_role_mappings.contains_key(name)
                && domain.security_role_mappings.len() >= SECURITY_RECORD_QUOTA
            {
                return over_security_quota(domain, "rolesmapping");
            }
            let existed = domain
                .security_role_mappings
                .insert(
                    name.to_string(),
                    fgac::SecurityRoleMapping {
                        role: name.to_string(),
                        backend_roles: listed("backend_roles"),
                        users: listed("users"),
                        hosts: listed("hosts"),
                    },
                )
                .is_some();
            written(name, existed)
        }
        "GET" => match domain.security_role_mappings.get(name) {
            Some(mapping) => ok(&json!({
                name: {
                    "hosts": mapping.hosts,
                    "users": mapping.users,
                    "backend_roles": mapping.backend_roles,
                }
            })),
            None => rendered(
                404,
                &json!({
                    "status": "NOT_FOUND",
                    "message": format!("rolesmapping {name} not found."),
                }),
            ),
        },
        "DELETE" => match domain.security_role_mappings.remove(name) {
            Some(_) => ok(
                &json!({ "status": "OK", "message": format!("'{name}' deleted.") }),
            ),
            None => rendered(
                404,
                &json!({
                    "status": "NOT_FOUND",
                    "message": format!("rolesmapping {name} not found."),
                }),
            ),
        },
        _ => wrong_method(request, "PUT, PATCH, GET, DELETE"),
    }
}

/// What the security API answers a write past [`SECURITY_RECORD_QUOTA`]: the record is not
/// stored, so the configuration this emulator holds stays bounded.
fn over_security_quota(domain: &Domain, kind: &str) -> RtResponse {
    error(
        400,
        ingest::ILLEGAL_ARGUMENT_EXCEPTION,
        &format!(
            "domain [{}] already holds the maximum of {SECURITY_RECORD_QUOTA} {kind} records",
            domain.domain_name
        ),
    )
}

/// What the security API answers a request on a domain it is not reachable on: the plugin
/// is part of fine-grained access control, so a domain without it has no such handler at
/// all, and only the MASTER user may change the security configuration of a domain that
/// has it.
fn security_api_guard(domain: &Domain, request: &Request<'_>) -> Option<RtResponse> {
    if !fgac::enforced(domain) {
        return Some(no_handler(request));
    }
    fgac::security_api_refusal(domain, request.principal)
        .map(|reason| error(403, ingest::SECURITY_EXCEPTION, &reason))
}

/// What the security API answers a write: the record was created, or the one that was
/// there was replaced.
fn written(name: &str, existed: bool) -> RtResponse {
    if existed {
        return ok(&json!({ "status": "OK", "message": format!("'{name}' updated.") }));
    }
    rendered(
        201,
        &json!({ "status": "CREATED", "message": format!("'{name}' created.") }),
    )
}

/// The answer a write refused by the one data-plane write path renders: the engine's own
/// error type decides the status, since the refusal already carries which condition held.
fn refused(refusal: &ingest::WriteRefusal) -> RtResponse {
    match refusal {
        ingest::WriteRefusal::Unavailable { message } => {
            error(503, "cluster_block_exception", message)
        }
        ingest::WriteRefusal::Rejected {
            error_type,
            message,
        } => {
            let status = if *error_type == ingest::SECURITY_EXCEPTION {
                403
            } else {
                400
            };
            error(status, error_type, message)
        }
    }
}

/// The refusal the engine answers a path it has no handler registered for.
fn no_handler(request: &Request<'_>) -> RtResponse {
    rendered(
        400,
        &json!({
            "error": format!(
                "no handler found for uri [{}] and method [{}]",
                request.path, request.method
            )
        }),
    )
}

/// The refusal the engine answers a path it DOES serve, reached with a method that path
/// does not take: a different class of failure from a path it serves nothing for, and
/// worded as the engine words it, naming the methods that are allowed.
fn wrong_method(request: &Request<'_>, allowed: &str) -> RtResponse {
    rendered(
        405,
        &json!({
            "error": format!(
                "Incorrect HTTP method for uri [{}] and method [{}], allowed: [{allowed}]",
                request.path, request.method
            )
        }),
    )
}

/// The refusal for an index the domain does not hold.
fn index_not_found(index_name: &str) -> RtResponse {
    indexed_error(
        404,
        INDEX_NOT_FOUND_EXCEPTION,
        &format!("no such index [{index_name}]"),
        index_name,
    )
}

/// An error naming the index it is about, which is what the engine's index errors carry
/// beside the type and the reason.
fn indexed_error(status: u16, error_type: &str, reason: &str, index_name: &str) -> RtResponse {
    let cause = json!({
        "type": error_type,
        "reason": reason,
        "index": index_name,
        "resource.type": "index_or_alias",
        "resource.id": index_name,
    });
    rendered(
        status,
        &json!({
            "error": {
                "root_cause": [cause.clone()],
                "type": error_type,
                "reason": reason,
                "index": index_name,
                "resource.type": "index_or_alias",
                "resource.id": index_name,
            },
            "status": status,
        }),
    )
}

/// The engine's error envelope: the type and the reason, with the status repeated in the
/// body as every OpenSearch error carries it.
fn error(status: u16, error_type: &str, reason: &str) -> RtResponse {
    rendered(
        status,
        &json!({
            "error": {
                "root_cause": [{ "type": error_type, "reason": reason }],
                "type": error_type,
                "reason": reason,
            },
            "status": status,
        }),
    )
}

/// A 200 answer carrying `body`.
fn ok(body: &Value) -> RtResponse {
    rendered(200, body)
}

/// A status with no body at all — what a `HEAD` answers.
fn status_only(status: u16) -> RtResponse {
    RtResponse::new(
        StatusCode::try_from(status).unwrap_or_else(|_| fallback_status()),
        SdkBody::empty(),
    )
}

/// The status a status this emulator could not build falls back to.
fn fallback_status() -> StatusCode {
    StatusCode::try_from(500).expect("500 is a status")
}

/// One rendered JSON answer, as the wire carries it.
fn rendered(status: u16, body: &Value) -> RtResponse {
    let payload = ::serde_json::to_vec(body).unwrap_or_default();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or_else(|_| fallback_status()),
        SdkBody::from(payload),
    );
    if let Err(refused) = response
        .headers_mut()
        .try_insert("content-type", "application/json")
    {
        ::tracing::warn!(
            error = %refused,
            "the data plane's content type cannot be sent on the wire and is left off"
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the data plane is addressed either at the host a domain publishes or under
    /// the reserved path namespace naming this plane and the domain; nothing else names it.
    #[test]
    fn a_prefixed_path_names_this_plane_the_domain_and_the_request_path() {
        // (the case, the path, the domain it names, the request path within it)
        struct Row {
            case: &'static str,
            path: &'static str,
            names: Option<(&'static str, &'static str)>,
        }
        let rows: &[Row] = &[
            Row {
                case: "a domain name and a cluster path",
                path: "/_aws/es/search-target/_cluster/health",
                names: Some(("search-target", "/_cluster/health")),
            },
            Row {
                case: "a domain name and the root path",
                path: "/_aws/es/search-target",
                names: Some(("search-target", "/")),
            },
            Row {
                case: "another plane under the same namespace",
                path: "/_aws/execute-api/abc/stage",
                names: None,
            },
            Row {
                case: "no namespace at all",
                path: "/_cluster/health",
                names: None,
            },
            Row {
                case: "the namespace and this plane but no domain",
                path: "/_aws/es",
                names: None,
            },
        ];
        for row in rows {
            let named = prefixed_addressing(row.path);
            assert_eq!(
                named, row.names,
                "case {:?}: expected {:?}, got {named:?}",
                row.case, row.names
            );
        }
    }

    /// RULE: a query selects documents by the clause it states, and a clause the engine
    /// registers no query for is refused rather than selecting everything.
    #[test]
    fn a_query_selects_by_the_clause_it_states() {
        let source = json!({ "level": "warn", "code": 7, "tags": ["a", "b"] });
        // (the case, the query, whether it selects the document)
        let rows: &[(&str, Value, bool)] = &[
            ("no query at all", json!({ "match_all": {} }), true),
            ("a term on a text field", json!({ "term": { "level": "warn" } }), true),
            ("a term on another value", json!({ "term": { "level": "info" } }), false),
            ("a term on a numeric field", json!({ "term": { "code": 7 } }), true),
            ("a term with an explicit value", json!({ "term": { "code": { "value": 7 } } }), true),
            ("a match clause", json!({ "match": { "level": "warn" } }), true),
            ("a terms clause", json!({ "terms": { "level": ["info", "warn"] } }), true),
            ("a term on one element of an array", json!({ "term": { "tags": "b" } }), true),
            ("an ids clause naming the document", json!({ "ids": { "values": ["one"] } }), true),
            ("an ids clause naming another", json!({ "ids": { "values": ["two"] } }), false),
            (
                "a bool composing a filter and a must_not",
                json!({ "bool": {
                    "filter": [{ "term": { "level": "warn" } }],
                    "must_not": [{ "term": { "code": 8 } }]
                } }),
                true,
            ),
            (
                "a bool whose must_not excludes it",
                json!({ "bool": { "must_not": [{ "term": { "level": "warn" } }] } }),
                false,
            ),
        ];
        for (case, query, expected) in rows {
            let select = selector(Some(query), 0)
                .map_err(|_| ())
                .unwrap_or_else(|()| panic!("case {case:?}: expected the query to be read"));
            let matched = select.matches("one", &source);
            assert_eq!(
                matched, *expected,
                "case {case:?}: expected {expected}, got {matched}"
            );
        }
        let refused = selector(Some(&json!({ "wildcard": { "level": "w*" } })), 0);
        assert!(
            refused.is_err(),
            "case \"a clause the engine registers no query for\": expected a refusal"
        );
        // And a `bool` nested past the engine's own bound is refused rather than walked.
        let mut nested = json!({ "match_all": {} });
        for _ in 0..=MAX_BOOL_NESTING {
            nested = json!({ "bool": { "must": [nested] } });
        }
        assert!(
            selector(Some(&nested), 0).is_err(),
            "case \"a bool nested past the engine's bound\": expected a refusal"
        );
    }

    /// RULE: the version banner reports the distribution and the version number the
    /// domain's own `EngineVersion` names.
    #[test]
    fn the_banner_version_comes_from_the_domains_engine_version() {
        // (the case, the stored EngineVersion, the distribution, the number)
        let rows: &[(&str, &str, &str, &str)] = &[
            ("an OpenSearch version", "OpenSearch_2.11", "opensearch", "2.11.0"),
            ("an Elasticsearch version", "Elasticsearch_7.10", "elasticsearch", "7.10.0"),
            ("a three-part version", "OpenSearch_2.11.1", "opensearch", "2.11.1"),
        ];
        for (case, stored, distribution, number) in rows {
            let answered = engine_distribution(stored);
            assert_eq!(
                answered,
                ((*distribution).to_string(), (*number).to_string()),
                "case {case:?}: expected {distribution}/{number}, got {answered:?}"
            );
        }
    }
}

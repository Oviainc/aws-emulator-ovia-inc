// SPDX-License-Identifier: CC0-1.0

//! The workspace's Prometheus data plane: the API `WorkspaceDescription$prometheusEndpoint`
//! announces.
//!
//! # Not a modelled operation
//!
//! `prometheusEndpoint` is documented as "The Prometheus endpoint available for this
//! workspace. For example, `https://aps-workspaces.<region>.amazonaws.com/workspaces/ws-.../api/v1/`"
//! — the URL at which THIS workspace's Prometheus HTTP API is served. That API is
//! Prometheus's own, not this service's Smithy surface: a remote_write agent `POST`s a
//! snappy-compressed protobuf `WriteRequest` to `remote_write` beneath that URL, and a
//! Grafana datasource or any PromQL caller reads the series back from `query`. Neither
//! request carries an `X-Amz-Target` or matches a modelled `@http` route, so nothing in
//! the generated routing table can name them, and they are reached through the seam an
//! emulated data plane uses:
//!
//! * [`crate::service::EmulatedService::override_operation`] resolves a request addressed
//!   beneath a workspace's announced endpoint to [`PrometheusDataPlane`], which is an
//!   author-written operation rather than a modelled one;
//! * that operation declares SigV4 ([`ErasedOperation::auth_schemes`]), so the router
//!   verifies the request's signature before it runs — the data plane is signed traffic,
//!   exactly as the control plane is.
//!
//! # What is served
//!
//! Beneath [`crate::state::workspace_prometheus_path`] — the path half of the announced
//! endpoint, so following the announced URL lands here:
//!
//! * `POST remote_write` takes a remote-write request and writes its samples to the
//!   workspace ([`crate::state::remote_write`], [`crate::state::IngestedMetrics::accept`]);
//! * `GET` or `POST query` evaluates a PromQL instant query against the samples the
//!   workspace holds ([`crate::state::promql`]) and answers in the Prometheus HTTP API's
//!   own JSON envelope.
//!
//! The version prefix (`api/v1`) is matched however many times the request repeated it:
//! the announced endpoint ENDS in that prefix, and a Prometheus client composes its target
//! by joining that endpoint with the API's own version-prefixed path — so both
//! compositions name the same endpoint of the same API and both are served.
//!
//! # The envelopes
//!
//! A refusal is answered in the envelope of the API the request addressed, never in this
//! service's restJson1 error shapes, which belong to the modelled operations: the
//! Prometheus HTTP API's JSON document (`status`, `errorType`, `error`) for `query`, which
//! that API defines, and a plain-text reason for `remote_write`, whose specification gives
//! the body of a non-2xx as a human-readable error. Every message is composed from the
//! constants here and from the state's own bounds, so none of them carries the request
//! back to the caller.
//!
//! # What bounds a request, and what never runs under the store's lock
//!
//! Both legs take caller-supplied documents, so each is bounded before the work
//! proportional to it runs, and neither runs that work while the store is held:
//!
//! * the write's body is bounded by [`crate::state::remote_write`]'s caps and is
//!   decompressed and decoded on the blocking pool BEFORE the lock is taken; what runs
//!   under the lock is the insertion of at most one request's worth of samples;
//! * the query's expression is bounded in SIZE, in nesting and in matcher count by
//!   [`crate::state::promql`], which owns every bound on it; the store is held only long
//!   enough to read a snapshot of one sample per series, and the caller's expression is
//!   then parsed, planned and applied to that snapshot on the blocking pool under a step
//!   and wall-clock budget.
//!
//! Each leg therefore has an `.await` across the work it does, which is where a timeout or
//! a cancellation of the request lands.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use crate::state::promql::{QueryError, QueryValue};
use crate::state::{
    Workspace, PROMETHEUS_API_VERSION, STATUS_ACTIVE, STATUS_UPDATING, WORKSPACES_PATH_PREFIX,
};

/// The auth schemes this plane is served under: SigV4, the scheme every operation of this
/// service declares. Stated here because an author-written operation's schemes are what
/// the router authenticates a request against when the model names none for its route —
/// so the data plane is verified traffic rather than an unsigned side door.
const DATA_PLANE_AUTH_SCHEMES: &[&str] = &["aws.auth#sigv4"];

/// The endpoint of the Prometheus HTTP API a request addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiEndpoint {
    /// `remote_write`: the write path of the remote-write protocol.
    RemoteWrite,
    /// `query`: the instant-query path of the Prometheus HTTP API.
    Query,
}

impl ApiEndpoint {
    /// The endpoint the last path segment names, when it names one this plane serves.
    fn named(segment: &str) -> Option<Self> {
        match segment {
            "remote_write" => Some(Self::RemoteWrite),
            "query" => Some(Self::Query),
            _ => None,
        }
    }
}

/// One request to a workspace's Prometheus API: the workspace it is addressed to and the
/// endpoint it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiRequest {
    pub(crate) workspace_id: String,
    pub(crate) endpoint: ApiEndpoint,
}

/// The data-plane request `uri` addresses, or `None` when the URI is not beneath a
/// workspace's announced Prometheus endpoint — in which case the request is left to the
/// modelled routing table exactly as it was.
///
/// The shape is the announced one: the workspace's path space
/// ([`WORKSPACES_PATH_PREFIX`]), the API's version prefix
/// ([`PROMETHEUS_API_VERSION`], repeated as the client's URL composition repeated it) and
/// then the endpoint's own name.
pub(crate) fn api_request_of(uri: &str) -> Option<ApiRequest> {
    let path = uri.split('?').next().unwrap_or(uri);
    let (workspace_id, rest) = path
        .strip_prefix(WORKSPACES_PATH_PREFIX)?
        .split_once('/')?;
    if workspace_id.is_empty() || workspace_id.contains('/') {
        return None;
    }
    let mut rest = rest;
    let mut versioned = false;
    while let Some(tail) = rest
        .strip_prefix(PROMETHEUS_API_VERSION)
        .and_then(|tail| tail.strip_prefix('/'))
    {
        rest = tail;
        versioned = true;
    }
    if !versioned {
        return None;
    }
    let endpoint = ApiEndpoint::named(rest.trim_end_matches('/'))?;
    Some(ApiRequest {
        workspace_id: workspace_id.to_string(),
        endpoint,
    })
}

/// The Prometheus data plane as an operation the router can run: author-written, so it
/// reads the request and writes the response itself rather than through a modelled
/// shape's codec.
///
/// Holds the SAME shared engine the generated entry threads into every operation, so the
/// series a write stores are the series a query — and a `DeleteWorkspace` — sees.
pub(crate) struct PrometheusDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> PrometheusDataPlane<Eng> {
    pub(crate) fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for PrometheusDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The URI resolved to this operation, so it names a workspace's Prometheus
            // endpoint; a URI that does not is never routed here.
            let Some(api) = api_request_of(req.uri()) else {
                return text(404, NOT_FOUND_MESSAGE);
            };
            match api.endpoint {
                ApiEndpoint::RemoteWrite => self.remote_write(req, ctx, &api.workspace_id).await,
                ApiEndpoint::Query => self.instant_query(req, ctx, &api.workspace_id).await,
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(DATA_PLANE_AUTH_SCHEMES)
    }
}

/// What this plane says to a request for a workspace this Region does not hold, or one
/// whose status says its data plane is not available. One sentence carrying nothing from
/// the request.
const NOT_FOUND_MESSAGE: &str = "the Prometheus endpoint of this workspace is not available";

/// What this plane says to a method the endpoint does not serve.
const METHOD_NOT_ALLOWED_MESSAGE: &str = "this endpoint of the Prometheus API serves another method";

/// What this plane says when this service's store is not reachable.
const STORE_UNAVAILABLE_MESSAGE: &str = "the workspace store is unavailable";

/// What this plane says when the request body could not be read at all. The router
/// buffers every request before dispatch, so a body it cannot hand back in memory is a
/// failure of the server, never a caller's malformed request.
const UNREADABLE_BODY_MESSAGE: &str = "the server could not read the request body";

/// What this plane says to a query whose evaluation spent the budget one query is served
/// under ([`crate::state::promql::Budget`]). The samples are untouched: only the
/// evaluation stopped.
const QUERY_BUDGET_SPENT_MESSAGE: &str =
    "the query asked for more work than this emulator spends on one query";

/// What this plane says when the evaluation did not run to an answer at all — the task
/// carrying it failed. Nothing is reported about the samples in that case.
const QUERY_NOT_EVALUATED_MESSAGE: &str = "the query was not evaluated";

/// The query parameter an instant query carries its expression in.
const QUERY_PARAMETER: &str = "query";

/// The query parameter an instant query carries its evaluation instant in.
const TIME_PARAMETER: &str = "time";

/// The operation name the audit record of a write carries. A data-plane write changes
/// this service's state, so it leaves the record every mutating handler leaves
/// ([`crate::state::record_state_change`]); the name is the endpoint it was made through,
/// since no modelled operation names it.
const REMOTE_WRITE_RECORD: &str = "PrometheusRemoteWrite";

impl<Eng: Send + Sync + 'static> PrometheusDataPlane<Eng> {
    /// Serve `POST remote_write`: read the request, then write its samples to the
    /// workspace under one lock.
    ///
    /// Answers `204` when the samples are stored, which is the status the remote-write
    /// specification gives a write a receiver accepted (there is nothing to report back
    /// about it). Every refusal is the store's own judgement
    /// ([`crate::state::WriteRefusal`]) or the body's
    /// ([`crate::state::remote_write::DecodeError`]), so this function decides nothing
    /// about the samples itself.
    ///
    /// The decompression and the protobuf decode — the work proportional to the body —
    /// run on the blocking pool, BEFORE the store's lock is taken: they need no state, so
    /// neither the async worker nor any other caller waits on them, and the `.await` is
    /// where a timeout or a cancellation of this request lands. What runs under the lock
    /// is bounded by the per-request caps the decode already enforced.
    async fn remote_write(
        &self,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
        workspace_id: &str,
    ) -> RtResponse {
        if req.method() != "POST" {
            return text(405, METHOD_NOT_ALLOWED_MESSAGE);
        }
        let Some(body) = req.body().bytes() else {
            return text(500, UNREADABLE_BODY_MESSAGE);
        };
        // `Content-Encoding` is what the format states the compression in, so the body is
        // read as the sender announced it rather than sniffed.
        let compressed = req
            .headers()
            .get(::http::header::CONTENT_ENCODING.as_str())
            .is_some_and(|encoding| {
                encoding
                    .trim()
                    .eq_ignore_ascii_case(crate::state::remote_write::SNAPPY_ENCODING)
            });
        // Refused on the DECLARED size before the body is copied for the blocking pool,
        // so a body past the bound costs one comparison.
        if body.len() > crate::state::remote_write::MAX_COMPRESSED_BYTES {
            return text(
                400,
                crate::state::remote_write::DecodeError::TooLarge.message(),
            );
        }
        let owned = body.to_vec();
        let read = ::tokio::task::spawn_blocking(move || {
            crate::state::remote_write::read_write_request(&owned, compressed)
        })
        .await;
        let written = match read {
            Ok(Ok(written)) => written,
            Ok(Err(error)) => return text(400, error.message()),
            // The blocking task itself failed (it was cancelled, or the pool is gone):
            // nothing was read, so nothing is claimed about the samples.
            Err(_) => return text(500, UNREADABLE_BODY_MESSAGE),
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return text(500, STORE_UNAVAILABLE_MESSAGE);
        };
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();
        store.materialize_due(now);
        // A write acts on a workspace the Region must already hold, so the accessor that
        // materialises nothing is the one taken: a Region holding nothing holds no
        // workspace either, and answers as a missing workspace does.
        let held = store.held_region_mut(&region);
        let Some(workspace) = held.and_then(|held| held.workspaces.get_mut(workspace_id)) else {
            return text(404, NOT_FOUND_MESSAGE);
        };
        if !serves_its_data_plane(workspace) {
            return text(404, NOT_FOUND_MESSAGE);
        }
        let arn = workspace.arn.clone();
        match workspace
            .metrics
            .accept(&written, &workspace.configuration, now)
        {
            Ok(_) => {
                crate::state::record_state_change(REMOTE_WRITE_RECORD, &arn, ctx.parts());
                accepted()
            }
            Err(refusal) => text(refusal.status(), &refusal.message()),
        }
    }

    /// Serve `query`: evaluate one PromQL instant query against the samples the workspace
    /// holds, and answer in the Prometheus HTTP API's JSON envelope.
    ///
    /// A REPORT: it writes nothing but the expiry
    /// [`crate::state::ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`]
    /// already owed, so two queries in a row read the same samples.
    ///
    /// Nothing whose size the caller chose runs while the store is held. Under the lock
    /// this reads a SNAPSHOT — one sample per series the workspace holds, found by
    /// bisection ([`crate::state::IngestedMetrics::instant_snapshot`]) — and then releases
    /// it; the caller's expression is parsed, planned and applied to that snapshot on the
    /// blocking pool, under a step and wall-clock [`crate::state::promql::Budget`], so one
    /// query's cost is neither another caller's latency nor the async worker's.
    async fn instant_query(
        &self,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
        workspace_id: &str,
    ) -> RtResponse {
        // The Prometheus HTTP API serves an instant query on `GET` and on `POST` with a
        // form body, which is how a client sends a long expression.
        let parameters = match req.method() {
            "GET" => parameters(query_of(req.uri()).as_bytes()),
            "POST" => {
                let Some(body) = req.body().bytes() else {
                    return query_error(500, "internal", UNREADABLE_BODY_MESSAGE);
                };
                let mut parameters = parameters(body);
                parameters.extend(self::parameters(query_of(req.uri()).as_bytes()));
                parameters
            }
            _ => return query_error(405, "bad_data", METHOD_NOT_ALLOWED_MESSAGE),
        };
        let Some(expression) = parameter(&parameters, QUERY_PARAMETER) else {
            return query_error(
                400,
                "bad_data",
                &format!("invalid parameter {QUERY_PARAMETER:?}: no expression found in input"),
            );
        };
        let at = match parameter(&parameters, TIME_PARAMETER) {
            None => ::std::time::SystemTime::now(),
            Some(stated) => match timestamp_of(stated) {
                Some(at) => at,
                None => {
                    return query_error(
                        400,
                        "bad_data",
                        &format!(
                            "invalid parameter {TIME_PARAMETER:?}: cannot parse to a valid timestamp"
                        ),
                    )
                }
            },
        };
        let expression = expression.to_string();
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return query_error(500, "internal", STORE_UNAVAILABLE_MESSAGE);
        };
        let region = crate::state::request_region(ctx.parts());
        // The store is held for the snapshot alone, and the guard is dropped before the
        // query is awaited: neither the parse nor the evaluation runs under the lock.
        let snapshot = {
            let mut store = state.lock();
            store.materialize_due(::std::time::SystemTime::now());
            // Read through a SHARED borrow of the whole store, so this report cannot
            // materialise a Region's store the way a create does.
            let held = &*store;
            let Some(workspace) = held.region(&region).workspaces.get(workspace_id) else {
                return query_error(404, "not_found", NOT_FOUND_MESSAGE);
            };
            if !serves_its_data_plane(workspace) {
                return query_error(404, "not_found", NOT_FOUND_MESSAGE);
            }
            workspace
                .metrics
                .instant_snapshot(at, &workspace.configuration)
        };
        // Parsing the expression, planning it and evaluating it are all proportional to
        // what the caller sent, so all three run on the blocking pool: the async worker
        // keeps serving, and this `.await` is where a timeout or a cancellation of the
        // request lands.
        let evaluated = ::tokio::task::spawn_blocking(move || {
            let plan = crate::state::promql::plan(&expression)?;
            crate::state::promql::evaluate(
                &plan,
                snapshot,
                at,
                &mut crate::state::promql::Budget::for_one_query(),
            )
        })
        .await;
        let value = match evaluated {
            Ok(answer) => answer,
            // The blocking task itself failed (it was cancelled, or the pool is gone):
            // nothing was evaluated, so nothing is reported about the samples.
            Err(_) => return query_error(500, "internal", QUERY_NOT_EVALUATED_MESSAGE),
        };
        match value {
            Ok(value) => query_success(&value),
            Err(QueryError::Parse(reason)) => query_error(
                400,
                "bad_data",
                &format!("invalid parameter {QUERY_PARAMETER:?}: {reason}"),
            ),
            // The expression is PromQL this plane does not evaluate. Stated as what it
            // is, with no result: an answer computed from nothing would be a wrong
            // answer, and a 200 is reserved for a query that really was evaluated.
            Err(QueryError::Unsupported(kind)) => query_error(
                501,
                "not_implemented",
                &format!("this emulator does not evaluate {kind} expressions"),
            ),
            // The expression is past a bound this emulator puts on an expression's own
            // size or complexity: the caller's to simplify, so it is `bad_data` like any
            // other parameter this plane will not take.
            Err(QueryError::TooComplex(reason)) => query_error(
                400,
                "bad_data",
                &format!("invalid parameter {QUERY_PARAMETER:?}: {reason}"),
            ),
            // The expression was admissible and what it asked of the samples held was
            // more than one query may spend. `422` with `execution` is what the
            // Prometheus HTTP API answers a query it would not finish.
            Err(QueryError::Exhausted) => {
                query_error(422, "execution", QUERY_BUDGET_SPENT_MESSAGE)
            }
        }
    }
}

/// Whether a workspace serves its Prometheus endpoint.
///
/// `WorkspaceStatusCode`'s own documentation decides: `ACTIVE` — "Workspace has been
/// created and is usable." — and `UPDATING` — "Workspace is being updated" — are the
/// statuses of a workspace that exists and is usable, while a workspace still `CREATING`
/// is not usable yet and one `DELETING` is on its way out. For those two the endpoint is
/// not there to serve, which is what a caller following it is told.
fn serves_its_data_plane(workspace: &Workspace) -> bool {
    workspace.status_code == STATUS_ACTIVE || workspace.status_code == STATUS_UPDATING
}

/// The query component of a request URI, empty when it carries none.
fn query_of(uri: &str) -> &str {
    uri.split_once('?').map_or("", |(_, query)| query)
}

/// The parameters of a form-urlencoded component, decoded by the framework's own parser —
/// the same one `crate::ops::list_scrapers` binds its query string with, so this file
/// states no decoding rule of its own.
fn parameters(encoded: &[u8]) -> Vec<(String, String)> {
    ::aws_smithy_schema_server::query::parse_form_urlencoded(encoded)
}

/// The first value of `name`, as a scalar parameter takes the first value of a repeated
/// key. A parameter present but empty is no value: there is nothing to evaluate in it.
fn parameter<'p>(parameters: &'p [(String, String)], name: &str) -> Option<&'p str> {
    parameters
        .iter()
        .find(|(key, value)| key == name && !value.is_empty())
        .map(|(_, value)| value.as_str())
}

/// The instant a `time` parameter names: a Unix timestamp in seconds (fractional seconds
/// allowed, as the API documents) or an RFC 3339 instant — the two forms the Prometheus
/// HTTP API accepts for a timestamp. `None` for anything else, and for an instant no
/// clock of this process can state.
fn timestamp_of(stated: &str) -> Option<::std::time::SystemTime> {
    if let Ok(seconds) = stated.parse::<f64>() {
        if !seconds.is_finite() {
            return None;
        }
        let millis = (seconds * 1_000.0).trunc();
        if millis < i64::MIN as f64 || millis > i64::MAX as f64 {
            return None;
        }
        return crate::state::instant_of(millis as i64);
    }
    let parsed = ::chrono::DateTime::parse_from_rfc3339(stated).ok()?;
    crate::state::instant_of(parsed.timestamp_millis())
}

/// The answer to a write this plane stored: the samples are held, and the remote-write
/// specification gives a receiver's success as `204 No Content`.
fn accepted() -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(204u16).expect("204 is a valid status"),
        SdkBody::empty(),
    );
    response.headers_mut().insert("content-length", "0");
    response
}

/// One plain-text answer of the write path: the status and one reason sentence.
fn text(status: u16, message: &str) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(format!("{message}\n")),
    );
    response
        .headers_mut()
        .insert("content-type", "text/plain; charset=utf-8");
    response
}

/// One answer of the query path, in the Prometheus HTTP API's envelope: `status`
/// `"success"` and the `data` the query evaluated to.
fn query_success(value: &QueryValue) -> RtResponse {
    let data = match value {
        QueryValue::Scalar(sample) => ::serde_json::json!({
            "resultType": "scalar",
            "result": sample_json(sample),
        }),
        QueryValue::Vector(elements) => ::serde_json::json!({
            "resultType": "vector",
            "result": elements
                .iter()
                .map(|element| ::serde_json::json!({
                    "metric": element.labels.iter()
                        .map(|(label, value)| (label.clone(), ::serde_json::Value::from(value.clone())))
                        .collect::<::serde_json::Map<String, ::serde_json::Value>>(),
                    "value": sample_json(&element.sample),
                }))
                .collect::<Vec<_>>(),
        }),
    };
    json(
        200,
        &::serde_json::json!({ "status": "success", "data": data }),
    )
}

/// One refusal of the query path, in the same envelope: `status` `"error"`, the
/// `errorType` the API names the class by, and the reason.
fn query_error(status: u16, error_type: &str, message: &str) -> RtResponse {
    json(
        status,
        &::serde_json::json!({
            "status": "error",
            "errorType": error_type,
            "error": message,
        }),
    )
}

/// One sample as the Prometheus HTTP API carries it: `[<seconds>, "<value>"]`, the
/// timestamp in seconds and the value as a string.
fn sample_json(sample: &crate::state::Sample) -> ::serde_json::Value {
    ::serde_json::json!([
        sample.timestamp_ms as f64 / 1_000.0,
        sample_value(sample.value)
    ])
}

/// A sample's value as the Prometheus HTTP API writes it: the shortest decimal that reads
/// back as the same number, and the three non-finite values spelled as that API spells
/// them.
fn sample_value(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "+Inf".to_string()
        } else {
            "-Inf".to_string()
        };
    }
    format!("{value}")
}

/// One JSON answer of the query path. The media type is the Prometheus HTTP API's own,
/// not this service's modelled envelope: the body is that API's document, and a client of
/// it reads `application/json`.
fn json(status: u16, body: &::serde_json::Value) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", crate::state::JSON_CONTENT_TYPE);
    response
}

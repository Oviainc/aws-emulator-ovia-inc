// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-artifact-artifact` service: how it claims requests and how it words the
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
//! ## The front door
//!
//! AWS Artifact is not reached directly: it is served through a gateway that, BEFORE
//! the service runs,
//!
//! 1. authorizes the resource the request targets, and refuses a target it cannot
//!    authorize with HTTP 401 `UnauthorizedException`; and
//! 2. validates the request BODY against the modelled shape, and refuses a body it
//!    cannot bind with HTTP 400 `BadRequestException`.
//!
//! Neither shape is in this service's model — the gateway answers before the service
//! is reached, so it answers in its own shapes — and neither refusal is a modelled
//! error: a caller sees `UnauthorizedException` where the service itself would have
//! said `ResourceNotFoundException`, and `BadRequestException` where it would have
//! said `ValidationException`. Values the request carries in its URI are NOT the
//! gateway's concern: a `@httpQuery` or `@httpLabel` value outside its modelled
//! constraint is refused by THIS service, as its modelled 400 `ValidationException` —
//! the whole shape, `reason` and `fieldList` included, not a message on its own.
//!
//! [`EmulatedService::examine`] is those three judgements for every request this service serves, reached
//! through [`Service::override_operation`] because all of them precede the modelled walk:
//! constraint validation runs before a handler, so a handler cannot answer ahead of it.
//! The framework's own boundary refusal cannot carry the modelled shape (the seam it
//! offers chooses an error code and a message, and nothing else), so a request whose
//! URI-bound value the model's constraints refuse is answered HERE, by
//! [`ConstraintRefusal`], in the one envelope `crate::state` words every such refusal in.
//! `override_refusal` below stays as the wording of anything that still reaches that
//! boundary. The one judgement the gateway cannot make here is whether a well-formed
//! identifier names a resource the account holds — that is this service's state — so each
//! report handler makes it, through `crate::scaffolded::Scaffolded::unauthorized`, and
//! answers the same 401.
//!
//! ## The document plane
//!
//! `ExportComplianceInquiry` hands out a PRESIGNED URL, and a presigned URL is a promise
//! that a caller can follow it: it names the endpoint the CALLER reached this emulator on
//! (`crate::state::document_url_host`), under the path
//! `crate::state::EXPORT_DOCUMENT_PATH_PREFIX` owns, which names no modelled operation.
//! Two seams carry it, the same pair any emulated data plane uses:
//!
//! - [`Service::can_handle`] also claims a request to a path this service publishes its
//!   exports at, so a fetch that carries neither a credential nor a host naming a service
//!   — which is every fetch of a URL this service handed out — is not answered
//!   `UnknownService`;
//! - [`Service::override_routing_error`] serves it. A document request names no modelled
//!   operation by construction, so the router reaches exactly this seam for it, and
//!   reaches it BEFORE authentication — which is what lets a presigned URL be followed
//!   without a signature, as a presigned URL is.
//!
//! What it serves is the document the export STORED
//! (`crate::state::InquiryExport::document`), rendered from the inquiry when the export
//! ran: a download is a pure read, and a URL whose export has since been released, or
//! that names another inquiry or another object, serves no document. The fetch is
//! authorised by the download token the announced URL carries
//! (`crate::state::InquiryExport::download_token`), the ONE thing a caller that was handed
//! the URL holds — the ids its path carries are minted from counters, so they authorise
//! nobody — and a fetch presenting no such token is refused `403`, as the report plane
//! below refuses a fetch carrying no term grant.
//!
//! The REPORT and TERM documents `GetReport` and `GetTermForReport` hand out are served
//! the same way, by [`Documents`] under `crate::state::DOCUMENT_PATH_PREFIX`: their bytes
//! are PERSISTED beside the catalog record they belong to (`crate::state::Document`), so
//! the URL those operations announce resolves to a document this service holds rather
//! than to a dead locator. What the page says is the stored record and that the emulator
//! published it — an AWS report's own contents are AWS's and are not invented here. That
//! URL names the endpoint the CALLER reached this emulator on
//! (`crate::state::document_url_host`), because this emulator is what serves it.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{path, validate, Violation, ViolationKind};
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, Refusal, Refused,
    ServerHttpProtocol, Service,
};
use std::marker::PhantomData;
use std::sync::Arc;

use crate::ops::types;
use crate::scaffolded::FrontDoorRefusal;

/// Which refusal the front door states for one request. The wire facts of each
/// (status, error code, message) live on [`FrontDoorRefusal`], not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrontDoorVerdict {
    /// The request targets a resource the gateway cannot authorize.
    Unauthorized,
    /// The request's body does not bind to the modelled shape.
    InvalidRequestBody,
}

/// What the pre-dispatch examination of one request concluded, when it refuses it.
///
/// Two layers answer before the modelled walk, and they are NOT the same refusal: the
/// gateway in front of this service judges the target and the body in its own shapes (see
/// the module docs), while a value the request carried in its URI is this service's own to
/// refuse, as the modelled `ValidationException`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreDispatchRefusal {
    /// The gateway refuses the request before this service is reached.
    FrontDoor(FrontDoorVerdict),
    /// The model's own constraints refuse values the request carried in its URI; each
    /// violation names the member and what its value failed.
    InvalidInput(Vec<Violation>),
}

/// The resource one request targets, as the gateway reads it: the kind of identifier
/// the operation's input carries, and the value the caller sent.
///
/// The gateway authorizes the resource ARN it builds from that identifier, so a value
/// that is not a well-formed identifier of its type names no resource to authorize at
/// all — which is why this judgement precedes the modelled constraint validation that
/// would otherwise refuse the same value as a `ValidationException`.
#[derive(Debug, Clone, Copy)]
enum Target<'a> {
    /// A `reportId` (`ReportId`), as every report operation carries.
    Report(&'a str),
    /// A `complianceInquiryId` (`InquiryId`), as every compliance-inquiry operation
    /// carries.
    ComplianceInquiry(&'a str),
}

impl Target<'_> {
    /// Whether this identifier can name a resource of its type at all.
    ///
    /// The syntax of each id lives in `crate::state` beside the catalog it keys, and
    /// is the model's: `ReportId`'s `^report-[a-zA-Z0-9]{16}$` and `InquiryId`'s
    /// `^compliance-inquiry-[a-zA-Z0-9]{16}$`.
    fn names_a_resource(&self) -> bool {
        match self {
            Self::Report(id) => crate::state::is_report_id(id),
            Self::ComplianceInquiry(id) => crate::state::is_inquiry_id(id),
        }
    }
}

/// One refusal the front door answers with, as a pre-dispatch operation.
///
/// WHY this is a seam and not a line in a handler: both refusals happen BEFORE the
/// modelled walk — the gateway authorizes the target and validates the body ahead of
/// the service — and the walk refuses a modelled constraint violation before any
/// handler runs, so neither answer is reachable from inside `handle`.
/// [`Service::override_operation`] is the one hook that sees the whole request ahead of
/// that walk, which is exactly what this needs.
struct FrontDoorAnswer(FrontDoorRefusal);

impl ErasedOperation for FrontDoorAnswer {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Rendered through the service's own protocol, so the front door's refusal
            // carries the same envelope (and the same `x-amzn-errortype` header) as
            // every other error this service answers. The status and the error code
            // come from the refusal itself: the shape is not in the model, so there is
            // no modelled error to read them off.
            svc.definition()
                .protocol()
                .serialize_error(
                    &self.0,
                    &crate::scaffolded::FRONT_DOOR_SCHEMA,
                    self.0.code,
                    self.0.status,
                )
                .unwrap_or_else(|e| svc.definition().framework_error_response(e))
        })
    }
}

/// Whether the member at `path` is carried in the request BODY.
///
/// Read off the member schema the model generated — a member with an HTTP location
/// (`@httpLabel`, `@httpQuery`, `@httpQueryParams`, `@httpHeader`,
/// `@httpPrefixHeaders`) travels in the URI, and everything else is body — so no
/// binding is listed here. The first path segment is the member: a violation deeper
/// inside a structure, union, list or map names the top-level member first, and that
/// member is what the request carries the value in.
fn body_bound(schema: &Schema, violated: &str) -> bool {
    let member = violated.split('.').next().unwrap_or(violated);
    schema.members().iter().any(|candidate| {
        candidate
            .member_name()
            .is_some_and(|name| path::member(name) == member)
            && candidate.http_label().is_none()
            && candidate.http_query().is_none()
            && candidate.http_query_params().is_none()
            && candidate.http_header().is_none()
            && candidate.http_prefix_headers().is_none()
    })
}

/// Run the pre-dispatch checks over one operation's request: the front door's two, and
/// this service's own refusal of a URI-bound value the model's constraints reject.
///
/// `target` names the resource the input targets, for an operation whose input carries
/// an identifier, and `None` for one that does not.
///
/// The input is read through the PROTOCOL's own binding against the operation's own
/// input schema, so the value judged is the value dispatch would hand the handler —
/// percent-decoding, query and body alike — and the constraints checked are the
/// model's, through the same validator the walk runs. Deserialization does not enforce
/// constraints, which is what lets the gateway judge a value the modelled constraint
/// would have refused.
///
/// The layering decides the order: the gateway answers ahead of this service, so an
/// unauthorizable target comes first and a body it cannot bind second, and only a request
/// both of them pass is refused for a value its URI carried.
fn judge<I>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    schema: &'static Schema,
    target: fn(&I) -> Option<Target<'_>>,
) -> Option<PreDispatchRefusal>
where
    I: DeserializableStruct + SerializableStruct,
{
    let mut deserializer = protocol.deserialize_input(req, schema).ok()?;
    let input = match I::deserialize(&mut *deserializer) {
        Ok(input) => input,
        // A member the shape requires, absent from the body, is a body the gateway
        // cannot bind. A URI-bound one is this service's own refusal, stated against the
        // member the request did not carry — absence is a constraint violation like any
        // other, found while reading the request rather than while walking it. Anything
        // else the codec could not read (a body that is not the protocol's format at all)
        // is left to the framework's own refusal: the gateway's answer to it is not
        // recorded.
        Err(aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => {
            return Some(if body_bound(schema, &member_name) {
                PreDispatchRefusal::FrontDoor(FrontDoorVerdict::InvalidRequestBody)
            } else {
                PreDispatchRefusal::InvalidInput(vec![Violation {
                    path: member_name,
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                }])
            })
        }
        Err(_) => return None,
    };

    // 1. Authorize the target, ahead of every constraint the model declares.
    if target(&input).is_some_and(|target| !target.names_a_resource()) {
        return Some(PreDispatchRefusal::FrontDoor(FrontDoorVerdict::Unauthorized));
    }

    // 2. Validate the body. A violation on a URI-bound member is NOT the gateway's:
    // the service refuses that one itself, as its modelled `ValidationException`.
    let violations = validate(&input, schema);
    if violations
        .iter()
        .any(|violation| body_bound(schema, &violation.path))
    {
        return Some(PreDispatchRefusal::FrontDoor(
            FrontDoorVerdict::InvalidRequestBody,
        ));
    }

    // 3. And the service's own refusal of what the URI carried.
    (!violations.is_empty()).then_some(PreDispatchRefusal::InvalidInput(violations))
}

/// Answers a request whose URI-bound values the model's own constraints refuse, in this
/// service's modelled `ValidationException` — the WHOLE shape, which declares `reason`
/// @required beside `message` and carries `fieldList` where the refused member is known.
///
/// WHY this is a seam and not the framework's boundary refusal: constraint validation runs
/// ahead of every handler, and the hook it offers a service
/// ([`Service::override_refusal`]) chooses an error code and a message and nothing else,
/// so the required `reason` cannot travel on it. [`Service::override_operation`] is the one
/// hook that sees the whole request ahead of that walk, which is what lets the refusal be
/// answered in the modelled shape — the same payload `crate::state` builds for every
/// field-validation refusal this service states itself.
///
/// The violations are derived from the request, by the same [`EmulatedService::examine`]
/// the routing decision used: nothing about one request is carried on this value, so one
/// instance serves every caller.
struct ConstraintRefusal<Eng> {
    /// The engine type this service's operations are named through; no store is read
    /// here, as a refusal of what the request CARRIED needs none.
    engine: PhantomData<fn() -> Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ConstraintRefusal<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let violations = match EmulatedService::<Eng>::examine(protocol, req) {
                Some(PreDispatchRefusal::InvalidInput(violations)) => violations,
                // Unreachable: this answer is routed to only for a request whose URI-bound
                // values the same examination refused, and it re-reads the same request.
                // Stated explicitly rather than folded into the arm above, so a routing
                // change that reached it leaves a record instead of a bare 400 — and
                // without failing the request on an invariant, which a refusal must never
                // do.
                other => {
                    ::tracing::warn!(
                        found = ?other,
                        "a request answered as a constraint refusal carried no modelled \
                         constraint violation"
                    );
                    Vec::new()
                }
            };
            match crate::state::constraint_violation_exception(&violations) {
                // Rendered through the service's own protocol, as every other modelled
                // error of this service is, with the status and the code the model
                // declares for the shape (`@error("client")`, `@httpError(400)`).
                Some(refusal) => protocol
                    .serialize_error(
                        &refusal,
                        &types::SCHEMA_VALIDATION_EXCEPTION,
                        VALIDATION_EXCEPTION_CODE,
                        VALIDATION_EXCEPTION_STATUS,
                    )
                    .unwrap_or_else(|e| svc.definition().framework_error_response(e)),
                // With no violation to state there is nothing this service can word, so
                // the framework answers the refusal itself, as it did before this seam.
                None => svc
                    .definition()
                    .framework_error_response(ProtocolError::Invalid(violations)),
            }
        })
    }
}

/// The wire error code of this service's modelled input refusal (`__type` and
/// `x-amzn-errortype`), which is the shape's own name.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The HTTP status the model declares for it (`@httpError(400)`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// Serves a report or term document from the stored bytes [`crate::state`] holds.
///
/// This is the data plane behind the URL `GetReport` and `GetTermForReport` announce. It
/// is NOT a modelled operation — AWS serves those documents from a presigned S3 URL, and
/// this emulator serves them from its own endpoint, because a URL it announces has to
/// resolve. It only READS: a download neither consumes the grant it was authorised by
/// nor changes the document.
struct Documents<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for Documents<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The same method policy the export plane answers under: a document is READ,
            // so `GET` carries it and `HEAD` reports its metadata without the bytes;
            // anything else is refused as the method it is.
            if !matches!(req.method(), "GET" | "HEAD") {
                return method_not_allowed();
            }
            let (path, query) = split_uri(req.uri());
            let Some(wanted) = parse_document_path(&path) else {
                return document_error(404, "NoSuchKey", DOCUMENT_NOT_FOUND_MESSAGE);
            };
            let Some(state) = crate::state::service_state(&*self.engine) else {
                return document_error(
                    500,
                    "InternalServerException",
                    "the artifact report catalog is unavailable",
                );
            };
            let store = state.lock();
            // A download is authorised by the SAME rule `GetReport` applies: one rule,
            // one place. Where the report's acceptance type requires the explicit
            // acceptance of its term, only a grant covering that term authorises the
            // fetch — and a fetch carrying NO token carries no grant, so the empty token
            // is what that rule is asked about.
            //
            // DELIBERATE ORDERING: authorization answers first, so a path naming a
            // report this service does not hold is refused as unauthorised (403) rather
            // than reported as absent — a document URL is handed to one caller, and a
            // 404 here would let anyone probe which report ids exist. Only a path this
            // rule authorises can be answered `404`, for the version or the object the
            // report does not publish.
            let token = query_param(&query, crate::state::TERM_TOKEN_PARAM).unwrap_or_default();
            let authorised = store
                .reports
                .get(&wanted.report_id)
                .is_some_and(|report| store.download_authorised(&token, report));
            if !authorised {
                ::tracing::info!(
                    operation = "GetReportDocument",
                    report_id = ?wanted.report_id,
                    report_version = wanted.version,
                    document_kind = ?wanted.kind,
                    outcome = "denied",
                    reason = "no term grant covers this report's term",
                    "refused a document fetch"
                );
                return document_error(403, "AccessDenied", crate::state::INVALID_TOKEN_MESSAGE);
            }
            let held = store.document(&wanted.kind, &wanted.report_id, wanted.version);
            let Some(document) = held.filter(|held| held.object == wanted.object) else {
                return document_error(404, "NoSuchKey", DOCUMENT_NOT_FOUND_MESSAGE);
            };
            ::tracing::info!(
                operation = "GetReportDocument",
                report_id = ?wanted.report_id,
                report_version = wanted.version,
                document_kind = ?wanted.kind,
                outcome = "served",
                "served a document"
            );
            document_response(
                &document.bytes,
                document.content_type,
                req.method() == "GET",
            )
        })
    }

    /// Served UNSIGNED. A document URL is handed to a caller to fetch directly, so the
    /// fetch carries no SigV4 `Authorization` header; the term grant the URL carries is
    /// what authorises it, and a fetch without one is refused. The modelled operations
    /// are unaffected — the router reads their schemes from the model, which this answer
    /// cannot weaken.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

/// What a document path named.
struct WantedDocument {
    kind: String,
    report_id: String,
    version: i64,
    object: String,
}

/// The document a data-plane path names, or `None` when the path is not one this service
/// publishes documents at.
fn parse_document_path(path: &str) -> Option<WantedDocument> {
    let rest = path
        .strip_prefix(crate::state::DOCUMENT_PATH_PREFIX)?
        .strip_prefix('/')?;
    let mut segments = rest.split('/');
    let kind = segments.next()?;
    let report_id = segments.next()?;
    let version = segments.next()?.strip_prefix('v')?.parse::<i64>().ok()?;
    let object = segments.next()?;
    if segments.next().is_some() {
        return None;
    }
    Some(WantedDocument {
        kind: smithy_server_runtime::percent_decode(kind),
        report_id: smithy_server_runtime::percent_decode(report_id),
        version,
        object: smithy_server_runtime::percent_decode(object),
    })
}

/// Serve one stored document: its own bytes, under its own content type and length.
///
/// `with_body` is false for a `HEAD`, which reports a document's metadata without its
/// bytes. BOTH document planes of this service answer through here, so one service does
/// not serve its documents under two conventions.
fn document_response(bytes: &[u8], content_type: &str, with_body: bool) -> RtResponse {
    let body = if with_body {
        SdkBody::from(bytes.to_vec())
    } else {
        SdkBody::empty()
    };
    let mut response = RtResponse::new(status(200), body);
    response
        .headers_mut()
        .insert("content-type", content_type.to_string());
    response
        .headers_mut()
        .insert("content-length", bytes.len().to_string());
    response
}

/// A status this module answers with. Every call site passes a constant, so the
/// conversion cannot fail.
fn status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("the data plane's statuses are valid constants")
}

/// A request URI's path and query string, in whichever form the request carries them:
/// an origin-form `/path?query`, or the absolute-form `https://host/path?query` an
/// SDK-built request carries.
///
/// Parsed with `::http::Uri` — the same parser [`Service::can_handle`] reads
/// `parts.uri.path()` from — so the request this service CLAIMS and the request it then
/// dispatches cannot disagree about which path was asked for. It is the one path reader
/// in this module: both document planes (the report documents below and the inquiry
/// exports above) resolve their path through it.
fn split_uri(uri: &str) -> (String, String) {
    if let Ok(parsed) = uri.parse::<::http::Uri>() {
        return (
            parsed.path().to_string(),
            parsed.query().unwrap_or_default().to_string(),
        );
    }
    // A form the URI parser will not accept is still matched the way it reads, rather
    // than discarded: split at the `?` that separates the two parts.
    match uri.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (uri.to_string(), String::new()),
    }
}

/// The value `name` carries in a URL query string, decoded. `None` when the query string
/// does not carry it.
///
/// The grammar is NOT restated here: the pairs are split and decoded by the framework's
/// own query decoder (`aws_smithy_schema_server::query::parse_form_urlencoded`), which is
/// what the protocol's `@httpQuery` binding reads a request with, so the document plane
/// and the modelled operations cannot disagree about decoding (`+`, `%XX`, a valueless
/// key). First match wins, as a repeated key's first binding does.
fn query_param(query: &str, name: &str) -> Option<String> {
    ::aws_smithy_schema_server::query::parse_form_urlencoded(query.as_bytes())
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// The `com-amazonaws-artifact-artifact` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the document plane is served from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own; it must read the one store the operations write, not a
    /// second copy.
    engine: Arc<Eng>,
    /// Answers a request whose targeted resource the front door cannot authorize.
    unauthorized: FrontDoorAnswer,
    /// Answers a request whose body the front door cannot bind to the modelled shape.
    invalid_request_body: FrontDoorAnswer,
    /// Answers a request whose URI-bound values the model's own constraints refuse, in
    /// this service's modelled `ValidationException`.
    invalid_input: ConstraintRefusal<Eng>,
    /// Serves the report and term documents the control plane announces URLs for.
    documents: Documents<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine: Arc::clone(&engine),
            unauthorized: FrontDoorAnswer(FrontDoorRefusal::unauthorized()),
            invalid_request_body: FrontDoorAnswer(FrontDoorRefusal::invalid_request_body()),
            invalid_input: ConstraintRefusal {
                engine: PhantomData,
            },
            documents: Documents { engine },
        }
    }

    /// The pre-dispatch judgement on one request, or `None` when it passes through to the
    /// modelled walk.
    ///
    /// One arm per operation, because the judgement needs that operation's own input
    /// shape: the gateway's two checks for an input that names a resource or carries
    /// members in its body, and the model's constraints over what the URI carried for
    /// every operation, which is every one of them. Each arm names its operation through
    /// that operation's own [`Operation::NAME`] and its input through the model-derived
    /// input schema, so nothing here is spelled out that the model already states.
    ///
    /// Takes the protocol rather than reading it off `self`, so the refusal this returns
    /// can be re-derived from the same request by [`ConstraintRefusal`], which is handed a
    /// `&dyn Service` and not this type.
    ///
    /// The three tag operations' `resourceArn` is deliberately not a [`Target`]: it is a
    /// `LongStringAttribute` the model gives no identifier syntax (any string without
    /// `<>` is admissible), so whether it names a resource at all is a question about
    /// this service's STATE — which tag set the ARN addresses — and each tag handler
    /// answers it with the same 401 through `crate::scaffolded::Scaffolded::unauthorized`.
    fn examine(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Option<PreDispatchRefusal> {
        use crate::ops;

        let key = protocol.routing_key(req).ok()?;

        // Reports: the `reportId` every report operation targets.
        if key == <ops::get_report_metadata::GetReportMetadata<Eng> as Operation>::NAME {
            return judge::<types::GetReportMetadataInput>(
                protocol,
                req,
                types::GET_REPORT_METADATA_INPUT_SCHEMA,
                |input| Some(Target::Report(&input.report_id)),
            );
        }
        if key == <ops::list_report_versions::ListReportVersions<Eng> as Operation>::NAME {
            return judge::<types::ListReportVersionsInput>(
                protocol,
                req,
                types::LIST_REPORT_VERSIONS_INPUT_SCHEMA,
                |input| Some(Target::Report(&input.report_id)),
            );
        }
        if key == <ops::get_term_for_report::GetTermForReport<Eng> as Operation>::NAME {
            return judge::<types::GetTermForReportInput>(
                protocol,
                req,
                types::GET_TERM_FOR_REPORT_INPUT_SCHEMA,
                |input| Some(Target::Report(&input.report_id)),
            );
        }
        if key == <ops::get_report::GetReport<Eng> as Operation>::NAME {
            return judge::<types::GetReportInput>(
                protocol,
                req,
                types::GET_REPORT_INPUT_SCHEMA,
                |input| Some(Target::Report(&input.report_id)),
            );
        }

        // Compliance inquiries: the `complianceInquiryId` each one targets, whether the
        // model binds it to the query string or to the body.
        if key
            == <ops::get_compliance_inquiry_metadata::GetComplianceInquiryMetadata<Eng> as Operation>::NAME
        {
            return judge::<types::GetComplianceInquiryMetadataInput>(
                protocol,
                req,
                types::GET_COMPLIANCE_INQUIRY_METADATA_INPUT_SCHEMA,
                |input| Some(Target::ComplianceInquiry(&input.compliance_inquiry_id)),
            );
        }
        if key
            == <ops::list_compliance_inquiry_queries::ListComplianceInquiryQueries<Eng> as Operation>::NAME
        {
            return judge::<types::ListComplianceInquiryQueriesInput>(
                protocol,
                req,
                types::LIST_COMPLIANCE_INQUIRY_QUERIES_INPUT_SCHEMA,
                |input| Some(Target::ComplianceInquiry(&input.compliance_inquiry_id)),
            );
        }
        if key == <ops::export_compliance_inquiry::ExportComplianceInquiry<Eng> as Operation>::NAME {
            return judge::<types::ExportComplianceInquiryInput>(
                protocol,
                req,
                types::EXPORT_COMPLIANCE_INQUIRY_INPUT_SCHEMA,
                |input| Some(Target::ComplianceInquiry(&input.compliance_inquiry_id)),
            );
        }
        if key
            == <ops::put_compliance_inquiry_feedback::PutComplianceInquiryFeedback<Eng> as Operation>::NAME
        {
            return judge::<types::PutComplianceInquiryFeedbackInput>(
                protocol,
                req,
                types::PUT_COMPLIANCE_INQUIRY_FEEDBACK_INPUT_SCHEMA,
                |input| Some(Target::ComplianceInquiry(&input.compliance_inquiry_id)),
            );
        }

        // Bodies without a target of their own.
        if key == <ops::create_compliance_inquiry::CreateComplianceInquiry<Eng> as Operation>::NAME {
            return judge::<types::CreateComplianceInquiryInput>(
                protocol,
                req,
                types::CREATE_COMPLIANCE_INQUIRY_INPUT_SCHEMA,
                |_| None,
            );
        }
        if key == <ops::put_account_settings::PutAccountSettings<Eng> as Operation>::NAME {
            return judge::<types::PutAccountSettingsInput>(
                protocol,
                req,
                types::PUT_ACCOUNT_SETTINGS_INPUT_SCHEMA,
                |_| None,
            );
        }
        if key == <ops::tag_resource::TagResource<Eng> as Operation>::NAME {
            return judge::<types::TagResourceInput>(
                protocol,
                req,
                types::TAG_RESOURCE_INPUT_SCHEMA,
                |_| None,
            );
        }

        // And every remaining operation, which neither names a resource the gateway
        // authorizes nor carries a body it binds — so what is judged for these is only
        // what their URI carried, against the model's own constraints. They are listed
        // for exactly that: the refusal of a `@httpQuery` or `@httpLabel` value is this
        // service's, and it is the same refusal on every operation that has one.
        if key == <ops::list_reports::ListReports<Eng> as Operation>::NAME {
            return judge::<types::ListReportsInput>(
                protocol,
                req,
                types::LIST_REPORTS_INPUT_SCHEMA,
                |_| None,
            );
        }
        if key == <ops::list_customer_agreements::ListCustomerAgreements<Eng> as Operation>::NAME {
            return judge::<types::ListCustomerAgreementsInput>(
                protocol,
                req,
                types::LIST_CUSTOMER_AGREEMENTS_INPUT_SCHEMA,
                |_| None,
            );
        }
        if key == <ops::list_compliance_inquiries::ListComplianceInquiries<Eng> as Operation>::NAME {
            return judge::<types::ListComplianceInquiriesInput>(
                protocol,
                req,
                types::LIST_COMPLIANCE_INQUIRIES_INPUT_SCHEMA,
                |_| None,
            );
        }
        if key == <ops::get_account_settings::GetAccountSettings<Eng> as Operation>::NAME {
            return judge::<types::GetAccountSettingsInput>(
                protocol,
                req,
                types::GET_ACCOUNT_SETTINGS_INPUT_SCHEMA,
                |_| None,
            );
        }
        if key == <ops::list_tags_for_resource::ListTagsForResource<Eng> as Operation>::NAME {
            return judge::<types::ListTagsForResourceInput>(
                protocol,
                req,
                types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
                |_| None,
            );
        }
        if key == <ops::untag_resource::UntagResource<Eng> as Operation>::NAME {
            return judge::<types::UntagResourceInput>(
                protocol,
                req,
                types::UNTAG_RESOURCE_INPUT_SCHEMA,
                |_| None,
            );
        }

        // A request naming no modelled operation of this service: there is no input shape
        // to judge it against, and the router answers it itself.
        None
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
        // A REPORT or TERM document fetch carries no credential and no `X-Amz-Target`,
        // and when a client addresses the emulator directly rather than by endpoint host
        // its host names no service either, so the path this service publishes those
        // documents at is the only thing left to identify it. The paths claimed here are
        // the ones `crate::state::DOCUMENT_PATH_PREFIX` owns, which no modelled operation
        // of any service is bound to, so claiming a read of one cannot take a request
        // another service would have served. Every method is claimed, not only the two
        // that are served: a `POST` to a path this service publishes is this service's to
        // refuse as a method, exactly as the inquiry-export plane refuses it.
        if parse_document_path(parts.uri.path()).is_some() {
            return true;
        }
        // And the same claim for the compliance-inquiry EXPORT documents, whose paths
        // `crate::state::EXPORT_DOCUMENT_PATH_PREFIX` owns: a fetch of a presigned URL
        // this service handed out carries no credential and no host naming a service
        // either, so without this the router answers `UnknownService` for a URL this
        // service announced. See the module docs.
        crate::state::parse_export_document_path(parts.uri.path()).is_some()
    }

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // The document data plane comes first: a fetch of a URL this service announced
        // names no modelled operation at all, so there is no routing key to resolve for
        // it (see the module docs). The plane itself decides the method: a read is served
        // and anything else is refused as a method, rather than falling through to the
        // router's answer for an unrouted request.
        if parse_document_path(&split_uri(req.uri()).0).is_some() {
            return Some(&self.documents as &dyn ErasedOperation);
        }

        // And the three refusals that precede the modelled walk (see the module docs):
        // this service is reached through a gateway whose authorization of the target and
        // binding of the body answer before it, and a value the request carried in its URI
        // is refused by this service in its modelled `ValidationException`, which the
        // framework's own boundary refusal cannot carry.
        //
        // The request's auth schemes are unaffected: the router reads them off the
        // MODELLED operation the routing key names, so a pre-dispatch refusal is still
        // only reached by a request whose signature verified.
        match Self::examine(self.definition().protocol(), req)? {
            PreDispatchRefusal::FrontDoor(FrontDoorVerdict::Unauthorized) => {
                Some(&self.unauthorized as &dyn ErasedOperation)
            }
            PreDispatchRefusal::FrontDoor(FrontDoorVerdict::InvalidRequestBody) => {
                Some(&self.invalid_request_body as &dyn ErasedOperation)
            }
            PreDispatchRefusal::InvalidInput(_) => {
                Some(&self.invalid_input as &dyn ErasedOperation)
            }
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // A refusal of a modelled constraint on a value the request carried does not reach
        // here: `override_operation` answers it ahead of the walk, in the modelled
        // `ValidationException` whose required `reason` this seam cannot carry (`Refused`
        // is a code and a message). This stays as the wording of whatever still reaches
        // the boundary — a refusal on a request `examine` could not read an input from —
        // so such a request is still answered in this service's validation envelope,
        // naming the member path and the requirement it failed WITHOUT quoting the
        // rejected value back at the caller. The STATUS and the CODE are already the
        // framework's, and are what the model declares (`ValidationException`,
        // `@httpError(400)`); the code is restated unchanged alongside the reworded
        // message, because a message without its matching code is worse than leaving
        // both alone.
        //
        // An authentication refusal is left to the framework: it is not a
        // modelled-input failure and this service does not restate it.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        Some(Refused {
            code: VALIDATION_EXCEPTION_CODE.to_string(),
            message: crate::state::constraint_violation_message(violations)?,
        })
    }

    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A request to a DOCUMENT URL this service handed out is exactly that case by
        // construction — the path is not in the model — so this is where an export's
        // presigned URL is served. Anything else keeps the framework's default.
        let target = crate::state::parse_export_document_path(&split_uri(req.uri()).0)?;
        Some(self.serve_export_document(req, &target))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<RtResponse> {
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
    /// Serve one request to a published export-document URL.
    ///
    /// A download is a pure READ: it answers the bytes the export stored and writes
    /// nothing, so following a presigned URL twice answers the same document and changes
    /// nothing a later caller sees.
    ///
    /// The fetch is authorised by the token the announced URL carries, by the one rule
    /// `crate::state` holds the export-download question in
    /// ([`crate::state::ComAmazonawsArtifactArtifactState::export_download_authorised`]),
    /// as a report download is authorised by its term grant's: the URL was handed to one
    /// caller, and the token is what that caller holds. A fetch carrying NO token carries
    /// no receipt, so the empty token is what that rule is asked about.
    ///
    /// DELIBERATE ORDERING, the same the report plane answers under: authorization first,
    /// so a path naming an export this store does not hold — one it never minted, one it
    /// has since released, or a path crossing one export's id with another inquiry's — is
    /// refused as unauthorised (403) rather than reported as absent, because the ids the
    /// path carries are minted from counters and a 404 here would let anyone enumerate
    /// which exports exist. Only a path this rule authorises can be answered `404`, for an
    /// object the export does not publish.
    fn serve_export_document(
        &self,
        req: &RtRequest,
        target: &crate::state::ExportDocumentTarget,
    ) -> RtResponse {
        if !matches!(req.method(), "GET" | "HEAD") {
            return method_not_allowed();
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return document_error(
                500,
                "InternalServerException",
                "the artifact compliance inquiry store is unavailable",
            );
        };
        let (_, query) = split_uri(req.uri());
        let store = state.lock();
        let token =
            query_param(&query, crate::state::INQUIRY_EXPORT_TOKEN_PARAM).unwrap_or_default();
        if !store.export_download_authorised(&token, target) {
            ::tracing::info!(
                operation = "GetComplianceInquiryExportDocument",
                compliance_inquiry_id = ?target.inquiry_id,
                export_id = ?target.export_id,
                outcome = "denied",
                reason = "no export token authorises this document",
                "refused a document fetch"
            );
            return document_error(403, "AccessDenied", crate::state::INVALID_TOKEN_MESSAGE);
        }
        let found = store
            .inquiry_exports
            .get(&target.export_id)
            .filter(|_| target.object == crate::state::INQUIRY_EXPORT_OBJECT);
        let Some(export) = found else {
            return document_error(404, "NoSuchKey", DOCUMENT_NOT_FOUND_MESSAGE);
        };
        ::tracing::info!(
            operation = "GetComplianceInquiryExportDocument",
            compliance_inquiry_id = ?target.inquiry_id,
            export_id = ?target.export_id,
            outcome = "served",
            "served a document"
        );

        // The same serving rule both document planes answer under: a `GET` carries the
        // bytes and a `HEAD` reports the metadata without them.
        document_response(
            &export.document,
            crate::state::INQUIRY_EXPORT_CONTENT_TYPE,
            req.method() == "GET",
        )
    }
}

/// The body of a document-plane refusal that found no document: one sentence carrying
/// nothing from the request, so a caller cannot probe for exports it was not handed.
const DOCUMENT_NOT_FOUND_MESSAGE: &str = "The requested document does not exist";

/// The read methods a document plane of this service serves: `GET` carries the bytes and
/// `HEAD` reports the metadata without them.
const DOCUMENT_READ_METHODS: &str = "GET, HEAD";

/// The refusal BOTH document planes answer a method they do not serve with. RFC 9110
/// requires a `405` to state which methods the target does support, so the refusal names
/// them in the `Allow` header and in its message rather than only in this file.
fn method_not_allowed() -> RtResponse {
    let mut response = document_error(
        405,
        "MethodNotAllowed",
        "This document is served by GET or HEAD",
    );
    response
        .headers_mut()
        .insert("allow", DOCUMENT_READ_METHODS);
    response
}

/// One document-plane error response: the status, the code in `x-amzn-errortype`, and a
/// JSON body carrying the reason.
///
/// The document plane is plain HTTPS rather than a modelled operation, so it answers in
/// its own small envelope instead of the service's restJson1 one. The message is built
/// from constants, never from the request.
fn document_error(status: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "message": message, "code": code }).to_string();
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut()
        .insert("content-type", crate::state::INQUIRY_EXPORT_CONTENT_TYPE);
    resp.headers_mut()
        .insert("x-amzn-errortype", code.to_string());
    resp
}



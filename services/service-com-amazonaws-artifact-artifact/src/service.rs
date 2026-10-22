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
//! gateway's concern: a `@httpQuery` value outside its modelled constraint is refused
//! by the service, as its 400 `ValidationException`, which is what the framework's own
//! boundary already answers — `override_refusal` below only restates the MESSAGE of
//! that refusal in this service's own wording.
//!
//! [`EmulatedService::front_door`] is check 1 and 2 for every request whose target or
//! body the gateway judges, reached through [`Service::override_operation`] because
//! both refusals precede the modelled walk: constraint validation runs before a
//! handler, so a handler cannot answer ahead of it. The one judgement the gateway
//! cannot make here is whether a well-formed identifier names a resource the account
//! holds — that is this service's state — so each report handler makes it, through
//! `crate::scaffolded::Scaffolded::unauthorized`, and answers the same 401.
//!
//! ## The document plane
//!
//! `ExportComplianceInquiry` hands out a PRESIGNED URL, and a presigned URL is a promise
//! that a caller can follow it: it is addressed to the document host this service
//! publishes (`crate::state::is_document_host`), under the path
//! `/compliance-inquiry/<inquiryId>/export/<exportId>/<object>`, which names no modelled
//! operation. Two seams carry it, the same pair any emulated data plane uses:
//!
//! - [`Service::can_handle`] also claims a request addressed to that published host,
//!   whose first label names no service, so the router does not answer `UnknownService`
//!   for a URL this service handed out;
//! - [`Service::override_routing_error`] serves it. A document request names no modelled
//!   operation by construction, so the router reaches exactly this seam for it, and
//!   reaches it BEFORE authentication — which is what lets a presigned URL be followed
//!   without a signature, as a presigned URL is.
//!
//! What it serves is the document the export STORED
//! (`crate::state::InquiryExport::document`), rendered from the inquiry when the export
//! ran: a download is a pure read, and a URL whose export has since been released, or
//! that names another inquiry or another object, is `404`. The report and term URLs
//! `GetReport` and `GetTermForReport` hand out are NOT served here: their documents are
//! AWS-published PDFs whose bytes this emulator does not hold, and synthesising them
//! would be inventing content, so those URLs stay locators.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{path, validate};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, Refusal, Refused,
    ServerHttpProtocol, Service,
};
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
        _req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
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

/// Run the front door's two checks over one operation's request.
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
fn judge<I>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    schema: &'static Schema,
    target: fn(&I) -> Option<Target<'_>>,
) -> Option<FrontDoorVerdict>
where
    I: DeserializableStruct + SerializableStruct,
{
    let mut deserializer = protocol.deserialize_input(req, schema).ok()?;
    let input = match I::deserialize(&mut *deserializer) {
        Ok(input) => input,
        // A member the shape requires, absent from the body, is a body the gateway
        // cannot bind either. Anything else the codec could not read (an absent URI
        // value, a body that is not the protocol's format at all) is left to the
        // framework's own refusal: the gateway's answer to it is not recorded.
        Err(aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => {
            return body_bound(schema, &member_name).then_some(FrontDoorVerdict::InvalidRequestBody)
        }
        Err(_) => return None,
    };

    // 1. Authorize the target, ahead of every constraint the model declares.
    if target(&input).is_some_and(|target| !target.names_a_resource()) {
        return Some(FrontDoorVerdict::Unauthorized);
    }

    // 2. Validate the body. A violation on a URI-bound member is NOT the gateway's:
    // the service refuses that one itself, as its modelled `ValidationException`.
    validate(&input, schema)
        .iter()
        .any(|violation| body_bound(schema, &violation.path))
        .then_some(FrontDoorVerdict::InvalidRequestBody)
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
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            unauthorized: FrontDoorAnswer(FrontDoorRefusal::unauthorized()),
            invalid_request_body: FrontDoorAnswer(FrontDoorRefusal::invalid_request_body()),
        }
    }

    /// The front door's judgement on one request, or `None` when it passes the request
    /// through to the modelled walk.
    ///
    /// One arm per operation the gateway judges: every operation whose input names a
    /// resource by identifier (checked first, so authorization precedes validation),
    /// and every operation that carries members in its request body. Each arm names
    /// its operation through that operation's own [`Operation::NAME`] and its input
    /// through the model-derived input schema, so nothing here is spelled out that the
    /// model already states.
    ///
    /// The three tag operations' `resourceArn` is deliberately not a [`Target`]: it is a
    /// `LongStringAttribute` the model gives no identifier syntax (any string without
    /// `<>` is admissible), so whether it names a resource at all is a question about
    /// this service's STATE — which tag set the ARN addresses — and each tag handler
    /// answers it with the same 401 through `crate::scaffolded::Scaffolded::unauthorized`.
    /// `TagResource` is here for its `tags` BODY alone.
    fn front_door(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<FrontDoorVerdict> {
        use crate::ops;

        let protocol = self.definition().protocol();
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

        // Every other operation carries neither a resource identifier nor a request
        // body, so the gateway has nothing to judge: the modelled walk answers it.
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
        // And a request addressed to the DOCUMENT host this service publishes its
        // presigned URLs under, whose first label names no service at all: without this
        // the router answers `UnknownService` for a URL this service handed out. See the
        // module docs.
        request_host(parts).is_some_and(crate::state::is_document_host)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // This service is reached through a gateway that answers before it (see the
        // module docs): a request whose target it cannot authorize, or whose body it
        // cannot bind, never reaches the modelled walk at all. Both answers are
        // pre-dispatch, which is what this hook is.
        //
        // The request's auth schemes are unaffected: the router reads them off the
        // MODELLED operation the routing key names, so a front-door refusal is still
        // only reached by a request whose signature verified.
        match self.front_door(req)? {
            FrontDoorVerdict::Unauthorized => Some(&self.unauthorized as &dyn ErasedOperation),
            FrontDoorVerdict::InvalidRequestBody => {
                Some(&self.invalid_request_body as &dyn ErasedOperation)
            }
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // What reaches here is a refusal of a value the request carried in its URI —
        // the gateway in front of this service validates the BODY (answered as
        // `BadRequestException` before dispatch, see `front_door`) and leaves
        // URI-bound values to the service, which refuses them as its modelled 400
        // `ValidationException` naming the member and the constraint. The STATUS and
        // the CODE of that refusal are already the framework's, and are what the model
        // declares (`ValidationException`, `@httpError(400)`); only the MESSAGE
        // differs, because this service states a constraint violation as the
        // validation envelope naming the member path and the requirement it failed
        // WITHOUT quoting the rejected value back at the caller. So the code is
        // restated unchanged alongside the reworded message: `Refused` carries both,
        // and a message without its matching code is worse than leaving both alone.
        //
        // An authentication refusal is left to the framework: it is not a
        // modelled-input failure and this service does not restate it.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        Some(Refused {
            code: "ValidationException".to_string(),
            message: crate::state::constraint_violation_message(violations)?,
        })
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
        // A request to a DOCUMENT URL this service handed out is exactly that case by
        // construction — the path is not in the model — so this is where an export's
        // presigned URL is served. Anything else keeps the framework's default.
        let target = crate::state::parse_export_document_path(request_path(req.uri()))?;
        Some(self.serve_export_document(req, &target))
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
    /// Serve one request to a published export-document URL.
    ///
    /// A download is a pure READ: it answers the bytes the export stored and writes
    /// nothing, so following a presigned URL twice answers the same document and changes
    /// nothing a later caller sees. The export, the inquiry it was taken from and the
    /// object name must all agree with the URL — an export this store no longer holds, or
    /// a path that crosses one export's id with another inquiry's id, names no document.
    fn serve_export_document(
        &self,
        req: &RtRequest,
        target: &crate::state::ExportDocumentTarget,
    ) -> RtResponse {
        if !matches!(req.method(), "GET" | "HEAD") {
            return document_error(405, "MethodNotAllowed", "This document is served by GET");
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return document_error(
                500,
                "InternalServerException",
                "the artifact compliance inquiry store is unavailable",
            );
        };
        let store = state.lock();
        let found = store.inquiry_exports.get(&target.export_id).filter(|export| {
            export.inquiry_id == target.inquiry_id
                && target.object == crate::state::INQUIRY_EXPORT_OBJECT
        });
        let Some(export) = found else {
            return document_error(404, "NoSuchKey", DOCUMENT_NOT_FOUND_MESSAGE);
        };

        let length = export.document.len();
        // A `HEAD` reports the document's metadata without its bytes, which is what the
        // method means; a `GET` carries them.
        let body = if req.method() == "HEAD" {
            SdkBody::empty()
        } else {
            SdkBody::from(export.document.clone())
        };
        let mut resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200 is a valid status"),
            body,
        );
        resp.headers_mut()
            .insert("content-type", crate::state::INQUIRY_EXPORT_CONTENT_TYPE);
        resp.headers_mut()
            .insert("content-length", length.to_string());
        resp
    }
}

/// The body of a document-plane refusal that found no document: one sentence carrying
/// nothing from the request, so a caller cannot probe for exports it was not handed.
const DOCUMENT_NOT_FOUND_MESSAGE: &str = "The requested document does not exist";

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

/// The `Host` header of a buffered request, or the URI authority when the request is
/// HTTP/2 (which carries `:authority` on the URI instead of a `Host` header).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

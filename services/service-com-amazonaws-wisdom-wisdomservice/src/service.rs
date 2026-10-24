// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-wisdom-wisdomservice` service: how it claims requests and how it words the
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
//! ## The content-upload plane
//!
//! `StartContentUpload` hands out a URL, and a URL is a promise that it can be
//! FOLLOWED. The URL it mints (`crate::state::content_upload_url`) addresses the
//! endpoint the caller reached, under the upload's own path
//! (`/knowledgeBases/<knowledgeBaseId>/uploads/<uploadId>`) — a path the model binds no
//! operation to, so it reaches this file twice:
//!
//! - [`Service::can_handle`] also claims a request to an upload path naming an upload
//!   THIS store holds, so following the URL routes back here even when nothing else in
//!   the request names a service (an upload URL is followed WITHOUT a signature, as a
//!   presigned URL is, and a loopback endpoint host names no service at all).//! - [`Service::override_routing_error`] serves it: that seam is reached BEFORE
//!   authentication precisely for requests that name no modelled operation, which is
//!   what makes an unsigned `PUT` to a minted URL work.
//!
//! The plane writes ONLY the upload the URL names — the bytes, bounded by
//! `crate::state::MAX_CONTENT_UPLOAD_BYTES` — and refuses a `PUT` that is late (the
//! URL's `urlExpiry` has passed) or that does not carry the headers the start call
//! published. Nothing else about the service is reachable through it.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service, ViolationKind};
use std::sync::Arc;

/// The error code this endpoint answers a value outside a modelled enumeration's value
/// set with: `BadRequestException`, HTTP 400.
///
/// It is NOT a modelled error — no operation in this model (nor in the Q Connect view of
/// the same endpoint) declares it (`smithy_query(query="operation",
/// operation="CreateAssistant")`: `AccessDeniedException`, `ConflictException`,
/// `ServiceQuotaExceededException`, `ValidationException`) — because the refusal happens
/// where the request body is read into the input shape, before the operation's own
/// validation: a `type` outside `AssistantType`'s value set, a `knowledgeBaseType` outside
/// `KnowledgeBaseType`'s, and so on are all answered with this one code, while a
/// constraint this service validates itself (an absent required member, a `@length` or
/// `@pattern` violation) keeps the modelled `ValidationException`.
///
/// The Q Connect view of this SAME endpoint — which is the view the shared `wisdom` key
/// is served from (`services/service-com-amazonaws-qconnect-wisdomservice/src/service.rs`)
/// — words this class identically, from its own copy of this constant and of
/// `override_refusal`: the two are one endpoint, so a client must not get a different code
/// for the same input depending on which key it addressed. Change one and change the other.
const ENUM_VALUE_REFUSAL_CODE: &str = "BadRequestException";

/// The key THIS service answers under, which is NOT the `wisdom` key its model's
/// `aws.auth#sigv4` name resolves to.
///
/// This model is Amazon Connect Wisdom: `smithy_query(query="service_info")` gives it
/// sdkId `Wisdom`, sigv4Name `wisdom`, arnNamespace `wisdom`, service shape
/// `WisdomService` version 2020-10-19, protocol `restJson1`, 41 operations. The repo
/// carries a SECOND model of that same endpoint,
/// `services/service-com-amazonaws-qconnect-wisdomservice` (sdkId `QConnect` — Amazon Q
/// in Connect, the endpoint's current API), whose `service_info` differs ONLY in sdkId:
/// the same service shape and version, the same protocol, the same signing name, the same
/// endpoint templates `https://wisdom.{Region}.{dnsSuffix}`, and 94 operations that
/// INCLUDE all 41 of this model's at the identical method+URI routes. restJson1 carries
/// no `X-Amz-Target`, so a request is dispatched by method+URI — and those routes are the
/// same here as there, which means no signal in a request can tell the two models apart.
/// The `wisdom` key therefore has to be served by exactly ONE of them, or the router
/// (which requires exactly one claimant) answers `500 AmbiguousService` for every request
/// to the endpoint, a code that exists in no AWS model.
///
/// The QConnect crate serves it, because it is the endpoint's current API and the
/// superset: a client signing `wisdom` reaches every operation EITHER model declares,
/// which is also what AWS does — one service answers both APIs on `wisdom.{region}`.
/// Claiming `wisdom` here as well would reinstate the ambiguity.
///
/// This service therefore answers under its own REGISTRATION identity: this model's shape
/// id `com.amazonaws.wisdom#WisdomService` in the key form the project names its service
/// crates by (`service-com-amazonaws-wisdom-wisdomservice`). It is unique per registered
/// service by construction and is no AWS signing name, endpoint prefix or sdkId — which
/// is exactly why it cannot collide with the shared endpoint identity. The legacy model
/// stays mounted, reachable and exercised by its own suite without ever contending for
/// `wisdom`.
///
/// Read this key as ADMINISTRATIVE, not as a wire identity: no AWS SDK or CLI ever signs
/// or addresses `com-amazonaws-wisdom-wisdomservice`, because AWS publishes exactly one
/// identity for this endpoint and the QConnect model above serves it. Only a caller that
/// names this registered model deliberately — this crate's own suite, or an operator
/// probing one mounted service — reaches these handlers, and nothing is lost by that: all
/// 41 routes of this model are bound in the QConnect crate, which answers the endpoint's
/// real traffic.
const OWN_REGISTRATION_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// Whether the request these `parts` came from resolves to [`OWN_REGISTRATION_KEY`] AND a
/// first URI path segment this model publishes (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule, and it is CALLED here rather than re-implemented, with this service's
/// own key in place of the signing name. It takes a whole `http::Request` while
/// [`Service::can_handle`] is handed only the request's parts, so the two things it reads
/// — the URI and the headers — are re-presented to it as a bodiless request. It reads
/// neither the body nor the method, so nothing it decides on is missing.
/// (`services/common`'s `claims_request` states the same rule over `Parts`; it is not
/// used here only because depending on it edits `Cargo.lock`, which this repository's
/// serialized squash merge refuses whenever another change lands against it — migrating
/// these two crates onto it is a one-line follow-up.)
fn claims_own_registration_key(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        OWN_REGISTRATION_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-wisdom-wisdomservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the content-upload plane is served from
    /// [`Service::can_handle`] and [`Service::override_routing_error`], neither of which
    /// is an operation with a `self.engine` of its own; both must read the one store
    /// `StartContentUpload` writes, not a second copy.
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

    /// Whether this request FOLLOWS an upload URL this service handed out: its path is
    /// one the upload plane publishes and it names an upload THIS store holds.
    ///
    /// The id in the path is the receipt: it was minted by this store, is unguessable,
    /// and expires. That is what makes the claim UNAMBIGUOUS — the Amazon Q in Connect
    /// view of this one endpoint mints its upload ids into its own store, so exactly the
    /// view that handed a URL out claims a request following it, whatever endpoint host
    /// or credential scope that request carries. Claiming every path of this shape
    /// instead would make both views claim it and the router refuse the request as
    /// `500 AmbiguousService`; the cost is that an upload URL this emulator never handed
    /// out is claimed by nobody and gets the router's own unknown-endpoint refusal.
    fn holds_content_upload(&self, target: &crate::state::ContentUploadTarget) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        store.content_upload(&target.upload_id).is_some()
    }

    /// Serve one request to a published upload URL.
    ///
    /// A `PUT` writes the asset into the upload the URL names — which is the only thing
    /// the plane can touch — and is refused when the URL is no longer usable, when the
    /// request does not carry the headers the start call published, or when the body is
    /// larger than this emulator stores. The upload, the knowledge base it was started
    /// under and the id in the path must all agree: a path that crosses one knowledge
    /// base with another's upload names nothing.
    fn serve_content_upload(
        &self,
        req: &RtRequest,
        target: &crate::state::ContentUploadTarget,
    ) -> RtResponse {
        if req.method() != "PUT" {
            return upload_error(
                405,
                "MethodNotAllowed",
                "This upload URL is written by a single PUT",
            );
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return upload_error(
                500,
                "InternalServerException",
                "the wisdom content-upload store is unavailable",
            );
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        let resolved_knowledge_base = store.resolve_knowledge_base_id(&target.knowledge_base_label);
        let names_this_upload = store
            .content_upload(&target.upload_id)
            .is_some_and(|upload| {
                resolved_knowledge_base.as_deref() == Some(upload.knowledge_base_id.as_str())
            });
        if !names_this_upload {
            return upload_error(404, "NoSuchUpload", UPLOAD_NOT_FOUND_MESSAGE);
        }
        // The URL stops being usable at its `urlExpiry`, so a late PUT is refused — and
        // the slot it named is reclaimed here, because the clock already owed it back.
        if store
            .content_upload(&target.upload_id)
            .is_some_and(|upload| upload.has_expired(now))
        {
            store.reclaim_expired_content_uploads(now);
            return upload_error(403, "AccessDenied", UPLOAD_EXPIRED_MESSAGE);
        }
        // `headersToInclude` is what the start call told the caller to send, so a write
        // that does not carry them is refused rather than stored.
        let headers_satisfied = store
            .content_upload(&target.upload_id)
            .is_some_and(|upload| {
                upload.headers_satisfied(|name| {
                    req.headers().get(name).map(::std::string::ToString::to_string)
                })
            });
        if !headers_satisfied {
            return upload_error(403, "AccessDenied", UPLOAD_HEADERS_MESSAGE);
        }
        let body = req.body().bytes().unwrap_or_default();
        if body.len() > crate::state::MAX_CONTENT_UPLOAD_BYTES {
            return upload_error(413, "EntityTooLarge", UPLOAD_TOO_LARGE_MESSAGE);
        }
        let Some(upload) = store.content_upload_mut(&target.upload_id) else {
            // Unreachable: the lookups above found it while this lock is held.
            return upload_error(404, "NoSuchUpload", UPLOAD_NOT_FOUND_MESSAGE);
        };
        upload.body = Some(body.to_vec());
        upload.received_at = Some(now);

        let mut resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200 is a valid status"),
            SdkBody::empty(),
        );
        resp.headers_mut().insert("content-length", "0");
        resp
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
        // Delegated to the framework's own predicate — see
        // [`claims_own_registration_key`]: this model is the Amazon Connect Wisdom view of
        // the endpoint Amazon Q in Connect now serves, publishing the very same wire
        // identity (signing name, endpoint, service shape, version, protocol) and a
        // strict subset of its routes, so the generated `wisdom` route key cannot tell
        // the two apart. It is claimed by the QConnect crate — the endpoint's current API
        // and the superset — and this service answers under its own registration key
        // instead, so exactly one service claims every request.
        // A request that FOLLOWS an upload URL this service handed out is claimed by
        // the store that minted it, whatever else the request carries: it has no
        // signature and its endpoint host may name no service at all, so the route key
        // cannot route it. The upload path binds no modelled operation, so nothing
        // modelled is diverted here. See [`Self::holds_content_upload`].
        if let Some(target) = crate::state::parse_content_upload_path(parts.uri.path()) {
            return self.holds_content_upload(&target);
        }
        claims_own_registration_key(parts)
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
        // required member, or a value outside a declared bound) to let this service
        // reword it; `None` keeps the framework's default (400 `ValidationException`,
        // which is also what this service's model declares for a constraint it
        // validates itself, so every other class is left alone).
        //
        // The ONE class this service states differently is a value outside a modelled
        // enumeration's value set: that is refused where the request body is read into
        // the operation's input shape, and this endpoint answers it 400
        // `BadRequestException` — a code neither this model nor the Q Connect view
        // declares as an operation error, because it is the deserialization boundary's,
        // not a modelled failure. Only the CODE is chosen here; the message stays the
        // framework's own `Value '…' at '…' failed to satisfy constraint: Member must
        // satisfy enum value set: […]` sentence, built from the request.
        //
        // `any`, not `all` or "the first": a request that ALSO breaks a constraint this
        // service validates itself could not be read into the input shape either, so the
        // boundary's refusal is the one the caller gets, and the message still states
        // every clause the walk found. That keeps the answer independent of the order the
        // members happen to be walked in.
        match refusal {
            Refusal::Invalid(violations)
                if violations
                    .iter()
                    .any(|violation| matches!(violation.kind, ViolationKind::EnumValue(_))) =>
            {
                Some(Refused {
                    code: ENUM_VALUE_REFUSAL_CODE.to_string(),
                    message: aws_smithy_schema_server::envelope(violations)
                        .unwrap_or_else(|| "invalid request".to_string()),
                })
            }
            _ => None,
        }
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
        // A request to an upload URL this service handed out is exactly that case by
        // construction — the model binds no operation to the upload path — so this is
        // where `StartContentUpload`'s URL is served. Anything else keeps the framework's
        // default.
        let target = crate::state::parse_content_upload_path(request_path(req.uri()))?;
        Some(self.serve_content_upload(req, &target))
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

/// The body of an upload-plane refusal that found no upload: one sentence carrying
/// nothing from the request, so a caller cannot probe for uploads it was not handed.
const UPLOAD_NOT_FOUND_MESSAGE: &str = "The requested upload does not exist";

/// The body of an upload-plane refusal of a write after the URL's `urlExpiry`.
const UPLOAD_EXPIRED_MESSAGE: &str = "Request has expired";

/// The body of an upload-plane refusal of a write that does not carry the headers the
/// start call published in `headersToInclude`.
const UPLOAD_HEADERS_MESSAGE: &str =
    "The request does not include the headers the upload was started with";

/// The body of an upload-plane refusal of a body larger than this emulator stores
/// (`crate::state::MAX_CONTENT_UPLOAD_BYTES`).
const UPLOAD_TOO_LARGE_MESSAGE: &str = "The upload is larger than this endpoint accepts";

/// One upload-plane error response: the status, the code in `x-amzn-errortype`, and a
/// JSON body carrying the reason.
///
/// The upload plane is plain HTTPS rather than a modelled operation, so it answers in
/// its own small envelope instead of the service's restJson1 one. The message is built
/// from constants, never from the request.
fn upload_error(status: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "message": message, "code": code }).to_string();
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "application/json");
    resp.headers_mut()
        .insert("x-amzn-errortype", code.to_string());
    resp
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            // Authority only, no path at all.
            None => "/",
        },
        None => without_query,
    }
}

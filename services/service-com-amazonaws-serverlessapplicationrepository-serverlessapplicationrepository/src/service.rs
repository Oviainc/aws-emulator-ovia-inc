// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository` service: how it claims requests and how it words the
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
//! ## The artifact document plane
//!
//! `Version$TemplateUrl` — and the `TemplateUrl` both CloudFormation template operations
//! report — is "A link to the template that can be used to deploy the application using
//! AWS CloudFormation", so it is a promise that a caller can FOLLOW it: fetching it and
//! deploying the bytes is what a CloudFormation client does with it. For a body a
//! publisher supplied INLINE this service therefore announces an address on the
//! emulator's own endpoint (`crate::state::minted_artifact_url`), under the path space
//! `crate::state::ARTIFACT_PATH_PREFIX`, which names no modelled operation. Two seams
//! carry it, the same pair any emulated data plane uses:
//!
//! - [`Service::can_handle`] also claims a request addressed to that published path,
//!   whatever routing signal it carries: a client following the URL may sign nothing at
//!   all, against an endpoint host that names no service, so the path itself is the claim
//!   — and without it the router answers `UnknownService` for a URL this service handed
//!   out;
//! - [`Service::override_routing_error`] serves it. An artifact request names no modelled
//!   operation by construction, so the router reaches exactly this seam for it, and
//!   reaches it BEFORE authentication — which is what lets a published artifact URL be
//!   followed with no `Authorization` header at all, as the real service's presigned object
//!   URL is: the authorisation such a fetch presents is the locator's own signature, which
//!   this seam checks.
//!
//! What it serves is the body the publish RETAINED (`Application::license_body` and
//! `readme_body`, `AppVersion::template_body`), read back out of the one store the
//! operations write: a download is a pure read, and a locator whose application has been
//! deleted, whose version was never published, or whose body the publisher named by a
//! `*Url` instead of supplying inline, is `404` — nothing is published there, and no
//! bytes are invented for it.
//!
//! A locator is PRESIGNED (`crate::state::presigned_artifact_url`), as the real service's
//! object URL is: the signature in its query string is what makes the document fetchable
//! by whoever holds the link and by nobody else, so this seam verifies that signature
//! before it reads the store — a locator presented with no signature, with a rewritten
//! one, or past the expiry it states is `403`, and only a locator this service signed is
//! served.
//!
//! ## How a response with no body is framed
//!
//! `DeleteApplication` answers `204` with an empty (`Unit`) output
//! (`smithy_query(query="operation", operation="DeleteApplication")`). A response with no
//! body still has a LENGTH, and AWS states it: that answer carries `content-length: 0`.
//! The protocol's serializer leaves the header off, which leaves the length to the
//! transport to frame, so a client or proxy that keys on a framing header being present
//! reads the emulator's reply differently from the service's.
//! [`Service::override_protocol`] therefore serves a protocol that is the model's own in
//! every respect except one: a response this service serializes with an EMPTY body says so
//! with `content-length: 0`. A response that carries a body is left exactly as the model's
//! protocol renders it, framed from the body it carries. This is the same seam, and the
//! same one-header correction, that the sibling `schemas` and `mediapackage` emulators
//! serve their own `Unit`-output operations through.
//!
//! That framing is what this service answers, and it is what the in-process front door
//! delivers (the inter-service callback path and the SDK connector both serve the router's
//! response unchanged). An answer served over HTTP/1.1 reaches a client WITHOUT it: the
//! HTTP writer the vendored transport uses removes `content-length` from a `204`, which
//! RFC 9110 §15.3.5 forbids there, and that writer is below anything this crate decides.
//! `tests/all/empty_response_framing.rs` pins what this service states, and
//! `tests/all/wire_probe.rs` pins what a socket then delivers.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because a published artifact is served from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own; it must read the one store the operations write, not a
    /// second copy.
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

/// How a response with no body is framed: with the `content-length: 0` that states its
/// length, rather than with no framing header at all. See the module docs.
///
/// Only an EMPTY body is touched, and only that one header: a response that carries a body
/// is framed from the body it carries, which is already right.
fn frame_empty_body(response: &mut RtResponse) {
    if response.body().bytes().is_some_and(<[u8]>::is_empty) {
        response.headers_mut().insert("content-length", "0");
    }
}

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing correction the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry.protocol().deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status, the body and every other header are the protocol's: only the framing
        // of an answer that has no body is added.
        frame_empty_body(&mut response);
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries the `errorCode`/`message`
        // envelope, so this body is never empty; the same framing is applied for the same
        // reason rather than leaving one of the two paths to answer differently.
        frame_empty_body(&mut response);
        Ok(response)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<RtResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names the
/// protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
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
        // A request addressed to an artifact URL this service published is this service's
        // WHATEVER routing key it carries, or none at all. The path space is this
        // service's own invention (`crate::state::ARTIFACT_PATH_PREFIX` names this
        // service and this kind of object), no other service binds it, and this is the
        // only claimant that can serve it — while the routing signals a client following
        // a published URL presents are not ours to choose: it may send no credential
        // scope at all (a published object URL is followed unsigned) against an endpoint
        // host that is an IP literal, `localhost`, or a deployment's own DNS name, or it
        // may re-use the same credential scope its control-plane calls carry. Claiming on
        // the path alone is what makes the announced address the served address under
        // every one of them, instead of `UnknownService` for a URL this service handed
        // out. See the module docs.
        //
        // `http::Uri::path` already strips the scheme, the authority and the query, for
        // an origin-form request line and an absolute-form one alike.
        if crate::state::addresses_artifact_plane(parts.uri.path()) {
            return true;
        }
        // Otherwise claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by the
        // protocol returned here. See the module docs: it is the model's own protocol, with
        // one correction — an answer with no body is framed with `content-length: 0`
        // instead of with no framing header at all.
        Some(self)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // This service words both framework refusals as the errors its own model
        // declares, so a client branches on the code it would see from AWS rather than
        // on the framework's generic `ValidationException`:
        //
        // * a member the model requires that is absent, or a value outside a declared
        //   bound (`MaxItems`' `range 1..100` on both list operations), is
        //   `BadRequestException` — "One of the parameters in the request is invalid"
        //   (HTTP 400, which the protocol keeps);
        // * an unverified caller is `ForbiddenException` — "The client is not
        //   authenticated" (HTTP 403).
        //
        // Both the code and the message are replaced together: a client branches on the
        // code, so a message without its matching code is worse than leaving both alone.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "BadRequestException".to_string(),
                message: violations
                    .iter()
                    .map(smithy_server_runtime::Violation::message)
                    .collect::<Vec<_>>()
                    .join(" "),
            }),
            Refusal::Unauthenticated => Some(Refused {
                code: "ForbiddenException".to_string(),
                message: "The client is not authenticated.".to_string(),
            }),
            // Non-exhaustive: a refusal class added later keeps the framework's wording
            // until this service words it.
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        //
        // A request to an artifact URL this service published is exactly that case by
        // construction — the path is not in the model — so this is where the retained
        // body is served. The whole published path space is answered here, including an
        // address in it that names no artefact this service mints: it is this service's
        // space, so an address in it is a document that does not exist rather than an
        // operation nothing implements. Anything else keeps the framework's default.
        let path = request_path(req.uri());
        if !crate::state::addresses_artifact_plane(path) {
            return None;
        }
        Some(self.serve_artifact(req, path))
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
    /// Serve one request to an artifact URL this service published.
    ///
    /// A fetch is a pure READ of the publish: it answers the bytes the publisher supplied
    /// inline and writes nothing, so following a locator twice answers the same document
    /// and changes nothing a later caller sees. The locator's own signature decides whether
    /// the download is authorised at all, and then every segment of it must agree with one
    /// stored record (`crate::state::ArtifactTarget`) — a locator whose application has
    /// been deleted, or whose version was never published, names no document.
    fn serve_artifact(&self, req: &RtRequest, path: &str) -> RtResponse {
        if !matches!(req.method(), "GET" | "HEAD") {
            return artifact_error(
                405,
                "MethodNotAllowed",
                "A published application artifact is served by GET.",
            );
        }
        // The locator this service announced carries the signature that authorises the
        // download, and it is checked BEFORE the store is read: a request that presents no
        // locator this service minted learns nothing about what is published, not even
        // whether anything is. Its scope is the region the address itself names, which is
        // the region of the catalogue it addresses.
        match crate::state::artifact_signature(
            request_authority(req),
            request_target(req.uri()),
            crate::state::artifact_path_region(path).unwrap_or(crate::state::DEFAULT_REGION),
            ::std::time::SystemTime::now(),
        ) {
            crate::state::ArtifactSignature::Honoured => {}
            // The wording each condition answers with is the one the real object store
            // gives for it, because this plane stands in for that store.
            crate::state::ArtifactSignature::Absent => {
                return artifact_error(403, "AccessDenied", "Access Denied")
            }
            crate::state::ArtifactSignature::Expired => {
                return artifact_error(403, "AccessDenied", "Request has expired")
            }
            crate::state::ArtifactSignature::Invalid => {
                return artifact_error(
                    403,
                    "SignatureDoesNotMatch",
                    "The request signature we calculated does not match the signature you \
                     provided. Check your key and signing method.",
                )
            }
        }
        // Which artefact the address names. A path under this space that names none — a
        // file name no publish mints, a segment count no locator carries — addresses no
        // document, which is the same absence as an artefact that was never published.
        let target = match crate::state::parse_artifact_path(path) {
            Some(target) => target,
            None => return artifact_not_found(),
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return artifact_error(
                500,
                crate::state::ErrorCode::InternalServerError.name(),
                "The serverless application catalogue is unavailable.",
            );
        };
        let store = state.lock();
        let Some(body) = store.artifact_body(&target) else {
            return artifact_not_found();
        };

        let length = body.len();
        let content_type = target.artifact.content_type(body);
        // A `HEAD` reports the document's metadata — its type and its length — without
        // the bytes, which is what the method means.
        let payload = if req.method() == "HEAD" {
            SdkBody::empty()
        } else {
            SdkBody::from(body.clone())
        };
        let mut resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200 is a valid status"),
            payload,
        );
        resp.headers_mut().insert("content-type", content_type);
        resp.headers_mut()
            .insert("content-length", length.to_string());
        resp
    }
}

/// The 404 the document plane answers for an address nothing is published at.
///
/// One sentence carrying nothing from the request, so a caller holding one locator cannot
/// probe the catalogue with it for artefacts it was not handed the URL of.
fn artifact_not_found() -> RtResponse {
    artifact_error(
        404,
        "NoSuchKey",
        "The requested application artifact does not exist.",
    )
}

/// One refusal from the artifact document plane: the status, the condition in
/// `x-amzn-errortype`, and a JSON body carrying it.
///
/// The document plane is plain HTTPS rather than a modelled operation — it is the address
/// a published artifact is fetched from — so it answers in its own small envelope instead
/// of this service's restJson1 one. Every message is a constant of this module, never
/// built from the request.
fn artifact_error(status: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "message": message, "errorCode": code }).to_string();
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut()
        .insert("content-type", "application/json");
    resp.headers_mut()
        .insert("x-amzn-errortype", code.to_string());
    resp
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn request_path(uri: &str) -> &str {
    request_target(uri).split('?').next().unwrap_or("/")
}

/// The request TARGET of a request URI: its path and its query, with any scheme and
/// authority removed — the two halves of the address a locator's signature covers.
fn request_target(uri: &str) -> &str {
    // The scheme is looked for only BEFORE the query: a query VALUE may carry `://`, which
    // a scan over the whole URI would mistake for the authority.
    let query_at = uri.find('?').unwrap_or(uri.len());
    let path_at = match uri[..query_at].find("://") {
        Some(scheme_end) => uri[scheme_end + 3..query_at]
            .find('/')
            .map_or(query_at, |slash| scheme_end + 3 + slash),
        None => 0,
    };
    &uri[path_at..]
}

/// The endpoint a request reached this emulator on: its `Host` header, or the authority an
/// HTTP/2 request carries on the URI instead of one. The host a locator was signed over, so
/// a locator is verified against the endpoint it is followed through.
fn request_authority(req: &RtRequest) -> &str {
    req.headers()
        .get(::http::header::HOST.as_str())
        .or_else(|| {
            let uri = req.uri();
            uri.split_once("://").map(|(_scheme, rest)| {
                match rest.find(['/', '?', '#']) {
                    Some(end) => &rest[..end],
                    None => rest,
                }
            })
        })
        .unwrap_or_default()
}

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-dataexchange-dataexchange` service: how it claims requests and how it words the
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
//! ## The upload plane
//!
//! `CreateJob` hands out an upload URL for a job that takes its content from one, and
//! such a URL is a promise that whoever holds it can WRITE there: it names the endpoint
//! the CALLER reached this emulator on, under the path space
//! `crate::state::parse_upload_path` owns, which names no modelled operation — the
//! modelled job routes stop one segment higher, at `/v1/jobs/{JobId}`. Two seams carry
//! it, the same pair any emulated data plane uses:
//!
//! - [`Service::can_handle`] also claims a request to that path space, so an upload
//!   that carries neither a credential nor a host naming a service — which is what a
//!   client following a presigned URL sends — is not answered `UnknownService`;
//! - [`Service::override_routing_error`] serves it. An upload names no modelled
//!   operation by construction, so the router reaches exactly this seam for it, and
//!   reaches it BEFORE authentication — which is what lets a presigned URL be followed
//!   without a signature, as a presigned URL is.
//!
//! What it does is store the bytes as the content of the job whose URL it is
//! (`crate::state::ComAmazonawsDataexchangeDataexchangeState::record_upload`), after
//! judging them: a path this service never minted names no upload, a window the create
//! call has already announced the end of is an expired request, and bytes that are not
//! the ones the job's declared MD5 describes are refused — HTTP's own `Content-MD5`, if
//! the client sent one, is checked against the same digest. The job's own execution
//! then reads those bytes (`crate::ops::start_job`).
//!
//! ## How an answer that carries no content is framed
//!
//! A sixth seam, `override_protocol`, is one this service DOES take, for one framing
//! decision no handler can make. Eight of this service's operations have an empty
//! (`smithy.api#Unit`) output and answer the no-content status: `CancelJob`, the five
//! deletes (`DeleteAsset`, `DeleteDataGrant`, `DeleteDataSet`, `DeleteEventAction`,
//! `DeleteRevision`) and the two tag mutations (`TagResource`, `UntagResource`). An answer
//! that carries no body still has a LENGTH, and AWS states it: such a response carries
//! `content-length: 0`. The protocol's serializer leaves that header off and the transport
//! does not frame a no-content status either, so a client, a proxy or a contract check that
//! keys on an explicit zero-length body reads the emulator's reply differently from the
//! service's.
//!
//! [`Service::override_protocol`] therefore serves a protocol that is the model's own in
//! every respect except one: a response at the no-content status carries no body and states
//! the length it has (`service_common::no_content_responses`, the SHARED reading of that
//! framing). Every response that carries content — every reader's, every create's and every
//! modelled refusal's — is left exactly as the model's protocol renders it, framed from the
//! body it carries. It is done at the one place every response of a modelled operation
//! passes through rather than per operation.
//!
//! That framing is what this service states, and it is what the in-process front door
//! delivers. An answer served over HTTP/1.1 reaches a client without it: the HTTP writer the
//! vendored transport uses removes `content-length` from a `204` (RFC 9110 §15.3.5 forbids a
//! body there), which is below anything this crate decides.
//! `tests/all/no_content_framing.rs` pins what this service states, and
//! `tests/all/no_content_wire_probe.rs` pins what a socket then delivers.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use aws_smithy_types::body::SdkBody;
use service_common::no_content_responses::without_content;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-dataexchange-dataexchange` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the upload plane is served from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own; it must write the one store the operations read, not a
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
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status and every other header are the protocol's: only an answer that carries
        // no content is emptied of whatever was rendered for it and states its length.
        Ok(without_content(response))
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries its error envelope at a 4xx or a
        // 5xx, so nothing here is emptied; the same call is made on both paths rather than
        // leaving one of them to answer differently.
        Ok(without_content(response))
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // An upload carries no credential and, when a client addresses the emulator
        // directly rather than by endpoint host, no host naming a service either, so
        // the path this service publishes uploads at is the only thing left to identify
        // it. Without this the router answers `UnknownService` for a URL this service
        // announced. Every method is claimed, not only the one that is served: a GET of
        // a path this service publishes is this service's to refuse as a method. See
        // the module docs.
        crate::state::parse_upload_path(parts.uri.path()).is_some()
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
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by the
        // protocol returned here. See the module docs: it is the model's own protocol, with
        // one correction — an answer that carries no content states `content-length: 0`
        // instead of leaving the framing header off.
        Some(self)
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
        // construction — the path is not in the model — so this is where such a URL is
        // served. Anything else keeps the framework's default.
        let target = crate::state::parse_upload_path(::service_common::request_path(req.uri()))?;
        Some(self.serve_upload(req, &target))
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
    /// Serve one request to a published upload URL.
    ///
    /// An upload is a WRITE — that is what the URL was handed out for — so it stores
    /// the bytes as the content of the job whose URL it is, and a second upload to the
    /// same URL replaces the first: a PUT is idempotent.
    ///
    /// The upload the URL addresses is what decides it, in this order: the method, so a
    /// path this service publishes states which methods it serves rather than appearing
    /// absent; the integrity value HTTP itself defines over the body, when the client
    /// sent one; and then the store, which judges the upload against the job that
    /// published it (see
    /// [`crate::state::ComAmazonawsDataexchangeDataexchangeState::record_upload`]).
    fn serve_upload(&self, req: &RtRequest, target: &crate::state::UploadTarget) -> RtResponse {
        if req.method() != UPLOAD_METHOD {
            return method_not_allowed();
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return upload_error(
                500,
                "InternalServerException",
                "the dataexchange store is unavailable",
            );
        };
        let body = req.body().bytes().unwrap_or_default();
        if body.len() > crate::state::MAX_UPLOAD_BYTES {
            return upload_error(413, "RequestEntityTooLarge", TOO_LARGE_MESSAGE);
        }
        let digest = crate::state::content_md5(body);
        if let Some(declared) = req.headers().get(CONTENT_MD5_HEADER) {
            // A value that is not a Base64 128-bit digest at all cannot be the digest
            // of anything, so it is refused before any comparison; one that is a digest
            // but not this body's is the mismatch. The comparison is between digests,
            // not between texts, so an equal digest written with different padding is
            // still equal.
            match ::aws_smithy_types::base64::decode(declared.trim()) {
                Ok(decoded) if decoded.len() == MD5_DIGEST_BYTES => {
                    if ::aws_smithy_types::base64::encode(&decoded) != digest {
                        return upload_error(400, "BadDigest", BAD_DIGEST_MESSAGE);
                    }
                }
                _ => return upload_error(400, "InvalidDigest", INVALID_DIGEST_MESSAGE),
            }
        }

        let mut store = state.lock();
        match store.record_upload(target, &digest, body.to_vec(), crate::state::now()) {
            crate::state::UploadOutcome::Stored => stored_response(body),
            crate::state::UploadOutcome::NoSuchUpload => {
                upload_error(404, "NotFound", NO_SUCH_UPLOAD_MESSAGE)
            }
            crate::state::UploadOutcome::Expired => {
                upload_error(403, "AccessDenied", EXPIRED_MESSAGE)
            }
            crate::state::UploadOutcome::DigestMismatch => {
                upload_error(400, "BadDigest", BAD_DIGEST_MESSAGE)
            }
            crate::state::UploadOutcome::StoreFull => {
                upload_error(507, "InsufficientStorage", STORE_FULL_MESSAGE)
            }
        }
    }
}

/// The one method an upload URL serves: a presigned upload target takes the bytes by
/// `PUT`, which is also the method the create call's own documentation sends a client
/// to it with.
const UPLOAD_METHOD: &str = "PUT";

/// The integrity header HTTP itself defines over a request body, which a client
/// uploading to a presigned target sends the content's Base64 MD5 in.
const CONTENT_MD5_HEADER: &str = "content-md5";

/// How many bytes an MD5 digest is, so a `Content-MD5` of another width is refused as
/// not being a digest rather than compared.
const MD5_DIGEST_BYTES: usize = 16;

/// The sentence for a `Content-MD5` that is not a Base64 128-bit digest at all.
const INVALID_DIGEST_MESSAGE: &str = "The Content-MD5 or checksum value that you specified is not valid.";

/// The sentence for bytes that are not the ones the declared digest describes —
/// whether the digest came in the request's own `Content-MD5` or was the one the job's
/// creation declared, because both are statements about these bytes.
const BAD_DIGEST_MESSAGE: &str = "The Content-MD5 you specified did not match what we received.";

/// The sentence for an upload to a URL whose announced window has closed.
const EXPIRED_MESSAGE: &str = "Request has expired";

/// The sentence for a path that is not an upload URL this service handed out.
const NO_SUCH_UPLOAD_MESSAGE: &str = "The requested path is not an upload URL this service published";

/// The sentence for a body larger than one upload retains
/// ([`crate::state::MAX_UPLOAD_BYTES`]).
const TOO_LARGE_MESSAGE: &str = "The uploaded content is larger than this emulator retains";

/// The sentence for a store already holding as much content as it retains
/// ([`crate::state::MAX_CONTENT_BYTES`]).
const STORE_FULL_MESSAGE: &str = "This emulator is already retaining as many content bytes as it holds";

/// The refusal the upload plane answers a method it does not serve with. RFC 9110
/// requires a `405` to state which methods the target does support, so the refusal
/// names it in the `Allow` header as well as in its message.
fn method_not_allowed() -> RtResponse {
    let mut response = upload_error(
        405,
        "MethodNotAllowed",
        "An upload URL is served by PUT",
    );
    response.headers_mut().insert("allow", UPLOAD_METHOD);
    response
}

/// The answer to an upload that was stored: the status a presigned upload target
/// answers, with the stored content's own entity tag and no body.
fn stored_response(content: &[u8]) -> RtResponse {
    let mut response = RtResponse::new(status(200), SdkBody::empty());
    response
        .headers_mut()
        .insert("etag", crate::state::content_etag(content));
    response.headers_mut().insert("content-length", "0");
    response
}

/// One upload-plane error response: the status, the code in `x-amzn-errortype`, and a
/// JSON body carrying the reason.
///
/// The upload plane is plain HTTPS rather than a modelled operation, so it answers in
/// its own small envelope instead of this service's restJson1 one. The message is built
/// from constants, never from the request.
fn upload_error(status_code: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "message": message, "code": code }).to_string();
    let mut response = RtResponse::new(status(status_code), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", "application/json");
    response
        .headers_mut()
        .insert("x-amzn-errortype", code.to_string());
    response
}

/// A status this plane answers with. Every call site passes a constant of this module,
/// so the conversion cannot fail.
fn status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("the upload plane's statuses are valid constants")
}

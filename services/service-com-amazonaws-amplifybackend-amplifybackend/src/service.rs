// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-amplifybackend-amplifybackend` service: how it claims requests and how it words the
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
//! Six opt-in seams are yours to change; every one defaults to the framework's
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
//! - `override_protocol` — the codec the dispatch sequence reads and answers one
//!   request with. THIS service overrides it: the generated `restJson1` protocol
//!   wrapped in AmplifyBackend's own error envelope (see
//!   [`AmplifyBackendProtocol`]), which is where the `code` member, the
//!   charset-qualified media type and the `ETag` on an error response come from.
//!   Everything a request reads and every success it writes is the generated
//!   protocol's, unchanged.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use sha2::{Digest, Sha256};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// The media type every AmplifyBackend response declares.
///
/// Recorded behaviour, not modelled: the service states the JSON media type WITH its
/// charset parameter, which is what a client matching on the exact header value sees.
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// The `code` member every AmplifyBackend error body carries beside `message`.
///
/// Recorded behaviour, not modelled: this service's error shapes declare only `Message`
/// (and `ResourceType` on `NotFoundException`), yet every recorded error body carries a
/// `code` naming the failure — the only machine-readable discriminator the service gives
/// a client that reads the payload. The value written is the concrete error's wire code,
/// the same string the restJson1 envelope puts in `__type` and `x-amzn-errortype`, so the
/// three cannot disagree.
///
/// Member schema rather than a hand-spliced JSON string so the value goes through the
/// same codec as every other member (see `__type` in `aws_smithy_schema_server`'s
/// restJson1 error envelope, which is declared the same way).
///
/// Declared in the `smithy.framework` namespace, not this service's, for the reason
/// `crate::scaffolded`'s synthetic shapes are: the member is NOT in the model, so it must
/// not collide with — or shadow — a modelled shape this service may later declare.
static ERROR_CODE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#ErrorEnvelope$code",
        "smithy.framework",
        "code",
    ),
    ShapeType::String,
    "code",
    0,
)
.with_json_name("code");

/// The `com-amazonaws-amplifybackend-amplifybackend` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated, model-derived half. Shared with [`Self::protocol`], which wraps the
    /// protocol this entry carries, so the entry is held behind an `Arc` rather than
    /// copied: there is exactly one routing table and one codec per mounted service.
    entry: Arc<crate::ServiceDefinition<Eng>>,
    /// This service's wire protocol: the generated one, with this service's own error
    /// envelope around it. Built once here, never per request, because
    /// [`Service::override_protocol`] hands back a borrow of it.
    protocol: AmplifyBackendProtocol<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let entry = Arc::new(crate::ServiceDefinition::new(engine));
        Self {
            protocol: AmplifyBackendProtocol {
                entry: Arc::clone(&entry),
            },
            entry,
        }
    }
}

/// This service's `restJson1` codec: the generated one for everything a request reads or
/// a success writes, plus AmplifyBackend's own ERROR envelope.
///
/// AmplifyBackend answers an error with more than the modelled shape: the body carries a
/// `code` beside `message`, the response declares `application/json; charset=utf-8`, and
/// it carries an `ETag` validator for the body. None of that is in the model — the model
/// states only the error shapes' members — so it cannot come from the generated codec,
/// and it is a SERVICE-wide property of the error channel rather than something each
/// handler could restate. Wrapping the one protocol the dispatch sequence uses is
/// therefore where it belongs: every modelled error of every operation leaves through
/// [`ServerProtocol::serialize_error`] here, so no operation can answer in a different
/// envelope.
///
/// Everything else — `routing_key`, `deserialize_input`, `serialize_output` and the
/// event-stream path — is the generated protocol's, untouched.
struct AmplifyBackendProtocol<Eng: Send + Sync + 'static> {
    entry: Arc<crate::ServiceDefinition<Eng>>,
}

impl<Eng: Send + Sync + 'static> AmplifyBackendProtocol<Eng> {
    /// The generated, model-derived protocol this one wraps.
    fn inner(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }
}

impl<Eng: Send + Sync + 'static> std::fmt::Debug for AmplifyBackendProtocol<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmplifyBackendProtocol")
            .field("protocol_id", self.inner().protocol_id())
            .finish()
    }
}

/// An error payload with this service's `code` member written in front of the modelled
/// members.
///
/// The `code` is written FIRST and the modelled members then flow through unchanged, so a
/// shape that happens to declare its own `code` one day would overwrite this one rather
/// than be shadowed by it.
struct WithErrorCode<'a> {
    code: &'a str,
    modelled: &'a dyn SerializableStruct,
}

impl<'a> SerializableStruct for WithErrorCode<'a> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        serializer.write_string(&ERROR_CODE_MEMBER, self.code)?;
        self.modelled.serialize_members(serializer)
    }
}

/// The `ETag` validator this service puts on an error response, derived from the response
/// BYTES.
///
/// Recorded behaviour is that the header is PRESENT; its recorded value is
/// environment-specific, so what matters is that it is a function of the body alone — the
/// same body always yields the same tag and any different body a different one, which is
/// the whole contract of an entity tag. It is a WEAK validator (`W/`), as the recorded
/// one is: it identifies the payload, not a byte-exact representation.
///
/// The length prefix is the body's length in hex, and the digest is a truncated SHA-256
/// of the body — 128 bits, so two distinct error bodies cannot be given one tag. The
/// recorded tag's length radix and digest construction are not constrained by anything
/// the recording states (its value is environment-specific), so both are this emulator's
/// choice; what the recording fixes is that the header is there and that its value is a
/// function of the body.
///
/// The envelope is applied on the ERROR path only: every recorded cluster is an error
/// response, and the corpus says nothing about a success's media type or validator, so a
/// 200 keeps the generated protocol's headers.
fn entity_tag(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    format!("W/\"{:x}-{}\"", body.len(), hex::encode(&digest[..16]))
}

impl<Eng: Send + Sync + 'static> ServerProtocol for AmplifyBackendProtocol<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &ShapeId {
        self.inner().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.inner().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.inner().deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        self.inner()
            .serialize_output(output, output_schema, operation_name, success_status)
    }

    /// The modelled error, in this service's own envelope: the `code` member beside
    /// `message`, the charset-qualified media type, and an `ETag` over the body.
    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self.inner().serialize_error(
            &WithErrorCode {
                code: error_code,
                modelled: error,
            },
            error_schema,
            error_code,
            http_status,
        )?;
        response.headers_mut().insert("content-type", JSON_CONTENT_TYPE);
        // Derived from the serialized body, so it is set only when that body is in
        // memory — which it always is here, the codec having just produced the bytes. A
        // header the HTTP layer rejected is OMITTED rather than failing the response: on
        // the error path, replacing a modelled failure with a transport failure would
        // lose the error the caller has to read. (`try_insert` for the same reason as
        // `insert_error_response_headers` in the framework's error path.)
        if let Some(body) = response.body().bytes() {
            let tag = entity_tag(body);
            let _ = response.headers_mut().try_insert("etag", tag);
        }
        Ok(response)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<RtResponse, ProtocolError> {
        self.inner()
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &*self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
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

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        match refusal {
            // AmplifyBackend declares ONE client-input error on every operation:
            // `BadRequestException` (400), documented as "An error returned if a
            // request is not formed properly". A request the framework refused before
            // any handler ran — an absent required member, or a value outside a
            // declared bound — is exactly that, so this service states it in its own
            // modelled shape instead of the framework's generic `ValidationException`.
            // Both the code and the message are replaced together; the status stays
            // with the protocol, which already answers 400 for this refusal.
            Refusal::Invalid(violations) => Some(Refused {
                code: "BadRequestException".to_string(),
                message: aws_smithy_schema_server::constraints::envelope(violations)
                    .unwrap_or_else(|| "the request is not formed properly".to_string()),
            }),
            // Everything else keeps this crate's default wording: an unauthenticated
            // caller is the emulator's refusal, not this service's. `Refusal` is
            // `#[non_exhaustive]`, so the catch-all is what covers it.
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
        let _ = req;
        None
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

    fn override_protocol(&self) -> Option<&dyn ServerHttpProtocol> {
        // The generated `restJson1` codec, wrapped in this service's own ERROR
        // envelope: AmplifyBackend answers an error with a `code` member beside
        // `message`, `content-type: application/json; charset=utf-8` and an `ETag` over
        // the body, none of which the model states. Held in a field (built once in
        // `new`), as this seam requires — never constructed per request. Everything a
        // request READS and every success it writes is the generated protocol's,
        // unchanged.
        Some(&self.protocol)
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-iotdataplane-iotmoonrakerservice` service: how it claims requests and how it words the
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
//! ## The media type a shadow document is announced with
//!
//! The data plane answers a request whose response body is a shadow state document
//! with `content-type: application/json`: the body IS a JSON document, and a client
//! or proxy decides whether to decode it from that header. On the wire that document
//! is the `JsonDocument` blob these operations bind as their `@httpPayload`
//! (`smithy_query(query="shape", shape="GetThingShadowResponse,UpdateThingShadowResponse,DeleteThingShadowResponse,JsonDocument")`),
//! and restJson1's generic default for a bound blob — `application/octet-stream` —
//! describes opaque bytes, not a JSON document. `override_protocol` below therefore
//! serves a protocol that is the model's restJson1 in every respect except one: a
//! response whose `@httpPayload` is this service's `JsonDocument` shape is announced
//! as `application/json`. Every other response, including the blob members
//! `GetRetainedMessage` carries in its JSON body, keeps exactly what the model's
//! protocol produces.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};

/// The media type the data plane announces for a shadow state document.
const JSON_DOCUMENT_MEDIA_TYPE: &str = "application/json";

/// The error code this service refuses a bad request with, which is the error its model
/// binds to every operation for one — see [`EmulatedService::override_refusal`]. Named
/// here as the wire code because a boundary refusal is rendered before any operation's
/// error enum is reached.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// Every output shape of this service whose `@httpPayload` member targets the
/// `JsonDocument` shape.
///
/// Source: `smithy_query(query="shape", shape="GetThingShadowResponse,
/// UpdateThingShadowResponse,DeleteThingShadowResponse,JsonDocument")` — each
/// declares the single member `payload` with `target: JsonDocument`,
/// `httpPayload: true`, `type: blob`, and `JsonDocument` is a `blob`. No other
/// operation of this service binds a `@httpPayload` in its output
/// (`smithy_query(query="digest")` lists all 11), so this is the complete set. The
/// set is recorded as output shapes because a runtime `Schema` member carries its
/// target's TYPE but not its target's shape id, so `JsonDocument` cannot be told
/// from any other blob at serialize time.
const JSON_DOCUMENT_PAYLOAD_OUTPUTS: &[&str] = &[
    "DeleteThingShadowResponse",
    "GetThingShadowResponse",
    "UpdateThingShadowResponse",
];

/// True when this output shape's response body is a `JsonDocument` — a
/// `@httpPayload` blob member on one of the shapes [`JSON_DOCUMENT_PAYLOAD_OUTPUTS`]
/// names. Both halves are required: the shape must be one the model gives a
/// `JsonDocument` payload, and the schema it is serialized against must still bind
/// that payload as a blob.
fn carries_json_document(output_schema: &Schema) -> bool {
    let id = output_schema.shape_id();
    if id.namespace() != "com.amazonaws.iotdataplane"
        || !JSON_DOCUMENT_PAYLOAD_OUTPUTS.contains(&id.shape_name())
    {
        return false;
    }
    output_schema
        .members()
        .iter()
        .any(|member| member.http_payload().is_some() && member.shape_type() == ShapeType::Blob)
}

/// The `com-amazonaws-iotdataplane-iotmoonrakerservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The protocol this service answers modeled requests in: the model's restJson1,
/// with the one media-type correction the module docs state.
///
/// It is implemented on the service itself because the model-derived protocol it
/// delegates to is owned by the generated entry and reached by reference
/// (`self.entry.protocol()`), and [`Service::override_protocol`] hands back a borrow
/// of `&self` — so there is one protocol instance, the generated one, and no second
/// route table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a WireRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a WireRequest,
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
    ) -> Result<WireResponse, ProtocolError> {
        let mut response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The body the protocol just wrote is the `JsonDocument` payload verbatim, so
        // only the media type announcing it changes — never the bytes, the status or
        // any other header.
        if carries_json_document(output_schema) {
            response
                .headers_mut()
                .insert("content-type", JSON_DOCUMENT_MEDIA_TYPE);
        }
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        // A modelled failure carries no `@httpPayload`: it is the protocol's JSON
        // error envelope, which already announces `application/json`.
        self.entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this
/// names the protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .field("json_document_payloads", &JSON_DOCUMENT_PAYLOAD_OUTPUTS)
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
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // This service's client-input error is `InvalidRequestException` — one of the
        // three errors its model binds to EVERY operation
        // (`smithy_query(query="digest")`: `common_errors` is InternalFailureException /
        // InvalidRequestException / ThrottlingException). `ValidationException`, which
        // the framework names by default on the JSON protocols, is not a shape this
        // service models at all, so a client's modelled-error dispatch cannot match it
        // and the refusal surfaces as an unknown failure. Every refusal the framework
        // describes structurally therefore carries this service's code — whichever
        // operation and whichever member it was, since the model binds the error to all
        // of them — and the status stays the 400 the protocol renders a constraint
        // refusal with.
        //
        // The MESSAGE is this service's too, and it is the one terse sentence this data
        // plane answers every front-door refusal with (`crate::state::BAD_REQUEST`) —
        // whichever member of whichever operation was at fault, and whether the value was
        // outside a bound or the member was absent. It carries NOTHING of the request:
        // not the member's name, not the bound it crossed, and not the value the caller
        // sent. The framework's structural envelope, which states all three, describes a
        // constraint walk rather than this service's contract, and a client matching the
        // refusal would not recognise it.
        //
        // The conditions a HANDLER reports are a different thing and keep their own
        // specific text (`crate::state` holds that catalog): they are the service's own
        // rules, not the front door's.
        //
        // An unauthenticated caller is not a constraint refusal and keeps the
        // framework's 403 wording: this service models `ForbiddenException` with that
        // status for exactly that case.
        match refusal {
            Refusal::Invalid(_) => Some(Refused {
                code: INVALID_REQUEST_EXCEPTION.to_string(),
                message: crate::state::BAD_REQUEST.to_string(),
            }),
            _ => None,
        }
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modeled operation of this service produces is serialized
        // by the protocol returned here, so this is the one place the media type it
        // announces is decided. See the module docs: a shadow state document — the
        // `JsonDocument` blob these operations bind as their `@httpPayload` — is
        // announced `application/json`, and every other response is left exactly as
        // the model's restJson1 protocol renders it.
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
}

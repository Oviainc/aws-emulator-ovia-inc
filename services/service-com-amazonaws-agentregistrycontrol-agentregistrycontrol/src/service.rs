// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-agentregistrycontrol-agentregistrycontrol` service: how it claims requests and how it words the
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

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use service_common::no_content_responses::without_content;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The host label this service's endpoint is addressed by, from its model's
/// `aws.api#service` `endpointPrefix`. It differs from the SigV4 signing name this
/// service SHARES with the Agent Registry data plane, and is the second key a request
/// that carries no credential scope resolves to. See [`EmulatedService::can_handle`].
const ENDPOINT_PREFIX: &str = "agent-registry-control";

/// The `com-amazonaws-agentregistrycontrol-agentregistrycontrol` emulated service: the generated [`ServiceDefinition`] plus
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

/// The protocol this service answers modelled requests in: the model's own `restJson1`,
/// with the one framing correction `service-common`'s no-content module states — an answer
/// at HTTP 204 carries no body, where the serializer renders the empty JSON object for an
/// output structure that declares no member.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
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
        let response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status and every other header are the protocol's: only an answer that carries
        // no content is emptied of the body rendered for it.
        Ok(without_content(response))
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries its error envelope at a 4xx or
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
    ) -> Result<WireResponse, ProtocolError> {
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by the
        // protocol returned here: the model's own, with the one framing correction below —
        // an answer at a no-content status carries no body. `TagResource` and
        // `UntagResource` both declare `http { code: 204 }` over an output structure with
        // no members, which the serializer would otherwise render as the empty JSON object.
        Some(self)
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `agent-registry` — a signing name it SHARES with the
        // Agent Registry DATA plane
        // (`services/service-com-amazonaws-agentregistry-agentregistry`), whose endpoint
        // is `agent-registry` while this one's is `agent-registry-control`. The router
        // resolves a request's routing key from its SigV4 credential scope first, so BOTH
        // services see the key `agent-registry`: claiming on the signing name alone makes
        // each claim the other's traffic, and the router — which refuses rather than
        // guesses — answers `500 AmbiguousService` for a request neither of them, or both
        // of them, can be told apart on, a code no AWS model carries.
        //
        // Nor can a path segment separate them: both models publish `registries` as a
        // first segment (`POST /registries` and `GET /registries/{registryId}` here,
        // `POST /registries/{registryId}/discoverable-records-list` there).
        //
        // What separates them is what AWS itself dispatches on: the endpoint host, then
        // the `restJson1` route. So this service claims a request whose routing key names
        // THIS endpoint — its signing name, or the `endpointPrefix` a host-addressed,
        // unsigned request resolves to — and whose method + URI its OWN model binds a
        // route for, by the one rule `service-common` states for every service under a
        // shared signing name. The two models bind no identical method + URI pattern, so
        // exactly one of them claims any Agent Registry request.
        service_common::claims_modeled_route(
            parts,
            &[self.entry.identity().signing_name(), ENDPOINT_PREFIX],
            self.definition(),
        )
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
        // reword it. What this service states, and all it states, is in
        // [`stated_refusal`]: the envelope, the error count, the member paths and every
        // requirement sentence stay the framework's, and an authentication refusal keeps
        // its own code and wording.
        stated_refusal(refusal)
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

/// How this service states the framework's constraint refusal, or `None` to keep the
/// framework's own wording for anything that is not one.
///
/// Two renderings, and nothing else:
///
/// * NO clause quotes the value the request submitted. Every one names the member it was
///   stated at and the requirement that member's value had to satisfy, whatever the
///   request carried — which is also what keeps a `@sensitive` member (`description`,
///   `customMetadataSchemaConfiguration`) out of the refusal it provoked.
/// * a `@pattern` is quoted as this service advertises the expression: without the pair of
///   whole-value anchors the model wraps it in, so a caller that re-applies the expression
///   it was handed back uses the same one. The CHECK still runs the model's pattern as
///   written; only the rendering drops the pair.
///
/// The envelope, its error count, the clause, the member paths, the grouping and every
/// requirement sentence belong to the shared home for this wording and to the framework
/// behind it — none of them is spelled here, so this service cannot drift from the sibling
/// Agent Registry service, which states the same sentences. An authentication refusal is a
/// different failure with its own code and status and nothing about it is this service's
/// to word.
fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    service_common::validation_message::aws_validation_refusal_withholding_violations(
        refusal,
        // Every clause, whatever member or constraint class it is about.
        |_| true,
    )
}

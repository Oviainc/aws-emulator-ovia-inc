// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-notifications-notifications` service: how it claims requests and how it words the
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
//!   before any handler ran. EDITED here: a numeric-bound (`@range`) and a `@pattern`
//!   violation are stated against the member and the requirement alone, without quoting
//!   the submitted value back, and a `@pattern` expression is quoted without the anchors
//!   the model wraps it in; a MAP member over its `@length` maximum is stated as the
//!   schema layer's property-count finding; every other class keeps the framework's
//!   wording. See [`clause`].
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
//! ## How an answer that carries no content is framed
//!
//! This service's two tag mutations answer `204 No Content` (see `SUCCESS_STATUS` in
//! `src/ops/tag_resource.rs` and `src/ops/untag_resource.rs`). Both declare an output
//! STRUCTURE with no members rather than `smithy.api#Unit`
//! (`smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse")`), and
//! the protocol's serializer renders such a structure as the empty JSON object `{}` — a
//! body on a response that has none, which RFC 9110 §15.3.5 gives a 204 no room for.
//!
//! [`Service::override_protocol`] below therefore serves a protocol that is the model's own
//! in every respect except one: a `204` this service answers carries no body, whatever the
//! serializer rendered for it, and so no header stating a length it does not have — which
//! is also what that same section forbids on a 204. A response that carries content is left
//! exactly as the model's protocol renders it. It is done here, at the one place every
//! response of this service passes through, rather than per operation.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};

/// The error code this service answers an input-constraint violation with: the
/// modelled `ValidationException` (`@httpError 400`,
/// `com.amazonaws.notifications#ValidationException`, bound to every operation as a
/// service-common error), which is also the code the framework's own refusal renderer
/// picks. Restated here because `override_refusal` replaces the code and the message
/// together, and a message without its matching code is worse than leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The clause this service states for one violated constraint.
///
/// A NUMERIC BOUND (`@range`) violation names the member and the bound it broke, and a
/// `@pattern` violation names the member and the expression a value has to satisfy;
/// NEITHER quotes the value submitted there. The recorded refusal of a `maxResults`
/// outside its range reads `Value at '<member>' failed to satisfy constraint: Member must
/// have value less than or equal to <max>`, and the recorded refusal of a patterned
/// member reads `Value at '<member>' failed to satisfy constraint: Member must satisfy
/// regular expression pattern: <expression>`.
///
/// The EXPRESSION is quoted without the whole-value anchors the model wraps it in, which
/// is `service_common::validation::unanchored` — the one implementation of that rule,
/// called rather than inlined again. The CHECK still runs the model's pattern as
/// written; only the rendering drops the pair.
///
/// Every other violation class keeps the framework's own clause — which quotes the
/// submitted value, the form `services/common/src/validation.rs` states as the
/// cross-service default — so this service rewords only the classes its recordings pin.
///
/// (The subject half of a validation message is one of the two parts
/// `services/common/src/validation.rs` lists as still to migrate into it; until that
/// lands it is spelled per service, as in the other service crates that word their own
/// refusals.)
fn clause(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::MinValue(min) => format!(
            "Value at '{}' failed to satisfy constraint: Member must have value greater than \
             or equal to {min}",
            violation.path
        ),
        ViolationKind::MaxValue(max) => format!(
            "Value at '{}' failed to satisfy constraint: Member must have value less than or \
             equal to {max}",
            violation.path
        ),
        ViolationKind::Pattern(pattern) => format!(
            "Value at '{}' failed to satisfy constraint: Member must satisfy regular expression \
             pattern: {}",
            violation.path,
            service_common::validation::unanchored(pattern)
        ),
        _ => violation.message(),
    }
}

/// True for the violation classes this service words differently from the framework.
fn reworded(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_) | ViolationKind::Pattern(_)
    )
}

/// How this service states a refusal of the values a request carried, or `None` to keep
/// the framework's wording when no clause of it is one this service rewords.
///
/// The envelope is the framework's own (`N validation error(s) detected: …`, clauses
/// joined with `; ` in the order the validating walk found them), so a refusal mixing a
/// range violation with another class reads as one message.
fn invalid_refusal(violations: &[Violation]) -> Option<Refused> {
    if !violations.iter().any(reworded) {
        return None;
    }

    let clauses: Vec<String> = violations.iter().map(clause).collect();
    let message = match clauses.len() {
        0 => return None,
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message,
    })
}

/// The status whose answer carries NO content, whatever a renderer wrote for it. See the
/// module docs.
const NO_CONTENT_STATUS: u16 = 204;

/// How an answer that carries no content is framed: with no body at all, and with no
/// framing header stating a length it does not have. See the module docs.
///
/// A response that carries content is untouched: it keeps the body the protocol rendered
/// and the framing that body already has.
fn frame_empty_body(response: &mut WireResponse) {
    if response.status().as_u16() == NO_CONTENT_STATUS {
        // The empty JSON object the serializer renders for an output structure with no
        // members goes no further: this status has no content to carry it.
        *response.body_mut() = aws_smithy_types::body::SdkBody::empty();
    }
}

/// The `com-amazonaws-notifications-notifications` emulated service: the generated [`ServiceDefinition`] plus
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

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing correction the module docs state.
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
        let mut response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status and every other header are the protocol's: only an answer that
        // carries no content is emptied of the body rendered for it and framed.
        frame_empty_body(&mut response);
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let mut response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries a JSON error envelope at a 4xx or
        // 5xx, so nothing here is emptied or reframed; the same call is made on both paths
        // rather than leaving one of them to answer differently.
        frame_empty_body(&mut response);
        Ok(response)
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
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a NUMERIC BOUND and a `@pattern` failure against the
        // member that failed and the requirement it broke, without quoting the submitted
        // value back and with a pattern's anchors dropped from the expression it quotes
        // (see [`clause`]); every other class keeps the framework's wording, and so does
        // an authentication refusal, which is not an input-constraint failure at all. The
        // code stays the modelled `ValidationException` (HTTP 400), which is what the
        // framework would have answered; only the wording changes.
        //
        // A MAP member over its `@length` maximum is answered by the schema layer that
        // reads the request as a JSON object, which reports the object's property count
        // rather than a member-length clause — and reports it INSTEAD of the per-member
        // envelope, so it is asked first.
        if let Some(refused) = service_common::validation_message::property_count_refusal(refusal) {
            return Some(refused);
        }
        match refusal {
            Refusal::Invalid(violations) => invalid_refusal(violations),
            _ => None,
        }
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by
        // the protocol returned here. See the module docs: it is the model's own protocol,
        // with one correction — an answer that carries no content carries no body and
        // states `content-length: 0`, instead of an empty JSON object and no framing
        // header at all.
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

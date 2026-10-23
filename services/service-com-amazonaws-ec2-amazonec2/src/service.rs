// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ec2-amazonec2` service: how it claims requests and how it words the
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
//! - `override_protocol` — the wire protocol the dispatch sequence runs. This
//!   service overrides it with [`Ec2QueryEnvelope`], which is the framework's
//!   ec2Query protocol plus the two parts of EC2's response envelope the Smithy
//!   protocol definition does not describe: the `<return>` success flag and the
//!   media type's `charset` parameter. It reaches every response this service
//!   renders and none of the ones the framework renders in front of it; see
//!   `RESPONSE_CONTENT_TYPE` for exactly where the line falls.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::{ProtocolError, QueryServerProtocol, ServerProtocol};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-ec2-amazonec2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's ec2Query protocol, built once here and served through
    /// [`Service::override_protocol`]. See [`Ec2QueryEnvelope`] for what it adds
    /// to the generated one.
    protocol: Ec2QueryEnvelope,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            // Built once, per `override_protocol`'s contract: never per request.
            protocol: Ec2QueryEnvelope::new(),
        }
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // EC2's ec2Query envelope carries two things the generated protocol does
        // not write: the success flag `<return>` on a mutation whose modelled
        // output shape does not declare it, and the `charset` parameter on the
        // response media type. Both belong to the envelope rather than to any one
        // operation, so they are served here, once, for every operation. What this
        // seam does NOT reach is noted on `RESPONSE_CONTENT_TYPE`.
        Some(&self.protocol)
    }
}

// =============================================================================
// EC2's ec2Query response envelope
// =============================================================================

/// The XML namespace EC2's model stamps on an ec2Query envelope — the service
/// shape's `@xmlNamespace` URI:
///
/// ```text
/// smithy_query(query="service_info",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// The generated `ServiceDefinition` builds its own protocol with this same
/// value. [`Ec2QueryEnvelope`] cannot borrow that instance — it lives beside it
/// on [`EmulatedService`] — so it names the namespace from the model here.
const XML_NAMESPACE: &str = "http://ec2.amazonaws.com/doc/2016-11-15";

/// The media type EC2 declares on an ec2Query response.
///
/// The model declares the protocol but not the media type's `charset`
/// parameter, so this is recorded behaviour: an EC2 ec2Query response answers
/// `content-type: text/xml;charset=UTF-8`, where the generated protocol writes
/// the bare `text/xml` the Smithy protocol definition prescribes.
///
/// [`Ec2QueryEnvelope`] is the one place a serializer's media type is chosen, so
/// it reaches every response this SERVICE renders: each success and each modelled
/// error, including the scaffold's 501. It does not reach the responses the
/// FRAMEWORK renders in front of the service — the refusal tail that answers a
/// constraint the model declares, a routing failure, a transport failure — which
/// `override_protocol` deliberately leaves in the model-derived envelope (its
/// contract in `vendor/smithy-server-runtime/src/service.rs`) and which therefore
/// still announce the bare `text/xml`. Those paths are a separate seam
/// (`override_refusal` / `override_routing_error` / `override_framework_error`)
/// and are left to the issue that owns them rather than reworded here.
const RESPONSE_CONTENT_TYPE: &str = "text/xml;charset=UTF-8";

/// The envelope's success flag, as the `<return>` element an ec2Query response
/// carries.
///
/// A member schema rather than a modelled one: the Smithy model declares no
/// `return` member on the output shapes this is written for, so the element has
/// no shape of its own to borrow. The XML codec names an element from the
/// member name, which is what makes this write `<return>true</return>`.
static RETURN_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#Ec2QueryEnvelope$return",
        "com.amazonaws.ec2",
        "return",
    ),
    ShapeType::Boolean,
    "return",
    0,
);

/// The operations whose modelled output declares members of its own and whose
/// AWS-documented response elements nevertheless include `return`.
///
/// Source: the EC2 API Reference's "Response Elements" for each operation.
/// `CreateSecurityGroup` documents `requestId`, `return` and `groupId`, while
/// `CreateSecurityGroupResult` in the Smithy model declares only `groupId`,
/// `tags` and `securityGroupArn` — so the flag has to come from the envelope.
/// An operation that reports a created resource instead of a flag (`CreateVpc`
/// answers `<vpc>` and no `<return>`) is deliberately absent.
const RETURN_BESIDE_MODELLED_OUTPUT: &[&str] = &["CreateSecurityGroup"];

/// An output serialized as itself, then followed by the envelope's `<return>`
/// success flag.
///
/// `true` is not a value read off state: reaching the serializer at all means
/// the handler returned `Ok`, which is exactly what the flag reports. A failure
/// renders through `serialize_error` and never through here.
struct WithReturnFlag<'a> {
    inner: &'a dyn SerializableStruct,
}

impl SerializableStruct for WithReturnFlag<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.inner.serialize_members(s)?;
        s.write_boolean(&RETURN_MEMBER, true)
    }
}

/// EC2's ec2Query protocol: the framework's [`QueryServerProtocol`] plus the two
/// parts of EC2's envelope the Smithy protocol definition does not describe.
///
/// Request reading, routing and the error envelope's shape are the framework's
/// unchanged — this only adds what EC2 writes on top.
#[derive(Debug)]
pub struct Ec2QueryEnvelope {
    inner: QueryServerProtocol,
}

impl Ec2QueryEnvelope {
    fn new() -> Self {
        Self {
            inner: QueryServerProtocol::ec2_query().with_namespace(XML_NAMESPACE),
        }
    }

    /// Whether this operation's success envelope carries the `<return>` flag.
    ///
    /// Three cases, in order:
    ///
    /// * the modelled output declares a `return` member of its own — the
    ///   handler owns the value (`DeleteSecurityGroup` sets it), so the envelope
    ///   must not write a second one;
    /// * the modelled output declares no members at all (`smithy.api#Unit`) —
    ///   the operation has nothing of its own to report, and every EC2 operation
    ///   modelled that way is a mutation, so the flag is the whole body. No
    ///   reporting operation is modelled with a Unit output: every `Describe*`
    ///   and `Get*` answers a result shape;
    /// * otherwise the flag is written only for the operations
    ///   [`RETURN_BESIDE_MODELLED_OUTPUT`] names.
    fn writes_return_flag(operation_name: &str, output_schema: &Schema) -> bool {
        let members = output_schema.members();
        if members
            .iter()
            .any(|member| member.member_name() == Some("return"))
        {
            return false;
        }
        members.is_empty() || RETURN_BESIDE_MODELLED_OUTPUT.contains(&operation_name)
    }
}

impl ServerProtocol for Ec2QueryEnvelope {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = if Self::writes_return_flag(operation_name, output_schema) {
            self.inner.serialize_output(
                &WithReturnFlag { inner: output },
                output_schema,
                operation_name,
                success_status,
            )?
        } else {
            self.inner
                .serialize_output(output, output_schema, operation_name, success_status)?
        };
        response
            .headers_mut()
            .insert("content-type", RESPONSE_CONTENT_TYPE);
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self
            .inner
            .serialize_error(error, error_schema, error_code, http_status)?;
        response
            .headers_mut()
            .insert("content-type", RESPONSE_CONTENT_TYPE);
        Ok(response)
    }
}

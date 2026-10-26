// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mediapackagevod-mediapackagevod` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it: it puts the input-validation gate described
//!   below in front of every modelled operation and lets the generated routing table
//!   serve the request from there.
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
//! ## Why this service refuses a request that breaks a modelled rule itself
//!
//! MediaPackage VOD models no `ValidationException`: the client-input error every
//! operation that takes parameters declares is `UnprocessableEntityException`, HTTP **422**,
//! "The parameters sent in the request are not valid"
//! (`smithy_query(query="operation",operation="ListAssets")` lists `ForbiddenException` 403,
//! `InternalServerErrorException` 500, `NotFoundException` 404, `ServiceUnavailableException`
//! 503, `TooManyRequestsException` 429 and `UnprocessableEntityException` 422, and nothing
//! else), and this service answers it with the per-attribute `errors` map `crate::scaffolded`
//! documents. The framework's request boundary refuses a value outside a declared constraint
//! — and a `@required` member the request left out — BEFORE any handler runs, and words that
//! refusal as a generic **400 `ValidationException`** with no `errors` member: a status and a
//! code this service never produces, so an SDK cannot deserialize it into a modelled type and
//! a client rendering per-field errors gets none. The seam that rewords such a refusal
//! (`Service::override_refusal` -> `Refused`) carries only a code and a message, never a
//! status or a body member, so it cannot state this one.
//!
//! So this service refuses such a request itself, through [`ModelledInputValidation`]: a gate
//! `override_operation` puts in front of every modelled operation. The gate collects the
//! violations from the framework's OWN machinery — the protocol's deserializer for an absent
//! required member and its constraint walk for a value outside a declared bound, through
//! `service_common::validation::refusing_violations` — so the rules enforced are exactly the
//! model's and nothing about a constraint is restated here; only the envelope and the words
//! are this service's ([`crate::state::modelled_constraint_refusal`]). Every request the
//! service accepts is handed straight to the generated operation, which the routing table
//! still owns.
//!
//! Two things the gate deliberately does not touch: authentication, which the router settles
//! before any operation runs and reads off the MODELLED operation the routing key names (so a
//! gate cannot weaken an operation's auth schemes), and the per-operation telemetry the
//! centralized sequence records — a request the gate refuses never enters that sequence, so
//! it is counted nowhere, exactly as a request refused before dispatch is.
//!
//! And one request it cannot reach: the gate stands in front of the operation the request
//! NAMES, so a request that resolves to no operation never reaches it. `UntagResource` is
//! the one operation of this model that has such a request — its `@required` `TagKeys` is
//! `@httpQuery("tagKeys")`, and the framework's REST route table treats a `@required`
//! request-observable binding as part of route matching
//! (`vendor/smithy-schema-server/src/rest_xml.rs`, `RequiredBinding` and `route_lookup`), so
//! a `DELETE /tags/{ResourceArn}` carrying no `tagKeys` matches no route at all and is
//! answered by the routing-error path. That is route matching below this crate, not the
//! envelope this seam owns.
//!
//! ## How an answer that carries no content is framed
//!
//! Several operations here answer `204 No Content`: each of the three deletes serves that
//! recorded status (`DeleteAsset`, `DeletePackagingConfiguration`,
//! `DeletePackagingGroup` — see each operation's `SUCCESS_STATUS`), and `TagResource` and
//! `UntagResource` are modelled with it (`smithy_query(query="digest")`). Such an answer
//! carries no content at all, and the length it does have is stated:
//! `content-length: 0`.
//!
//! Two things stand between the protocol's serializer and that answer. The three deletes
//! declare an output STRUCTURE with no members rather than `smithy.api#Unit`, and the
//! serializer renders that structure as the empty JSON object `{}` — a body on a response
//! that has none, which RFC 9110 §15.3.5 gives a 204 no room for. And a response the
//! serializer leaves with an empty body carries no framing header, which leaves the length
//! to the transport — so a client, proxy or SDK transport that keys on a framing header
//! being present on a bodyless response reads the emulator's reply differently from the
//! service's.
//!
//! `override_protocol` below therefore serves a protocol that is the model's own in every
//! respect except one: a `204` this service answers carries no body, whatever the
//! serializer rendered for it, and a response this service serializes with an EMPTY body
//! states `content-length: 0`. A response that carries content is left exactly as the
//! model's protocol renders it, framed from the body it carries. It is done here, at the
//! one place every response of this service passes through, rather than per operation.
//!
//! That framing is what this service answers, and it is what the in-process front door
//! delivers (the inter-service callback path and the SDK connector both serve the router's
//! response unchanged). An answer served over HTTP/1.1 reaches the client without the
//! header: the HTTP writer the vendored transport uses removes `content-length` from a
//! `204`, which that same section forbids there, and that writer is below anything this
//! crate decides. `tests/all/empty_response_framing.rs` pins what this service states.

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused,
    ServerHttpProtocol, Service,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};

/// The `com-amazonaws-mediapackagevod-mediapackagevod` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The input-validation gate in front of each modelled operation, keyed by the
    /// routing key the protocol resolves for it (the operation's own `NAME`). Built once,
    /// in `new`.
    input_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            input_validation: input_validation_gates::<Eng>(),
        }
    }
}

/// One [`ModelledInputValidation`] gate per modelled operation.
///
/// Each gate is zero-sized: it carries only the operation TYPE, which is what names the
/// input shape and the schema the framework's walk runs against. The key is the
/// operation's own `NAME`, so it is the model's routing key rather than a literal written
/// here.
fn input_validation_gates<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($map:ident; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(
                        ModelledInputValidation::<crate::ops::$module::$operation<Eng>>::new(),
                    ) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    // Every operation the model binds. What a request must satisfy is a property of its
    // INPUT SHAPE, so the gate goes in front of all of them: a request this service will
    // not accept is refused the same way whichever operation it named.
    gate!(
        gates;
        configure_logs::ConfigureLogs,
        create_asset::CreateAsset,
        create_packaging_configuration::CreatePackagingConfiguration,
        create_packaging_group::CreatePackagingGroup,
        delete_asset::DeleteAsset,
        delete_packaging_configuration::DeletePackagingConfiguration,
        delete_packaging_group::DeletePackagingGroup,
        describe_asset::DescribeAsset,
        describe_packaging_configuration::DescribePackagingConfiguration,
        describe_packaging_group::DescribePackagingGroup,
        list_assets::ListAssets,
        list_packaging_configurations::ListPackagingConfigurations,
        list_packaging_groups::ListPackagingGroups,
        list_tags_for_resource::ListTagsForResource,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_packaging_group::UpdatePackagingGroup,
    );
    gates
}

/// This service's input-validation gate in front of one operation `Op`.
///
/// Zero-sized: `Op` is a type parameter only, naming the input shape to read the request
/// into and the input `Schema` the constraint walk runs against. It holds no engine and no
/// state, so the operation it guards stays the single owner of the request's behaviour and
/// there is no second instance of any operation and no second store.
struct ModelledInputValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModelledInputValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation + 'static> ErasedOperation for ModelledInputValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r WireRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, WireResponse> {
        Box::pin(async move {
            if let Some(refused) = refuse_invalid_input::<Op>(svc, req) {
                return refused;
            }
            // Accepted: the generated operation serves it, through the framework's own
            // deserialize -> validate -> handle -> serialize sequence (which finds
            // nothing left to refuse).
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable: this gate is only reached through `override_operation`,
                // which resolved its key from that same routing table.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The protocol `svc` answers with: its own [`Service::override_protocol`] when it serves
/// one, else the model-derived protocol of the generated entry.
///
/// The same choice the framework's centralized sequence makes, so a refusal the gate
/// renders is read and framed exactly like a response that sequence renders — one protocol
/// for every answer this service gives.
fn resolved_protocol(svc: &dyn Service) -> &dyn ServerHttpProtocol {
    svc.override_protocol()
        .unwrap_or_else(|| svc.definition().protocol())
}

/// `Some(response)` when this service will not accept `Op`'s input: its
/// `UnprocessableEntityException` envelope — HTTP 422, the modelled code, and both body
/// members (the sentence and the per-attribute `errors` map).
///
/// `None` when the request satisfies the model, and `None` for a request that failed to
/// READ for a reason that is not an input violation — an unreadable body is a protocol
/// failure, and the framework words those per protocol.
fn refuse_invalid_input<Op: Operation>(svc: &dyn Service, req: &WireRequest) -> Option<WireResponse> {
    let protocol = resolved_protocol(svc);
    // Both violation classes come from the framework's own machinery — the protocol's
    // deserializer finds a required member absent while READING the request, its
    // constraint walk finds a bad value by walking what was read — so this carries no
    // second copy of what counts as a violation.
    let violations = ::service_common::validation::refusing_violations::<Op>(protocol, req);
    let error = crate::state::modelled_constraint_refusal(&violations, Op::INPUT_SCHEMA)?;
    protocol
        .serialize_error(
            &error,
            &crate::scaffolded::UNPROCESSABLE_ENTITY_SCHEMA,
            crate::scaffolded::UNPROCESSABLE_ENTITY_CODE,
            crate::scaffolded::UNPROCESSABLE_ENTITY_STATUS,
        )
        // An envelope that will not serialize leaves the request to the framework, which
        // refuses it in the generic envelope — the very divergence this gate removes, so
        // it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                operation = Op::NAME,
                %error,
                "this service's UnprocessableEntityException would not serialize; this \
                 refusal falls back to the framework's generic envelope"
            );
        })
        .ok()
}

/// The status whose answer carries NO content, whatever a renderer wrote for it, and the
/// length such an answer states. See the module docs.
const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// How an answer that carries no content is framed: with no body at all, and with the
/// `content-length: 0` that states the length it has, rather than with no framing header.
/// See the module docs.
///
/// A response that carries content is untouched: it keeps the body the protocol rendered
/// and the framing that body already has.
fn frame_empty_body(response: &mut WireResponse) {
    if response.status().as_u16() == NO_CONTENT_STATUS {
        // The empty JSON object the serializer renders for an output structure with no
        // members goes no further: this status has no content to carry it.
        *response.body_mut() = aws_smithy_types::body::SdkBody::empty();
    }
    if response.body().bytes().is_some_and(<[u8]>::is_empty) {
        response
            .headers_mut()
            .insert("content-length", NO_CONTENT_LENGTH);
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
        self.entry.protocol().deserialize_input(request, input_schema)
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
        // Put this service's input-validation gate in front of the modelled operation the
        // request names, so a request that breaks a rule the MODEL declares is refused
        // with this service's `UnprocessableEntityException` (422, both body members)
        // instead of the framework's generic 400 `ValidationException` envelope — see the
        // module docs. The gate then hands every request the service accepts to the
        // generated operation, which the routing table still owns; a request that names no
        // modelled operation resolves as it always did.
        let key = self.entry.protocol().routing_key(req).ok()?;
        let gate = self.input_validation.get(key)?;
        Some(gate.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Left to the framework DELIBERATELY. Every input violation this service states is
        // answered by the gate above, before the centralized sequence reaches this seam,
        // and `Refused` carries only a code and a message: rewording a refusal here would
        // name `UnprocessableEntityException` on a response the protocol still sends as
        // 400 with no `errors` member — a code that does not match its status, which is a
        // worse answer than the framework's own. The only refusals that reach here are the
        // ones the gate leaves alone (authentication, a body the codec cannot read), which
        // are the emulator's and the protocol's rather than this service's.
        let _ = refusal;
        None
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

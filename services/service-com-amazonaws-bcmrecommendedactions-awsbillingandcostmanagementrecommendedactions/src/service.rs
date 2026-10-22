// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions` service: how it claims requests and how it words the
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
//! - `override_protocol` — the codec this service's own responses are serialized
//!   in. The default returns `None`, keeping the model-derived protocol; this
//!   service overrides it (see "The media type this service announces" below).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The media type this service announces
//!
//! The model declares the `awsJson1_0` protocol trait, but every recorded response
//! this SERVICE produces — a success and a failure its handler raises alike —
//! announces `application/x-amz-json-1.1`, and the media type a client reads is what
//! it keys content negotiation and protocol assertion off. `override_protocol`
//! below therefore serves the awsJson `1.1` codec: byte-identical JSON to `1.0`, so a
//! request sent with either media type still reads.
//!
//! A refusal of the REQUEST is not a response the service produced: the recorded
//! refusals of an unmodelled member value carry the `1.0` media type the model's
//! protocol trait declares, so [`ModelledValidation`] serializes in the
//! model-derived protocol, and the framework's own routing and transport envelopes
//! are left as they are.

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationError, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The codec this service answers in (awsJson `1.1`; see the module docs). Built
    /// once here because [`Service::override_protocol`] hands back a borrow of it.
    protocol: aws_smithy_schema_server::rpc::HttpRpcServerProtocol<
        aws_smithy_schema_server::json::JsonCodec,
    >,
    /// States a refused `ListRecommendedActions` input in that operation's own
    /// modelled `ValidationException`; see [`ModelledValidation`].
    modelled_validation: ModelledValidation<Eng>,
}

/// Serves `ListRecommendedActions`, stating a refused input in the operation's own
/// modelled `ValidationException` instead of the framework's generic refusal.
///
/// WHY this is a seam and not a line in the handler: `ValidationException` models
/// `reason` (REQUIRED) and `fieldList` beside `message`, and the framework
/// validates every modelled constraint — `ActionFilter$key` and `$matchOption` are
/// enum shapes, `FilterValues` has a minimum length, `MaxResults` a range — BEFORE
/// a handler runs, rendering a violation as the protocol's generic refusal body,
/// which carries `message` alone. A client deserializing that error gets null for a
/// member the model marks required, so `reason`-based branching and per-field
/// reporting are impossible. The refusal is therefore stated HERE, where the
/// violations the boundary found are still in hand, through the same
/// [`crate::ops::list_recommended_actions::field_validation_failed`] rendering the
/// handler's own refusals use; a request with no violation is handed to the
/// generated walk unchanged.
///
/// Generic over the engine type for the same reason the rest of this file is: the
/// operation whose schema and routing key it reads is, so naming a concrete engine
/// here would let this seam drift from the service the router mounts.
struct ModelledValidation<Eng>(::std::marker::PhantomData<Eng>);

impl<Eng: Send + Sync + 'static> ModelledValidation<Eng> {
    /// The violations in `req`, read the way the framework's own walk reads them:
    /// deserialize the input against the operation's schema, then validate the built
    /// value against it. `None` means the request carries no structured refusal —
    /// either it is valid, or it failed in a way that has no member-level form (a
    /// body the codec cannot read at all), which the generated walk renders itself.
    fn violations(
        protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<Vec<smithy_server_runtime::Violation>> {
        let schema = Self::input_schema();
        let error = match protocol.deserialize_input(req, schema) {
            Ok(mut deser) => {
                match <crate::ops::types::ListRecommendedActionsInput as DeserializableStruct>::deserialize(
                    &mut *deser,
                ) {
                    // A built input: every modelled constraint, in one walk.
                    Ok(input) => {
                        let violations = aws_smithy_schema_server::constraints::validate(
                            &input, schema,
                        );
                        return (!violations.is_empty()).then_some(violations);
                    }
                    // Absence is found while READING the request, so it arrives as a
                    // codec error; `refusal_of` gives it the same structured form a
                    // constraint violation has.
                    Err(e) => aws_smithy_schema_server::ProtocolError::Serde(e),
                }
            }
            Err(e) => e,
        };
        match smithy_server_runtime::refusal_of(&error) {
            Some(Refusal::Invalid(violations)) if !violations.is_empty() => Some(violations),
            _ => None,
        }
    }

    /// The operation's input schema — read off the operation itself, never spelled
    /// out here, so this seam validates exactly what the generated walk validates.
    fn input_schema() -> &'static aws_smithy_schema::Schema {
        <crate::ops::list_recommended_actions::ListRecommendedActions<Eng> as Operation>::INPUT_SCHEMA
    }

    /// The routing key the model names this operation by.
    fn operation_name() -> &'static str {
        <crate::ops::list_recommended_actions::ListRecommendedActions<Eng> as Operation>::NAME
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ModelledValidation<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The MODEL-DERIVED protocol, not this service's own override: a
            // refusal of the request is answered by the input validator in front of
            // the service, which reads and announces the protocol the model declares
            // (see the module docs on the media type). The codec is the same JSON
            // either way, so the request reads identically.
            let protocol = svc.definition().protocol();
            if let Some(violations) = Self::violations(protocol, req) {
                let modeled =
                    crate::ops::list_recommended_actions::field_validation_failed(&violations);
                return protocol
                    .serialize_error(
                        modeled.as_serializable(),
                        modeled.variant_schema(),
                        modeled.shape_name(),
                        modeled.http_status(),
                    )
                    .unwrap_or_else(|e| svc.definition().framework_error_response(e));
            }
            // Nothing to restate: run the generated deserialize -> validate ->
            // handle -> serialize walk. Reached through the routing table rather than
            // through `override_operation`, which would route straight back here.
            // The walk reads and validates the body again — the price of answering a
            // refusal in the modelled shape, paid once per request on a body the
            // transport has already bounded.
            match svc.definition().routing_table().get(Self::operation_name()) {
                Some(op) => op.run(svc, req, ctx).await,
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: aws_smithy_schema_server::rpc::AwsJson11(),
            modelled_validation: ModelledValidation(::std::marker::PhantomData),
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
        // One case needs it: `ListRecommendedActions`, so that a refused input is
        // stated in that operation's own modelled `ValidationException` — with the
        // modelled `reason` and `fieldList` — instead of the framework's generic
        // refusal body. See [`ModelledValidation`] for why that cannot be done from
        // inside `handle`. The operation name is read from the model, not spelled out,
        // and a request naming any other operation is left to the generated routing
        // table.
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        (routing_key == ModelledValidation::<Eng>::operation_name())
            .then_some(&self.modelled_validation as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Nothing to reword here: a refusal of this service's one operation never
        // reaches this seam. `ModelledValidation` answers it BEFORE the framework
        // renders it, because the words are not the problem — the generic refusal
        // body cannot carry the modelled `reason`/`fieldList` members at all, and
        // `Refused` is only a code and a message.
        let _ = refusal;
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response this SERVICE produces — a success and a modelled failure
        // its handler raises — is serialized by the protocol returned here, so this
        // is the one place the media type it announces is decided. See the module
        // docs: the recorded responses carry the awsJson `1.1` media type, which is
        // what a client keys its parsing off.
        Some(&self.protocol)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation. Left to the entry's protocol-correct default (awsJson ->
        // 404 in the media type the model's protocol declares): naming no operation
        // is refused in FRONT of the service, like an unmodelled member value, so it
        // is not a response the service produced.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // A transport-level failure the router detects before naming an operation (a
        // malformed `aws-chunked` body). Left to the entry's protocol-correct
        // rendering: the error arrives BORROWED and is not `Clone`, so it cannot be
        // re-rendered here, and the default renderer consumes the owned one.
        let _ = error;
        None
    }
}

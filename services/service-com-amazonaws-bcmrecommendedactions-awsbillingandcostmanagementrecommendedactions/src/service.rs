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
//!   renders the protocol-correct default, which for the awsJson protocol this
//!   model declares is **501 `NotImplemented`** in the media type that protocol
//!   declares (`application/x-amz-json-1.0`). This is the one place in this crate
//!   that states that status — the seam below points here rather than restating
//!   it, and `tests/all/unrouted_operation.rs` pins it on the wire; the
//!   per-protocol table it comes from lives in
//!   [`smithy_server_runtime::pre_dispatch`], which is where to look for a
//!   protocol other than this one. Return `Some(resp)` to answer in your own
//!   envelope if desired.
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
/// That covers EVERY way this input can be refused, not only the member-level
/// ways: a request the codec cannot read at all — a member present with a value of
/// the wrong JSON type, a body that is not the input object — is refused here too,
/// as the modelled `cannotParse`
/// ([`crate::ops::list_recommended_actions::cannot_parse_input`]). Handing that
/// class to the generated walk would render exactly the generic body described
/// above, so the one class of input this service calls unreadable would be the one
/// whose refusal states no `reason`.
///
/// Generic over the engine type for the same reason the rest of this file is: the
/// operation whose schema and routing key it reads is, so naming a concrete engine
/// here would let this seam drift from the service the router mounts.
struct ModelledValidation<Eng>(::std::marker::PhantomData<Eng>);

/// Why this operation refuses the request's input, in the two classes the modelled
/// `ValidationExceptionReason` distinguishes.
///
/// Both are stated in the operation's own `ValidationException`, which is what this
/// seam exists for: neither class may reach the framework's generic refusal body,
/// where the REQUIRED `reason` cannot travel.
enum InputRefusal {
    /// Values the service READ and does not accept — an absent required member, a
    /// value outside a modelled constraint, a value outside an enum's set. Stated
    /// as `fieldValidationFailed`, one `fieldList` entry per violation.
    Refused(Vec<smithy_server_runtime::Violation>),
    /// A value the service could not READ at all. Stated as `cannotParse`, with the
    /// codec's own explanation as the message and one `fieldList` entry per input
    /// member whose value could not be read — empty when the body names none, as a
    /// body that is not even an object does not.
    Unreadable {
        detail: String,
        members: Vec<(String, String)>,
    },
}

impl<Eng: Send + Sync + 'static> ModelledValidation<Eng> {
    /// How `req`'s input is refused, read the way the framework's own walk reads it:
    /// deserialize the input against the operation's schema, then validate the built
    /// value against it. `None` means this operation has nothing to restate — either
    /// the input is valid, or the boundary refused the request for a reason that is
    /// not this input's content (an authentication refusal, a body the transport
    /// could not read), whose status and envelope the framework owns.
    fn refusal(
        protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<InputRefusal> {
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
                        return (!violations.is_empty())
                            .then_some(InputRefusal::Refused(violations));
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
            Some(Refusal::Invalid(violations)) if !violations.is_empty() => {
                Some(InputRefusal::Refused(violations))
            }
            // A codec failure with no member-level form, which is the rest of
            // `ProtocolError::Serde`: a member whose value is of the wrong JSON
            // type, a body that is not the input object, JSON that does not parse.
            // The service could not READ the request, so it is refused as
            // `cannotParse` rather than handed on to the generated walk, which would
            // re-read the same body, fail the same way and render the generic body.
            _ => match error {
                aws_smithy_schema_server::ProtocolError::Serde(serde_error) => {
                    Some(InputRefusal::Unreadable {
                        // The codec words its failures for the CALLER (`expected
                        // number, got string`), so its explanation is the message —
                        // the same one this refusal carried before it also carried a
                        // reason.
                        detail: serde_error.to_string(),
                        members: Self::unreadable_members(protocol, req),
                    })
                }
                _ => None,
            },
        }
    }

    /// Each input member whose own value the codec could not read, named the way the
    /// boundary validator names a member, with the codec's explanation for it.
    ///
    /// Found by reading each member of the body ON ITS OWN through the SAME codec
    /// that failed on the whole body: a member that reads in isolation is not one the
    /// codec choked on, and one that does not read is reported with the clause it
    /// failed with. Only keys the MODEL declares as members of this input are read
    /// back, so the work is bounded by the input shape rather than by how many keys a
    /// request carries, and a key the codec skips as unknown is never reported.
    ///
    /// An empty answer is the honest one for a body with no member to name — one that
    /// is not an object, or does not parse as JSON at all — and the model allows it:
    /// `fieldList` is optional where `reason` is not.
    fn unreadable_members(
        protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Vec<(String, String)> {
        let schema = Self::input_schema();
        let Some(body) = req.body().bytes() else {
            return Vec::new();
        };
        let Ok(serde_json::Value::Object(members)) =
            serde_json::from_slice::<serde_json::Value>(body)
        else {
            return Vec::new();
        };
        members
            .iter()
            .filter(|(key, _)| Self::declares_member(schema, key))
            .filter_map(|(key, value)| {
                let mut only = serde_json::Map::new();
                only.insert(key.clone(), value.clone());
                let probe = aws_smithy_runtime_api::http::Request::new(
                    aws_smithy_types::body::SdkBody::from(
                        serde_json::Value::Object(only).to_string(),
                    ),
                );
                let mut deser = protocol.deserialize_input(&probe, schema).ok()?;
                let error = <crate::ops::types::ListRecommendedActionsInput as DeserializableStruct>::deserialize(
                    &mut *deser,
                )
                .err()?;
                // A member ABSENT from a one-member body is not a member that could
                // not be read: that is what the whole-body read reports, and it
                // already has a member-level form of its own.
                if matches!(
                    error,
                    aws_smithy_schema::serde::SerdeError::MissingMember { .. }
                ) {
                    return None;
                }
                Some((
                    aws_smithy_schema_server::constraints::path::member(key),
                    error.to_string(),
                ))
            })
            .collect()
    }

    /// Whether the model declares `key` as a member of this operation's input, by the
    /// name the WIRE carries — a member's `jsonName` when it declares one, its member
    /// name otherwise, which is how the codec itself looks a key up.
    fn declares_member(schema: &aws_smithy_schema::Schema, key: &str) -> bool {
        schema.members().iter().any(|member| {
            member.json_name().map(|name| name.value()) == Some(key)
                || member.member_name() == Some(key)
        })
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
            if let Some(refusal) = Self::refusal(protocol, req) {
                let modeled = match refusal {
                    InputRefusal::Refused(violations) => {
                        crate::ops::list_recommended_actions::field_validation_failed(&violations)
                    }
                    InputRefusal::Unreadable { detail, members } => {
                        crate::ops::list_recommended_actions::cannot_parse_input(&detail, &members)
                    }
                };
                return protocol
                    .serialize_error(
                        modeled.as_serializable(),
                        modeled.variant_schema(),
                        modeled.shape_name(),
                        modeled.http_status(),
                    )
                    .unwrap_or_else(|e| svc.definition().framework_error_response(e, req));
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
    ///
    /// This is also where this service's store is installed
    /// ([`crate::state::install_service_state`]): the emulator runs this constructor
    /// once per service as it boots, so the inventory the store starts with — and
    /// the instant stamped on each of its records — is fixed at the emulator's start
    /// rather than by whichever request reaches the store first.
    pub fn new(engine: Arc<Eng>) -> Self {
        crate::state::install_service_state(&*engine);
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
        // modeled operation. Left to the entry's protocol-correct default, whose
        // status and media type the module docs above state once: naming no operation
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

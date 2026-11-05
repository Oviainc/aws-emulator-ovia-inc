// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudformation-cloudformation` service: how it claims requests and how it words the
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
//! Two of those seams are taken for the one reason named in
//! [`query_error_envelope`]: an awsQuery error response carries a `<RequestId>` element
//! in its `<ErrorResponse>` envelope, which the framework's renderers omit.
//! `override_operation` wraps the operation the routing table names, so the element
//! reaches a handler-returned error and a refusal raised while serving it, and the
//! [`CloudFormationDefinition`] this file hands back from `definition` adds it to a
//! routing failure, a transport failure and a pre-dispatch refusal. Everything else about
//! those paths — status, `<Type>`, `<Code>`, `<Message>` — stays with the framework.
//!
//! `override_refusal` is taken for two classes as well: a refusal of the member that names a
//! PUBLIC EXTENSION BY ARN, which CloudFormation's registry parses itself, is stated as the
//! registry states it rather than as a generic request-validation failure — see
//! [`registry_refusal_of_a_public_extension_arn`] — and a value outside an enum whose value
//! set this service lists in its own order is stated in THAT order, see
//! [`enum_value_set_in_the_services_own_order`].
//!
//! `override_protocol` is taken for one rendering: a floating-point member is written as a
//! double, with its fractional part, which is how CloudFormation's envelope writes one —
//! see [`double_members`].
//!
//! The definition this file hands back is also where this front door's AUDIT leg hangs: the
//! generated entry is reached through the shared management-event recorder
//! ([`management_events`]), so every call this service serves — accepted and refused alike —
//! is recorded in the account's event history where CloudTrail's `LookupEvents` reports it,
//! through one code path for every operation rather than a recording written into each
//! handler.

pub mod custom_resource_plane;
pub mod double_members;
pub mod management_events;
pub mod query_error_envelope;

use crate::state::provisioning::custom_resource;
use crate::state::registry;
use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol, Violation, ViolationKind};
use double_members::WithDoublesAsDoubles;
use query_error_envelope::{CloudFormationDefinition, RequestIdEnvelopeOperation};
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The dotted path the validating walk names the member that identifies a public extension
/// BY ARN at — `ActivateTypeInput$PublicTypeArn`, the one member of this service's model
/// targeting `ThirdPartyTypeArn`
/// (`smithy_query(query="operation", operation="ActivateType")`), in the lowerCamel spelling
/// every violation path takes.
const PUBLIC_EXTENSION_ARN_PATH: &str = "publicTypeArn";

/// The error code of the registry's own modelled refusal, `CFNRegistryException` — the wire
/// code its `awsQueryError` trait declares, which is what a client activating a third-party
/// extension branches on.
const REGISTRY_REFUSAL_CODE: &str = "CFNRegistryException";

/// The wire error code this service refuses a request on input grounds with — the code a
/// CloudFormation client branches on, and the one the framework's query renderer already
/// names for a refused value, restated here because a reworded refusal must carry it too
/// (see [`crate::scaffolded::Scaffolded::ValidationError`]).
const VALIDATION_REFUSAL_CODE: &str = "ValidationError";

/// `RegistrationStatus`, in the order CloudFormation's OWN input validation lists its value
/// set: the registration still running first, then the two statuses it concludes in.
///
/// That is the order a registration passes through, and it is not the order the model
/// declares the enum in (`smithy_query(query="shape", shape="RegistrationStatus")` gives
/// `[COMPLETE, IN_PROGRESS, FAILED]`) — so the refusal is stated from this order rather than
/// from the declaration.
const REGISTRATION_STATUS_VALUE_SET: &[&str] = &[
    registry::registration_status::IN_PROGRESS,
    registry::registration_status::COMPLETE,
    registry::registration_status::FAILED,
];

/// `TemplateStage`, in the order CloudFormation's OWN input validation lists its value set:
/// the stage CloudFormation DEPLOYS first, then the one the template was submitted as.
///
/// That is not the order the model declares the enum in
/// (`smithy_query(query="shape", shape="TemplateStage")` gives `[Original, Processed]`) — so a
/// value outside the set is refused from this order rather than from the declaration.
const TEMPLATE_STAGE_VALUE_SET: &[&str] = &[
    crate::ops::get_template::TEMPLATE_STAGE_PROCESSED,
    crate::ops::get_template::TEMPLATE_STAGE_ORIGINAL,
];

/// Every enum of this service whose value set its own refusals state in an order other than
/// the model's declaration order.
///
/// Keyed by the SET rather than by an operation or a member, so each entry governs every
/// member that targets that enum and one refusal cannot drift from another's.
const SERVICE_ORDERED_ENUM_VALUE_SETS: &[&[&str]] =
    &[REGISTRATION_STATUS_VALUE_SET, TEMPLATE_STAGE_VALUE_SET];

/// The order this service states `allowed` in, when it is one of the sets
/// [`SERVICE_ORDERED_ENUM_VALUE_SETS`] names — compared as a SET, so the model's declaration
/// order is what the entry replaces.
fn service_ordered_value_set(allowed: &[String]) -> Option<&'static [&'static str]> {
    SERVICE_ORDERED_ENUM_VALUE_SETS
        .iter()
        .copied()
        .find(|ordered| {
            ordered.len() == allowed.len()
                && allowed
                    .iter()
                    .all(|value| ordered.contains(&value.as_str()))
        })
}

/// This service's wording for a value outside an enum's declared set: the same refusal the
/// framework states, with the value set listed in the order the SERVICE lists it
/// ([`SERVICE_ORDERED_ENUM_VALUE_SETS`]).
///
/// The message is a requirement of this error class — a client matching the service's
/// refusal text matches the order too — and only the order changes: the status, the code and
/// every other word stay the framework's, which is why the sentence is rebuilt through the
/// framework's own envelope rather than spelled again here. `None` when no violation of this
/// request names a set this service orders differently, which keeps every other refusal
/// exactly as the framework worded it.
fn enum_value_set_in_the_services_own_order(violations: &[Violation]) -> Option<Refused> {
    let mut restated = violations.to_vec();
    let mut reordered = false;
    for violation in &mut restated {
        let ViolationKind::EnumValue(allowed) = &mut violation.kind else {
            continue;
        };
        let Some(ordered) = service_ordered_value_set(allowed) else {
            continue;
        };
        let in_the_services_order: Vec<String> =
            ordered.iter().map(|value| (*value).to_string()).collect();
        if *allowed != in_the_services_order {
            *allowed = in_the_services_order;
            reordered = true;
        }
    }
    if !reordered {
        return None;
    }
    Some(Refused {
        code: VALIDATION_REFUSAL_CODE.to_string(),
        message: aws_smithy_schema_server::envelope(&restated)?,
    })
}

/// The registry's own refusal for a public-extension ARN this service's registry parses and
/// resolves nothing from, in place of the generic request-validation wording.
///
/// CloudFormation reads the extension's kind, publisher and NAME out of that ARN itself and
/// refuses what the lookup cannot resolve with the registry's modelled error — the ARN is not
/// gated on syntax ahead of the registry, so the modelled `ThirdPartyTypeArn` pattern (which
/// is narrower than the ARNs CloudFormation parses) must not pre-empt that refusal with a
/// different error class. This restates exactly the two refusals the registry owns: a value
/// pinning a public version number past the extension's name
/// ([`registry::PublicTypeArnParts::public_version_number`]), and one whose name segment is no
/// extension type name ([`registry::is_type_name`]) — the same refusals, in the same order,
/// that `ActivateType`'s handler answers for an ARN the pattern let through. Every other
/// refusal of that member, and every other member's, keeps the framework's wording: this
/// service has no evidence that CloudFormation words those as registry failures.
fn registry_refusal_of_a_public_extension_arn(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        if violation.path != PUBLIC_EXTENSION_ARN_PATH
            || !matches!(violation.kind, ViolationKind::Pattern(_))
        {
            return None;
        }
        let arn = violation.value.as_deref()?;
        let parts = registry::split_public_type_arn(arn);
        let message = match &parts {
            Some(parts) if parts.public_version_number.is_some() => {
                registry::PUBLIC_VERSION_NUMBER_IN_ARN
            }
            Some(parts) if registry::is_type_name(&parts.type_name) => return None,
            _ => registry::INVALID_TYPE_NAME,
        };
        Some(Refused {
            code: REGISTRY_REFUSAL_CODE.to_string(),
            message: message.to_string(),
        })
    })
}

/// The `com-amazonaws-cloudformation-cloudformation` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: CloudFormationDefinition<Eng>,
    /// The envelope wrapper every routable request is served through. Held in a field,
    /// built once, because `override_operation` hands back a reference to it.
    envelope: RequestIdEnvelopeOperation<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the custom resource RESPONSE plane is served from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own; it must read and write the one store the operations use,
    /// not a second copy.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            // The generated entry, routing through the AUDITED table: every modelled
            // operation, each constructed from this same shared engine and wrapped in the
            // management-event recorder (see [`management_events`]), so each call this
            // front door serves is recorded as a management event of
            // `cloudformation.amazonaws.com`.
            entry: CloudFormationDefinition::new(
                Arc::clone(&engine),
                management_events::audited_table(&engine),
            ),
            envelope: RequestIdEnvelopeOperation::new(Arc::clone(&engine)),
            engine,
        }
    }
}

/// The protocol this service answers modelled requests in: the model's own, with every
/// floating-point member rendered as a double ([`double_members`]).
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model. Reading
/// and routing a request are the generated protocol's unchanged; so is every byte of the
/// envelope it writes.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a WireRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a WireRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_output(
            &WithDoublesAsDoubles::new(output),
            output_schema,
            operation_name,
            success_status,
        )
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        // A modelled error of this service declares no floating-point member today, so the
        // wrapper changes nothing here; it is applied anyway so the rule is the protocol's
        // rather than one path's, and a modelled error that gains one cannot answer
        // differently from an output.
        self.entry.protocol().serialize_error(
            &WithDoublesAsDoubles::new(error),
            error_schema,
            error_code,
            http_status,
        )
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            &WithDoublesAsDoubles::new(event),
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

/// The host a request is addressed to and the PATH it names, read through the workspace's
/// one reading of each: a `Host` header where the request carries one, else the authority of
/// its absolute URI ([`::service_common::data_plane_addressing::addressed_host`]), and the
/// path with any scheme, authority and query removed
/// ([`::service_common::request_path`]). Both are reached from a request's PARTS
/// (`can_handle`) and from a runtime-API request (`override_routing_error`), which share no
/// type, so each seam passes the two things it has.
fn addressed_host(uri: &str, host_header: Option<&str>) -> Option<String> {
    ::service_common::data_plane_addressing::addressed_host(uri, host_header)
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own protocol, with the one rendering correction above. Held on `&self`
        // (it IS `&self`), so nothing is built per request.
        Some(self)
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        if match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        } {
            return true;
        }
        // And a request addressed to the host this service publishes a custom resource
        // provider request's `ResponseURL` on, whose first label names no service at all:
        // without this the router answers `UnknownService` for a URL this service handed
        // out. See [`custom_resource_plane`].
        if addressed_host(
            &parts.uri.to_string(),
            parts.headers.get("host").and_then(|host| host.to_str().ok()),
        )
        .is_some_and(|host| custom_resource::is_response_host(&host))
        {
            return true;
        }
        // ...and one whose PATH is one of those published URLs, whatever endpoint name it
        // reached this emulator by: the path names the STACK, by its own ARN, so a provider
        // following the URL it was handed addresses this service's own target either way.
        custom_resource::parse_response_path(parts.uri.path())
            .is_some_and(|target| custom_resource::target_region(&target).is_some())
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request the generated routing table can serve is served through the
        // envelope wrapper, which delegates to that same table entry and then adds the
        // `<RequestId>` the awsQuery error envelope carries (see
        // [`query_error_envelope`]). The routing stays the model's: a request whose
        // `Action` names no modelled operation is declined here, so the framework answers
        // it as the unroutable request it is.
        self.entry.routes(req).then_some(&self.envelope as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. Two classes are reworded here: a refusal of the member that names a
        // PUBLIC EXTENSION BY ARN, which the registry parses itself (see
        // [`registry_refusal_of_a_public_extension_arn`]), and a value outside an enum whose
        // value set this service lists in its own order (see
        // [`enum_value_set_in_the_services_own_order`]).
        match refusal {
            Refusal::Invalid(violations) => {
                registry_refusal_of_a_public_extension_arn(violations)
                    .or_else(|| enum_value_set_in_the_services_own_order(violations))
            }
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A write to the `ResponseURL` of a custom resource provider request is exactly that
        // case by construction — the path is not in the model — so this is where the
        // provider's documented answer is received. The PATH decides: a request to this
        // service's own endpoint that named no operation is a routing failure, whatever its
        // host looks like, and keeps the framework's default.
        let target =
            custom_resource::parse_response_path(::service_common::request_path(req.uri()))?;
        // The Region the answer is about: the one the stack's own ARN names, and failing that
        // the one the published host carries.
        let region = match custom_resource::target_region(&target) {
            Some(region) => region.to_string(),
            None => addressed_host(req.uri(), req.headers().get("host"))
                .and_then(|host| {
                    custom_resource::response_host_region(&host).map(str::to_string)
                })?,
        };
        Some(custom_resource_plane::serve(
            &*self.engine,
            req,
            &region,
            &target,
        ))
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

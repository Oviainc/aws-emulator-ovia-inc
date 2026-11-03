// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-iotmanagedintegrations-iotmanagedintegrations` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. The key and the handler stay the generated
//!   table's; the operation is served through [`InModelledEnvelope`], which states a
//!   request refused for violating a modelled constraint in THIS service's own
//!   `ValidationException` instead of the framework's pre-dispatch envelope. See
//!   "Why the modelled envelope" below.
//! - `override_refusal` — EDITED HERE. A refusal the framework still renders (the
//!   fallback in [`validation_exception`]) is worded from the same violations through
//!   the same [`stated`], so the two refusal paths cannot disagree.
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
//! # Why the modelled envelope
//!
//! Every error shape this service's model declares carries its prose in a member the
//! model names `Message`
//! (`smithy_query(query="shape",shape="ValidationException",model_path="services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json")`
//! reports `ValidationException {Message: ErrorMessage}`, `httpError` 400), and a
//! modelled error serialized through that shape's own schema writes exactly that
//! member name. The framework's pre-dispatch renderer, which answers a request refused
//! BEFORE any handler ran, gets the status (400) and the error code
//! (`ValidationException`) right but writes its prose under a framework-chosen,
//! protocol-wide lowercase `message` key (`json` in
//! `vendor/smithy-server-runtime/src/pre_dispatch.rs`), because the shape it renders is
//! the synthetic `smithy.framework#ValidationException` rather than this service's.
//! A caller reading the member the model declares therefore finds no `Message` at all
//! on a refusal, and an SDK surfaces the exception with an empty reason.
//!
//! So the operations are served through [`InModelledEnvelope`], which collects the
//! refusal itself — from the framework's own deserializer and constraint walk, so not
//! one rule is restated here — and renders it through the model's own
//! `ValidationException` schema. Nothing else about the request changes: a request this
//! service does not refuse is handed to the generated `ErasedOperation::run`, which
//! owns deserialize -> validate -> handle -> serialize, telemetry, and every modelled
//! error.
//!
//! # How this service words a refusal
//!
//! The sentence is the framework's own envelope — `N validation error(s) detected:
//! Value at '<member path>' failed to satisfy constraint: <the requirement>` — with the
//! REJECTED VALUE left out of the clause ([`stated`]). Managed integrations states a
//! refused member and the requirement it failed and does not quote back what was sent:
//! both refusals recorded from the service — an out-of-set value on
//! `CreateManagedThing`'s `authenticationMaterialType` and on
//! `CreateProvisioningProfile`'s `provisioningType`, neither member `@sensitive` — carry
//! the clause without it. It is also the only safe wording here, because most of this
//! service's constrained input members ARE `@sensitive` (`Owner`,
//! `AuthenticationMaterial`, `SerialNumber`, `Brand`, `Model`, `Classification`, `Tags`,
//! `CaCertificate`, …), and an error body must not publish a value the model marks so.
//!
//! An enum clause states a MEMBERSHIP and an ORDER. The membership is the model's,
//! always. The order is not: it is a property of the ENUM that the public model does not
//! carry, and the only source for it is the order this service's refusals are recorded
//! stating. Both enums whose refusal is recorded list their values in an order no order
//! derivable from the model reproduces — `AuthMaterialType`, which the model declares as
//! `[CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE, ZWAVE_QR_BAR_CODE,
//! ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]`, is listed
//! `[PRE_ONBOARDED_CLOUD, ZIGBEE_QR_BAR_CODE, ZWAVE_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE,
//! DISCOVERED_DEVICE, CUSTOM_PROTOCOL_QR_BAR_CODE]`, and `ProvisioningType`, declared
//! `[FLEET_PROVISIONING, JITR]`, is listed `[JITR, FLEET_PROVISIONING]`. So the listing is
//! recorded per enum ([`VALUE_LISTINGS`], [`listed`]) beside the ONE renderer every
//! enum-constrained member of every operation reaches ([`clause`]), rather than in a
//! handler or per operation.
//!
//! A listing is identified by the SET it orders — which is what a violation carries, a
//! clause having no shape id — and is applied only to an enum whose admitted set it is a
//! permutation of. It therefore cannot add a value this model does not declare nor drop
//! one it does; it reaches that enum through every operation, every binding (body, URI
//! label, query string), a nested path and a handler's own second-line check alike; and an
//! enum whose membership the model changes stops matching a stale listing and states the
//! model's order rather than an outdated set. An enum with no recorded listing keeps the
//! order the member schema carries, which is the model's declaration order: no order over
//! the model reproduces both recorded listings — reversing the declaration gives the
//! two-value one but lists `DISCOVERED_DEVICE` second where the recording lists it fifth,
//! and a hashed set's bucket order (`java.util.HashSet` over `[FLEET_PROVISIONING, JITR]`)
//! iterates `FLEET_PROVISIONING` first where the recording lists `JITR` first — so there
//! is no rule to extend to an enum nothing recorded.
//!
//! `tests/all/enum_value_set_listing.rs` pins the order both halves state, and
//! `tests/all/enum_value_set_rendering.rs` the membership: an out-of-set enum member is
//! stated with EXACTLY the value set the model declares for it — every declared value
//! once, nothing undeclared, the same set whichever operation, binding or rejected value
//! reached it — rendered `[a, b, c]`.
//!
//! Everything else about the clause — which constraint classes exist and how each one
//! words its requirement — stays the framework's, so a constraint class added there
//! cannot arrive with a sentence of this service's own.
//!
//! Two costs of serving the operations this way are deliberate. The wrapper reads the
//! request a second time (`deserialize_input` + `constraints::validate`, which the
//! generated `run` then repeats), so a request that the model accepts is parsed and
//! validated twice; the framework exposes no seam that hands a refusal to the service
//! before rendering it, and correctness of the refusal is worth one extra parse of a
//! control-plane request. And the wrapped set is spelled out in [`wrap_operations`]:
//! an operation the model gains later is NOT wrapped until it is listed there, and
//! keeps the framework's lowercase-`message` envelope until it is — the fallback is a
//! worse message, never a wrong route, because `override_operation` answers `None` for
//! a key it does not hold and the generated table serves it unchanged.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The wire error code for a request this service refuses as invalid: the shape
/// `com.amazonaws.iotmanagedintegrations#ValidationException`, which every operation of
/// this service lists and whose only member the model calls `Message`.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The `com-amazonaws-iotmanagedintegrations-iotmanagedintegrations` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before its handler runs is refused
    /// in this service's own `ValidationException`. Keyed by the routing key the
    /// protocol resolves — the key the generated table uses — and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by the operation's own `Operation::NAME` — the
/// model-derived routing key `ServerHttpProtocol::routing_key` resolves and the
/// generated table is keyed by, so no key is spelled by hand here.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(InModelledEnvelope::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut in_modelled_envelope = OperationRoutingTable::new();
        // EVERY operation the model binds. A declared constraint is a property of the
        // INPUT SHAPE, not of a handler, so the wrapper goes in front of all of them —
        // implemented or still scaffolded: a request that violates the model is refused
        // the same way whoever would have served it.
        wrap_operations!(
            in_modelled_envelope,
            engine,
            create_account_association::CreateAccountAssociation,
            create_cloud_connector::CreateCloudConnector,
            create_connector_destination::CreateConnectorDestination,
            create_credential_locker::CreateCredentialLocker,
            create_destination::CreateDestination,
            create_event_log_configuration::CreateEventLogConfiguration,
            create_managed_thing::CreateManagedThing,
            create_notification_configuration::CreateNotificationConfiguration,
            create_ota_task::CreateOtaTask,
            create_ota_task_configuration::CreateOtaTaskConfiguration,
            create_provisioning_profile::CreateProvisioningProfile,
            delete_account_association::DeleteAccountAssociation,
            delete_cloud_connector::DeleteCloudConnector,
            delete_connector_destination::DeleteConnectorDestination,
            delete_credential_locker::DeleteCredentialLocker,
            delete_destination::DeleteDestination,
            delete_event_log_configuration::DeleteEventLogConfiguration,
            delete_managed_thing::DeleteManagedThing,
            delete_notification_configuration::DeleteNotificationConfiguration,
            delete_ota_task::DeleteOtaTask,
            delete_ota_task_configuration::DeleteOtaTaskConfiguration,
            delete_provisioning_profile::DeleteProvisioningProfile,
            deregister_account_association::DeregisterAccountAssociation,
            get_account_association::GetAccountAssociation,
            get_cloud_connector::GetCloudConnector,
            get_connector_destination::GetConnectorDestination,
            get_credential_locker::GetCredentialLocker,
            get_custom_endpoint::GetCustomEndpoint,
            get_default_encryption_configuration::GetDefaultEncryptionConfiguration,
            get_destination::GetDestination,
            get_device_discovery::GetDeviceDiscovery,
            get_event_log_configuration::GetEventLogConfiguration,
            get_hub_configuration::GetHubConfiguration,
            get_managed_thing::GetManagedThing,
            get_managed_thing_capabilities::GetManagedThingCapabilities,
            get_managed_thing_certificate::GetManagedThingCertificate,
            get_managed_thing_connectivity_data::GetManagedThingConnectivityData,
            get_managed_thing_meta_data::GetManagedThingMetaData,
            get_managed_thing_state::GetManagedThingState,
            get_notification_configuration::GetNotificationConfiguration,
            get_ota_task::GetOtaTask,
            get_ota_task_configuration::GetOtaTaskConfiguration,
            get_provisioning_profile::GetProvisioningProfile,
            get_runtime_log_configuration::GetRuntimeLogConfiguration,
            get_schema_version::GetSchemaVersion,
            list_account_associations::ListAccountAssociations,
            list_cloud_connectors::ListCloudConnectors,
            list_connector_destinations::ListConnectorDestinations,
            list_credential_lockers::ListCredentialLockers,
            list_destinations::ListDestinations,
            list_device_discoveries::ListDeviceDiscoveries,
            list_discovered_devices::ListDiscoveredDevices,
            list_event_log_configurations::ListEventLogConfigurations,
            list_managed_thing_account_associations::ListManagedThingAccountAssociations,
            list_managed_thing_schemas::ListManagedThingSchemas,
            list_managed_things::ListManagedThings,
            list_notification_configurations::ListNotificationConfigurations,
            list_ota_task_configurations::ListOtaTaskConfigurations,
            list_ota_task_executions::ListOtaTaskExecutions,
            list_ota_tasks::ListOtaTasks,
            list_provisioning_profiles::ListProvisioningProfiles,
            list_schema_versions::ListSchemaVersions,
            list_tags_for_resource::ListTagsForResource,
            put_default_encryption_configuration::PutDefaultEncryptionConfiguration,
            put_hub_configuration::PutHubConfiguration,
            put_runtime_log_configuration::PutRuntimeLogConfiguration,
            register_account_association::RegisterAccountAssociation,
            register_custom_endpoint::RegisterCustomEndpoint,
            reset_runtime_log_configuration::ResetRuntimeLogConfiguration,
            send_connector_event::SendConnectorEvent,
            send_managed_thing_command::SendManagedThingCommand,
            start_account_association_refresh::StartAccountAssociationRefresh,
            start_device_discovery::StartDeviceDiscovery,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_account_association::UpdateAccountAssociation,
            update_cloud_connector::UpdateCloudConnector,
            update_connector_destination::UpdateConnectorDestination,
            update_destination::UpdateDestination,
            update_event_log_configuration::UpdateEventLogConfiguration,
            update_managed_thing::UpdateManagedThing,
            update_notification_configuration::UpdateNotificationConfiguration,
            update_ota_task::UpdateOtaTask,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusals it owes stated in this service's
/// own `ValidationException` — the modelled shape, whose prose member the model names
/// `Message` — instead of the framework's pre-dispatch envelope, which names a
/// lowercase `message` for every restJson1 service alike. See the module docs for why.
///
/// The refusal is built from the structured [`Violation`]s the framework's own walk
/// collects, never from a rendered response, so the wrapper cannot find a different set
/// of violations from the walk that follows it.
struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition` is
            // not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = refused_by::<O>(protocol, req);
            if let Some(refusal) = validation_exception(protocol, &violations) {
                return refusal;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a constraint the model declares (`@length`, `@range`,
/// `@pattern`, an enum's value set, `@uniqueItems`). Empty when the request satisfies
/// the model, and empty for a failure that is NOT a refusal — a body the codec cannot
/// read at all is a serialization failure, which the framework words per protocol.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer
/// and `constraints::validate` over the built input), which is the same pair the
/// centralized sequence uses, so no rule is restated here.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // Absence is found while READING the request and a bad value while walking what
        // was read; to a caller they are one refusal, so they are one here — exactly as
        // `smithy_server_runtime::refusal_of` classifies them.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }],
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations`: HTTP 400 `ValidationException` with the prose
/// in the `Message` member the model declares, serialized through that shape's own
/// schema.
///
/// `None` when there is nothing to refuse — and also when the modelled error cannot be
/// serialized, which leaves the request to the framework's walk rather than answering a
/// 400 with no reason at all.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    let refused = crate::ops::types::ValidationException {
        // One wording for both refusal paths, so one violation cannot be stated two
        // ways: `stated` is what `override_refusal` hands the framework as well.
        message: Some(stated(violations)?),
    };
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        // A modelled error that will not serialize leaves the request to the framework,
        // which refuses it in the generic envelope — the very divergence this wrapper
        // removes, so it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                %error,
                "the modelled ValidationException would not serialize; this refusal falls \
                 back to the framework's generic envelope"
            );
        })
        .ok()
}

/// One violation's clause as this service states it: the framework's own clause
/// ([`Violation::message`]) with the rejected value lifted out of its opening, so only
/// `Value '<what was sent>' at` becomes `Value at`. See the module docs for why this
/// service does not quote the value back.
///
/// The replaced text is built from the SAME two fields the framework's renderer
/// interpolated, so it matches the echo whatever the value holds — including the elided
/// rendering a long value is stored as — and the ONE replacement is confined to this
/// clause's own opening. A violation with no value to echo (a `@required` member absent)
/// is left exactly as the framework words it.
fn clause(violation: &Violation) -> String {
    // An enum clause lists the value set in the order THIS SERVICE lists that enum's
    // values; every other class is worded exactly as the framework words it.
    let relisted = relisted(violation);
    let violation = relisted.as_ref().unwrap_or(violation);
    let clause = violation.message();
    match violation.value.as_deref() {
        Some(value) => clause.replacen(
            &format!("Value '{value}' at '{}'", violation.path),
            &format!("Value at '{}'", violation.path),
            1,
        ),
        None => clause,
    }
}

/// The order this service states an enum's admitted values in, for the enums whose
/// refusals are recorded publishing a listing.
///
/// This is service metadata the public model does not carry: a Smithy enum declares a
/// membership, and the order a refusal lists it in is the service's own (see the module
/// docs, which show that no order over the model — declaration, reversed, sorted, by
/// length, by a hashed set's buckets — reproduces both recorded listings). So the only
/// source for a listing is what the service's refusals state, and a listing is recorded
/// here, beside the one renderer every enum-constrained member of every operation reaches,
/// rather than in an operation handler.
///
/// A listing is identified by the SET it orders — the violation carries the set, not a
/// shape id — and so is applied ONLY to an enum whose admitted set it is a permutation of:
/// it can neither add a value this model does not declare nor drop one it does, and an
/// enum whose membership the model changes stops matching a stale listing and states the
/// model's order again. Each set below is declared by exactly one enum of this model, so a
/// listing reaches the enum it was recorded for and no other.
const VALUE_LISTINGS: &[&[&str]] = &[
    // `AuthMaterialType` (`CreateManagedThing.AuthenticationMaterialType`), which the
    // model declares `[CUSTOM_PROTOCOL_QR_BAR_CODE, WIFI_SETUP_QR_BAR_CODE,
    // ZWAVE_QR_BAR_CODE, ZIGBEE_QR_BAR_CODE, DISCOVERED_DEVICE, PRE_ONBOARDED_CLOUD]`.
    &[
        "PRE_ONBOARDED_CLOUD",
        "ZIGBEE_QR_BAR_CODE",
        "ZWAVE_QR_BAR_CODE",
        "WIFI_SETUP_QR_BAR_CODE",
        "DISCOVERED_DEVICE",
        "CUSTOM_PROTOCOL_QR_BAR_CODE",
    ],
    // `ProvisioningType` (`CreateProvisioningProfile.ProvisioningType`), which the model
    // declares `[FLEET_PROVISIONING, JITR]`.
    &["JITR", "FLEET_PROVISIONING"],
];

/// The same violation with its enum value set in the order this service lists that enum's
/// values, or `None` when there is nothing to re-list: a violation of another class, or an
/// enum this service publishes no listing for — which keeps the order the member schema
/// carries, the model's.
///
/// The clause itself is still rendered by the framework ([`Violation::message`]), so only
/// the ORDER of the set is this service's; the requirement's wording, the path and the
/// envelope stay in one place.
fn relisted(violation: &Violation) -> Option<Violation> {
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return None;
    };
    let declared: Vec<&str> = allowed.iter().map(String::as_str).collect();
    let listing = listed(&declared)?;
    Some(Violation {
        path: violation.path.clone(),
        kind: ViolationKind::EnumValue(listing.iter().map(|value| (*value).to_string()).collect()),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    })
}

/// The listing this service publishes for the enum admitting exactly `declared`, or `None`
/// when it publishes none.
fn listed(declared: &[&str]) -> Option<&'static [&'static str]> {
    VALUE_LISTINGS
        .iter()
        .copied()
        .find(|listing| orders_the_same_set(listing, declared))
}

/// Whether two value lists hold the same values, in whatever order each holds them.
fn orders_the_same_set(one: &[&str], other: &[&str]) -> bool {
    if one.len() != other.len() {
        return false;
    }
    let mut one: Vec<&str> = one.to_vec();
    let mut other: Vec<&str> = other.to_vec();
    one.sort_unstable();
    other.sort_unstable();
    one == other
}

/// `violations` as this service states them, or `None` when there is nothing to state.
///
/// The envelope is the one the framework writes (`aws_smithy_schema_server::envelope`):
/// the violation count, then the clauses joined by `"; "`. It is rebuilt here rather
/// than post-processed because the CLAUSES are this service's ([`clause`]), and because
/// rewriting the whole rendered envelope once per violation would cost the product of
/// the two — both of which a request controls through the number of members it puts
/// outside their constraints. Each clause is now touched exactly once, so the wording is
/// linear in what the walk collected (which the framework also caps, at 100 violations
/// and 256 echoed characters each) on a path every operation reaches before its handler.
fn stated(violations: &[Violation]) -> Option<String> {
    match violations {
        [] => None,
        [only] => Some(format!("1 validation error detected: {}", clause(only))),
        many => {
            let clauses: Vec<String> = many.iter().map(clause).collect();
            Some(format!(
                "{} validation errors detected: {}",
                many.len(),
                clauses.join("; ")
            ))
        }
    }
}

/// The refusal a HANDLER owes for a value outside an enum's value set, worded through the
/// one path every refusal of this service is worded through ([`clause`], [`stated`]).
///
/// The constraint walk in front of every operation ([`InModelledEnvelope`]) refuses an
/// out-of-set enum before any handler sees one, so a handler's own check is a second line
/// — and a second line must not state the same refusal differently from the first. The
/// sentence is the framework's envelope with the rejected value left out: `1 validation
/// error detected: Value at '<member path>' failed to satisfy constraint: Member must
/// satisfy enum value set: [...]`. `rejected` is what the request sent, and it is passed
/// only so [`clause`] can lift the framework's echo of it back out — it is never
/// published, which also keeps a `@sensitive` member's value out of the body.
///
/// `member_path` is the wire (lowerCamel) path the clause names, and `allowed` is the
/// member's declared value set, in whatever order the caller holds it: the clause states it
/// in the order this service lists that enum's values ([`VALUE_LISTINGS`]), so a handler's
/// second line and the walk in front of it state one refusal identically.
pub(crate) fn enum_value_set_refusal(member_path: &str, allowed: &[&str], rejected: &str) -> String {
    let violation = Violation {
        path: member_path.to_string(),
        kind: ViolationKind::EnumValue(allowed.iter().map(|value| (*value).to_string()).collect()),
        value: Some(rejected.to_string()),
        sensitive: false,
    };
    // One violation, so `stated` answers `Some`; the fallback states the same clause
    // without the count rather than asserting that with a panic in a request path.
    stated(std::slice::from_ref(&violation)).unwrap_or_else(|| clause(&violation))
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request refused before the handler runs is refused
        // in this service's modelled `ValidationException` rather than in the
        // framework's pre-dispatch envelope, which names no `Message`. A key the model
        // does not name resolves to `None` here exactly as it does in the generated
        // table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` states a constraint refusal itself, in the modelled
        // envelope, so this seam is what words a refusal the FRAMEWORK still renders —
        // the serialization fallback in `validation_exception`. Wording it from the same
        // violations through the same [`stated`] keeps the two paths from disagreeing: a
        // value outside a declared constraint is refused with the same sentence either
        // way, and neither quotes the rejected value back. The code is the one the
        // framework would have answered with (`ValidationException`, which is also the
        // modelled shape), because a client branches on the code and `Refused` replaces
        // the code and the message together.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: stated(violations)?,
            }),
            // Authentication is the framework's to word: the request never reached an
            // operation, so there is nothing of this service's own to say about it.
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
}

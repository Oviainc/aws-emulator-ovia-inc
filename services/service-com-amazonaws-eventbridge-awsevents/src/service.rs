// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-eventbridge-awsevents` service: how it claims requests and how it words the
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
//!   correctly, or serve one THROUGH something of the service's own. This service
//!   serves `PutEvents` through the wrapper that lets a refusal of its `Entries`
//!   list state the entries the request carried (see the inline note on
//!   `QuotingSubmittedEntries`); every other request resolves through the
//!   generated routing table.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. This service words one here: the constraint clauses
//!   the framework states, with a `@pattern` requirement reported without the
//!   anchors the model spells it with. See the inline note on that method.
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

use smithy_server_runtime::{
    http, ErasedOperation, Operation, OperationRoutingTable, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use service_common::validation::SubmittedEcho;
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the
/// `ValidationException` the framework itself would have chosen for this
/// `awsJson1_1` service — restated here because `override_refusal` replaces the
/// code and the message together, so rewording one without the other would
/// change which code a client branches on.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Render one constraint violation the way this service states it: the value
/// that was rejected, the member path it sat at, and the requirement it failed.
///
/// Every requirement but one is worded exactly as the framework words it. The
/// exception is `@pattern`, reported as the expression a value must satisfy
/// WITHOUT the anchors the model spells it with
/// (`crate::state::pattern_requirement`): the anchoring is how the model makes
/// the match total, not part of the expression the service asks a value to
/// satisfy, and a client that lifts the expression out of the message to
/// re-validate its input cannot use one wrapped in anchors it never sent.
///
/// `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => crate::state::pattern_requirement(pattern),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    // An absent member carries no value, and is reported as the null it was.
    let value = match &violation.kind {
        ViolationKind::MissingRequiredMember => None,
        _ => violation.value.as_deref(),
    };
    Some(crate::state::constraint_clause(
        &violation.path,
        value,
        &requirement,
    ))
}

/// The `ValidationException` message for a refusal: the count of violated
/// constraints over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    if clauses.is_empty() {
        return None;
    }
    Some(crate::state::validation_envelope(&clauses))
}

/// `violation` with the clause's value half replaced by the COLLECTION the caller
/// submitted, where the framework stored its own summary of one there.
///
/// A constraint clause of this service states the value that was rejected, and the
/// value a clause on a collection member rejects is the collection itself. The
/// framework's constraint walk has no rendering for one — it stores
/// `<n> items`, read back with the one reader of that rendering
/// (`service_common::validation_message::list_item_count`) — which is not a value
/// any caller sent, so that one clause states the list as `echo` renders it. Every
/// other clause keeps the framework's own rendering, which already states the
/// bytes the caller submitted, and a collection the echo walk could not render
/// (one past the bounds it states) keeps the summary rather than losing the
/// clause.
fn quoting_the_submitted_collection(violation: Violation, echo: SubmittedEcho) -> Violation {
    let summarised = violation.value.as_deref().is_some_and(|stored| {
        matches!(
            violation.kind,
            ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
        ) && ::service_common::validation_message::list_item_count(stored).is_some()
    });
    match echo {
        SubmittedEcho::Rendered(value) if summarised => Violation {
            value: Some(value),
            ..violation
        },
        _ => violation,
    }
}

/// The member POSITIONS the echo-collecting walk may visit for one request of this
/// service, stated from this model rather than taken from the shared default
/// (256).
///
/// A COLLECTION is quoted in a clause only when every one of its elements was
/// walked, so the ceiling has to cover the collection whose printed form this
/// service reproduces one element PAST its bound: `PutEventsRequestEntryList` is
/// `@length` max 10 over `PutEventsRequestEntry`, whose seven members bottom out
/// in seven leaf positions plus the elements of its `Resources` list, so 11
/// entries need a few hundred. This covers that with room for the rest of the
/// request. Past it the framework's own count summary stands, which is the
/// fail-safe direction: this model's largest collection of structures,
/// `TargetList` (max 100) over the deeply nested `Target`, is far beyond it and no
/// recording shows what AWS echoes for one, so a clause on that list keeps the
/// framework's summary.
const MAX_ECHO_POSITIONS: usize = 512;

/// One modelled operation, served so that a refusal of a `@length` on one of its
/// COLLECTION members states the collection the caller submitted.
///
/// Why it exists: [`Service::override_refusal`] is handed the violations alone,
/// never the request they were found in, so the one clause whose value the
/// framework could not render ([`quoting_the_submitted_collection`]) cannot be
/// completed there. Here the input can be re-read.
///
/// Nothing is re-implemented and no check is repeated in other words: the
/// violations and the echoes come from the tree's shared collecting walk
/// (`service_common::validation::refusing_violations_with_printed_echoes_over`,
/// which runs the protocol's own deserializer and the framework's own
/// `constraints::validate` and owns every bound on what one clause may quote), the
/// sentence from the one envelope builder this file uses for every refusal, and the
/// status and the envelope from the framework's refusal channel. A request this
/// wrapper does not refuse is handed to the generated operation, which owns
/// deserialize -> validate -> handle -> serialize and every modelled error. The
/// cost is DELIBERATE: an accepted request's body is read twice, here and in the
/// operation's own sequence, because the framework hands no already-built input to
/// an operation.
struct QuotingSubmittedCollections<Op> {
    operation: Op,
}

impl<Op> QuotingSubmittedCollections<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: Op) -> Self {
        Self { operation }
    }
}

impl<Op> ErasedOperation for QuotingSubmittedCollections<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> smithy_server_runtime::BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across
            // the await below would make this future non-`Send`.
            let refused: Vec<Violation> =
                ::service_common::validation::refusing_violations_with_printed_echoes_over::<Op>(
                    svc.definition().protocol(),
                    req,
                    MAX_ECHO_POSITIONS,
                    crate::ops::put_events::PRINTED_ENTRY_MEMBERS,
                )
                .into_iter()
                .map(|(violation, echo)| quoting_the_submitted_collection(violation, echo))
                .collect();
            if let Some(message) = constraint_envelope(&refused) {
                return svc.definition().refusal_response(
                    aws_smithy_schema_server::ProtocolError::Invalid(refused),
                    Some(Refused {
                        code: VALIDATION_EXCEPTION.to_string(),
                        message,
                    }),
                    req,
                );
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must
    /// not change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The first URI path segment EVERY request to this service carries: the ROOT path.
///
/// This model is `awsJson1_1` and binds no `smithy.api#http` trait to any operation
/// (`smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-eventbridge-awsevents/smithy-model/\
/// com-amazonaws-eventbridge-awsevents.json")` reports protocol `awsJson1_1` and no
/// `http` binding), so every EventBridge request is `POST /` with
/// `X-Amz-Target: AWSEvents.<Operation>` — the crate's `OPERATIONS.md`, "Wire
/// protocol". `emulator_server::ROOT_PATH_SEGMENT` is the framework's sentinel for
/// exactly that path.
///
/// It is claimed EXPLICITLY, rather than through the generated `ROUTE_PATH_SEGMENTS`
/// (EMPTY for a service with no http bindings, which claims EVERY path under the
/// signing name): the `events` signing name is shared with the EventBridge V2
/// endpoint, whose `rpcv2Cbor` requests arrive at
/// `POST /service/AWSEventsV2/operation/<Operation>`. Claiming every path would make
/// both services claim that request and the router would answer `500
/// AmbiguousService` — a code that exists in no AWS model — instead of serving it.
const OWN_PATH_SEGMENTS: &[&str] = &[emulator_server::ROOT_PATH_SEGMENT];

/// Whether the request these `parts` came from matches this service's SigV4 signing
/// name AND the one URI path its wire protocol publishes ([`OWN_PATH_SEGMENTS`]).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of that rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment must
/// be one of the segments passed, by exact segment equality.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn claims_route_key_and_root_path(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        OWN_PATH_SEGMENTS,
    )
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`QuotingSubmittedCollections`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(QuotingSubmittedCollections::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// The `com-amazonaws-eventbridge-awsevents` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`QuotingSubmittedCollections`]. Keyed by the routing key the protocol resolves —
    /// the key the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of `self`.
    quoting_submitted_collections: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut quoting_submitted_collections = OperationRoutingTable::new();
        wrap_operations!(
            quoting_submitted_collections,
            engine,
            activate_event_source::ActivateEventSource,
            cancel_replay::CancelReplay,
            create_api_destination::CreateApiDestination,
            create_archive::CreateArchive,
            create_connection::CreateConnection,
            create_endpoint::CreateEndpoint,
            create_event_bus::CreateEventBus,
            create_partner_event_source::CreatePartnerEventSource,
            deactivate_event_source::DeactivateEventSource,
            deauthorize_connection::DeauthorizeConnection,
            delete_api_destination::DeleteApiDestination,
            delete_archive::DeleteArchive,
            delete_connection::DeleteConnection,
            delete_endpoint::DeleteEndpoint,
            delete_event_bus::DeleteEventBus,
            delete_partner_event_source::DeletePartnerEventSource,
            delete_rule::DeleteRule,
            describe_api_destination::DescribeApiDestination,
            describe_archive::DescribeArchive,
            describe_connection::DescribeConnection,
            describe_endpoint::DescribeEndpoint,
            describe_event_bus::DescribeEventBus,
            describe_event_source::DescribeEventSource,
            describe_partner_event_source::DescribePartnerEventSource,
            describe_replay::DescribeReplay,
            describe_rule::DescribeRule,
            disable_rule::DisableRule,
            enable_rule::EnableRule,
            list_api_destinations::ListApiDestinations,
            list_archives::ListArchives,
            list_connections::ListConnections,
            list_endpoints::ListEndpoints,
            list_event_buses::ListEventBuses,
            list_event_sources::ListEventSources,
            list_partner_event_source_accounts::ListPartnerEventSourceAccounts,
            list_partner_event_sources::ListPartnerEventSources,
            list_replays::ListReplays,
            list_rule_names_by_target::ListRuleNamesByTarget,
            list_rules::ListRules,
            list_tags_for_resource::ListTagsForResource,
            list_targets_by_rule::ListTargetsByRule,
            put_events::PutEvents,
            put_partner_events::PutPartnerEvents,
            put_permission::PutPermission,
            put_rule::PutRule,
            put_targets::PutTargets,
            remove_permission::RemovePermission,
            remove_targets::RemoveTargets,
            start_replay::StartReplay,
            tag_resource::TagResource,
            test_event_pattern::TestEventPattern,
            untag_resource::UntagResource,
            update_api_destination::UpdateApiDestination,
            update_archive::UpdateArchive,
            update_connection::UpdateConnection,
            update_endpoint::UpdateEndpoint,
            update_event_bus::UpdateEventBus,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            quoting_submitted_collections,
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
        // This service signs as `events` — a signing name it SHARES with the
        // EventBridge V2 endpoint (`services/service-com-amazonaws-eventbridgev2-\
        // awseventsv2`, `aws.auth#sigv4` name `events`, endpointPrefix `eventsv2`) and
        // with the LEGACY CloudWatch Events model of this very service. Claiming on the
        // signing name alone makes several services claim one request, which the router
        // answers `500 AmbiguousService` instead of serving any of them.
        //
        // So the claim is narrowed to this service's own request space: the route key
        // AND the single URI path its `awsJson1_1` wire protocol publishes (`POST /`),
        // leaving the V2 endpoint's `rpcv2Cbor` path space to that service. The legacy
        // CloudWatch Events crate — same shape, version, protocol, endpoint prefix and
        // signing name, a strict SUBSET of this model's operations — claims nothing, so
        // this service owns the `events` key deterministically.
        claims_route_key_and_root_path(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve the operation the generated routing table names, through the
        // wrapper that lets a refusal of a collection member state the collection
        // the request carried (see [`QuotingSubmittedCollections`]). The routing
        // DECISION is not overridden: the key is resolved by the model's own
        // protocol, and a request the table cannot serve is declined here so the
        // framework's routing-error path stays exactly as it was.
        let operation = self.definition().protocol().routing_key(req).ok()?;
        self.definition().routing_table().get(operation)?;
        self.quoting_submitted_collections.get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a constraint violation through the one builder in
        // `crate::state`, which is also what a handler enforcing an unmodelled
        // constraint reaches, so a refusal found at the boundary and one found in a
        // handler read identically. The only difference from the framework's own
        // wording is that a `@pattern` requirement is reported unanchored — see
        // [`constraint_clause`].
        //
        // The code stays `ValidationException` (HTTP 400), which is what the
        // framework would have answered; `override_refusal` replaces the code and
        // the message together, so it is restated rather than changed.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
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

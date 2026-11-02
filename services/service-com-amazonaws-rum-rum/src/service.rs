// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-rum-rum` service: how it claims requests and how it words the
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
//!   correctly. EDITED HERE, for two reasons that compose: every request this service
//!   serves goes through [`AnnouncedMediaType`], which runs the operation unchanged and
//!   announces this service's media type on the response (see "The media type this
//!   service announces" below), and a request that binds no value for a `@required`
//!   `@httpQuery` member — which the framework's required-query-binding rule leaves
//!   matching NO route, so the router would answer it "unknown operation" rather than
//!   letting the operation state its own refusal — is served by the operation the model
//!   binds to its method and path (see [`QUERY_UNMATCHED_ROUTES`], which lists every
//!   route of this service that binds such a member). Which operation serves a request is
//!   still the model's routing table's answer in every other case.
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
//! ## The media type this service announces
//!
//! The model declares `restJson1`, whose serializer announces
//! [`crate::state::JSON_CONTENT_TYPE`] on every JSON body, but a recorded RUM response to
//! a request that carried NO payload — every operation, a 200 and a 404 alike — carries
//! [`crate::state::RESPONSE_CONTENT_TYPE`] instead, and the announced media type is what a
//! client selecting a parser or validating the response's type reads. What the frontend
//! answers in is the media type the REQUEST's payload declared, and a request that carried
//! none declared nothing, so such an answer falls back to RUM's own JSON type; a request
//! that DID carry a payload declared `restJson1`'s `application/json` and is answered in
//! it. Both halves are a property of the SERVICE, uniform across operations and across
//! success and error paths, so they are applied once here rather than in twenty handlers:
//! [`AnnouncedMediaType`] fronts every request this service serves, runs the operation
//! the model names unchanged, and settles that one header on the way out
//! ([`announced`]). A response whose body is not the JSON envelope keeps the type its own
//! target shape maps to.
//!
//! `override_protocol` is not the seam for it: the codec is right, and swapping in
//! another protocol would swap away the URI-based routing this REST service needs.
//!
//! ## How a refused request is worded
//!
//! A request whose input breaks a modelled constraint is refused with the code and the
//! words [`stated`] chooses, and the clauses themselves are
//! [`crate::state::validation`]'s — the one place this service words a constraint refusal.
//! It is reached from two places, which is why the choice lives in one function: the gate
//! in front of every operation, which collects the violations against that operation's own
//! input schema together with the request's own lists
//! ([`AnnouncedMediaType::stated_refusal`]), and [`Service::override_refusal`], for a
//! refusal the centralized walk finds anyway and hands back as the violations alone.
//!
//! A refusal has TWO sources, and one request's clauses from both are stated in ONE
//! envelope. The framework's boundary walk applies the constraints its regex engine can
//! compile and runs BEFORE a handler; the rest — the look-around `@pattern`s and the
//! constraints of a shape reached as a map KEY — are applied by this service itself
//! ([`crate::state::validation`]). Those checks live with the operation that accepts the
//! members (`crate::ops::<operation>::self_applied`), and the gate asks the operation the
//! routing key names for them ([`collected_refusal`]) rather than leaving them to a handler
//! the walk's refusal means never runs: one clause per failing (member, requirement),
//! whichever layer applied it, in the order the input declares the members
//! ([`in_declared_order`]).
//!
//! Two things the gate deliberately does not touch: authentication, which the router
//! settles before any operation runs and reads off the MODELLED operation the routing
//! key names (so a gate cannot weaken an operation's auth schemes), and the refusals
//! that happen in FRONT of the service — a request naming no modelled operation, an
//! unverifiable signature, a malformed `aws-chunked` body. Those are not responses this
//! service produced, so they keep the framework's protocol-correct rendering.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{path, Violation, ViolationKind};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};
use std::collections::HashMap;
use std::sync::Arc;

/// The error code RUM words a refused TAG call with: `BadRequestException`.
///
/// The ARN-addressed tagging operations are served by a different frontend from this
/// service's app-monitor operations, with its own error vocabulary: the recorded corpus
/// unit `rum.untag-resource-unknown-resource-rejected` sends a `ResourceArn` that is not
/// an ARN and records `err["Code"] == "BadRequestException"`, where an app-monitor call
/// refused on a modelled constraint answers `ValidationException`. The STATUS is the same
/// 400 either way; the code a client branches on is not.
///
/// The one definition of that code is the frontend's own
/// (`crate::state::TAGGING_BAD_REQUEST_ERROR_CODE`, which
/// `crate::state::TaggingRefusal` answers a refusal the HANDLERS make with), so the
/// boundary and the handlers cannot word one condition two ways.
use crate::state::TAGGING_BAD_REQUEST_ERROR_CODE as TAGGING_BAD_REQUEST_CODE;

/// The whole message that frontend reports a `ResourceArn` which is not an ARN with.
///
/// It carries nothing from the request — not the value, not the member, not the pattern —
/// so it is a constant rather than a format string, and it is the same sentence for every
/// value that fails `Arn`'s syntax. Recorded in the same corpus unit, which sends a
/// `ResourceArn` of `not-an-arn-at-all` and pins this message. A refusal of any OTHER tag
/// member keeps the framework's request-derived sentence: the frontend's wording for those
/// is not known, and naming the member and the constraint it failed is what a caller needs
/// to fix the request.
const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The dotted lowerCamel path a violation on the tagging operations' `ResourceArn`
/// member reports.
const RESOURCE_ARN_MEMBER_PATH: &str = "resourceArn";

/// The input members only RUM's tagging operations declare: `ResourceArn` (`TagResource`,
/// `UntagResource`, `ListTagsForResource`) and `TagKeys` (`UntagResource`), as the dotted
/// lowerCamel paths a violation reports.
///
/// Source: `smithy_query(query="operation",
/// operation="TagResource,UntagResource,ListTagsForResource")` against this service's
/// model, and the service digest for the rest — every other operation here names its
/// resource by `Name` or by `Id`. `Tags` is deliberately NOT in the list: `TagResource`
/// and `CreateAppMonitor` both declare it, so a violation about it does not say which
/// frontend refused the call.
const TAGGING_MEMBER_PATHS: [&str; 2] = [RESOURCE_ARN_MEMBER_PATH, "tagKeys"];

/// Whether these violations are about a request to one of the tagging operations.
///
/// [`Refusal`] describes WHAT was refused, not which operation was called, so the member
/// is the discriminator: a violation on one of [`TAGGING_MEMBER_PATHS`] can only have come
/// from a tag call. A path is dotted, so a violation inside a member starts with that
/// member's name followed by a `.`.
///
/// Asked only where the operation itself is not in hand ([`Service::override_refusal`]);
/// the gate in front of every operation knows which operation the routing key named and
/// reads it off [`TAGGING_OPERATIONS`] instead, which is what lets a refusal about the tag
/// MAP — a member both frontends carry — still be worded by the frontend that answers the
/// call.
fn is_tagging_refusal(violations: &[Violation]) -> bool {
    violations.iter().any(|violation| {
        TAGGING_MEMBER_PATHS.iter().any(|member| {
            violation.path == *member || violation.path.starts_with(&format!("{member}."))
        })
    })
}

/// The operations RUM's ARN-addressed TAGGING frontend answers, by their modelled names:
/// every operation of this service that addresses its resource by `ResourceArn`.
///
/// Source: `smithy_query(query="operation",
/// operation="TagResource,UntagResource,ListTagsForResource")` against this service's
/// model — every other operation names its resource by `Name` or by `Id`. A refusal of one
/// of these is worded in that frontend's error vocabulary ([`stated`]) whatever member it is
/// about, which [`TAGGING_MEMBER_PATHS`] cannot say on its own for the tag map.
const TAGGING_OPERATIONS: [&str; 3] = ["TagResource", "UntagResource", "ListTagsForResource"];

/// Whether one of these violations says the `ResourceArn` is not an ARN: a `@pattern`
/// violation on that member, which is the one refusal the tagging frontend has its own
/// sentence for ([`INVALID_RESOURCE_ARN_MESSAGE`]).
fn is_resource_arn_syntax_violation(violations: &[Violation]) -> bool {
    violations.iter().any(|violation| {
        violation.path == RESOURCE_ARN_MEMBER_PATH
            && matches!(violation.kind, ViolationKind::Pattern(_))
    })
}

/// EVERY route of this service that binds a `@required` `@httpQuery` member, as
/// `(method, pattern, operation)` — the pattern being the one the model's `@http` trait
/// gives that operation.
///
/// The framework's REST router rejects a candidate route whose `@required` query bindings
/// are not on the request — the rule that lets an S3-class model tell several operations
/// apart on ONE method + URI template by the SHAPE of the request. This service overloads
/// no route: every method + URI template it publishes names exactly one operation, so for
/// a request that binds no value for such a member the rule only misfires: the request
/// matches NO route and is answered "unknown operation" (HTTP 501 `NotImplemented`, a
/// status and code no operation here models) instead of reaching the operation the model
/// binds to its method and path, whose own answer for an absent `@required` member is the
/// service's common `ValidationException` (HTTP 400).
///
/// So the routes below are resolved by method and path alone, which takes nothing away
/// from the model's own routing table — it is consulted first, and only a request it
/// cannot name is matched here. The table is EVERY route of this service with such a
/// member, so no sibling route answers the same client mistake differently from another:
///
/// * `UntagResource` — `TagKeys` (`tagKeys`), whose `TagKeyList` admits the empty list
///   (`@length(min: 0)`), which a query-bound list spells as no values at all;
/// * `BatchDeleteRumMetricDefinitions` — `MetricDefinitionIds` (`metricDefinitionIds`),
///   a list with no `@length`, so the empty list again, and `Destination` (`destination`);
/// * `BatchGetRumMetricDefinitions` — `Destination`;
/// * `DeleteRumMetricsDestination` — `Destination`.
///
/// Source: `smithy_query(query="operation", operation="UntagResource,
/// BatchDeleteRumMetricDefinitions,BatchGetRumMetricDefinitions,DeleteRumMetricsDestination")`
/// against this service's model, which is also where each pattern is read from. Every
/// other operation of this service either binds no `@required` member to the query string
/// at all, or (`ListRumMetricsDestinations`, the metric-definition create and update)
/// binds its required members to the path or the body, so its route always resolves.
const QUERY_UNMATCHED_ROUTES: &[(&str, &str, &str)] = &[
    ("DELETE", "/tags/{ResourceArn}", "UntagResource"),
    (
        "DELETE",
        "/rummetrics/{AppMonitorName}/metrics",
        "BatchDeleteRumMetricDefinitions",
    ),
    (
        "GET",
        "/rummetrics/{AppMonitorName}/metrics",
        "BatchGetRumMetricDefinitions",
    ),
    (
        "DELETE",
        "/rummetrics/{AppMonitorName}/metricsdestination",
        "DeleteRumMetricsDestination",
    ),
];

/// Whether `path` is a request path the modelled `@http.uri` `pattern` matches: the same
/// segments in the same order, a literal segment matching itself and a `{Label}` segment
/// matching ONE non-empty segment — an SDK percent-encodes a non-greedy label, so the
/// label itself carries no `/`, and none of [`QUERY_UNMATCHED_ROUTES`]'s patterns binds a
/// greedy one.
///
/// Segments are read the way the framework's own matcher reads them (leading and trailing
/// `/` ignored, nothing else skipped), or this and the generated table would disagree
/// about which requests the table already resolves.
fn path_matches_pattern(pattern: &str, path: &str) -> bool {
    let mut expected_segments = pattern.trim_matches('/').split('/');
    let mut segments = path.trim_matches('/').split('/');
    loop {
        match (expected_segments.next(), segments.next()) {
            (None, None) => return true,
            (Some(expected), Some(segment)) => {
                let matched = if expected.starts_with('{') && expected.ends_with('}') {
                    !segment.is_empty()
                } else {
                    expected == segment
                };
                if !matched {
                    return false;
                }
            }
            // One path is longer than the other, so they are different routes.
            _ => return false,
        }
    }
}

/// The operation the model binds to `req`'s method and path among
/// [`QUERY_UNMATCHED_ROUTES`], or `None` when `req` addresses none of them.
///
/// The URI this seam is handed is the whole one the SDK sent, so its path comes from the
/// shared reader (`service_common::uri_path`) rather than from a private copy of it.
fn query_unmatched_operation(req: &aws_smithy_runtime_api::http::Request) -> Option<&'static str> {
    let path = service_common::uri_path(req.uri());
    QUERY_UNMATCHED_ROUTES
        .iter()
        .find(|(method, pattern, _)| req.method() == *method && path_matches_pattern(pattern, path))
        .map(|(_, _, operation)| *operation)
}

/// The `com-amazonaws-rum-rum` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The one gate every request this service serves is run through: it announces this
    /// service's media type on the response. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of it.
    announced_media_type: AnnouncedMediaType<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            announced_media_type: AnnouncedMediaType::new(engine),
        }
    }
}

/// This service's envelope in front of one request: the media type every RUM response
/// announces, and the words a request refused on a modelled constraint is stated in. See
/// the module docs.
///
/// It decides nothing else. The operation that serves the request is the one the model's
/// routing table names, reached through its own `run`, so that operation stays the single
/// owner of the request — its telemetry included. The exceptions are the routes the
/// protocol's routing key cannot name ([`QUERY_UNMATCHED_ROUTES`]), which the gate serves
/// from the operations it holds, and a request the framework's own constraint walk refuses,
/// which the gate states in front of the operation because that is the only place the
/// request's own values are still in hand ([`AnnouncedMediaType::stated_refusal`]).
struct AnnouncedMediaType<Eng> {
    /// `UntagResource`, built from the SAME shared engine the generated entry threads
    /// into every operation, so this gate and the rest of the service read one store.
    /// Held rather than looked up because [`ErasedOperation::auth_schemes`] is asked
    /// without a request in hand, and the schemes this gate reports for the route the
    /// routing key cannot name must be that operation's MODELLED ones.
    untag_resource: crate::ops::untag_resource::UntagResource<Eng>,
    /// `BatchDeleteRumMetricDefinitions`, held from that same shared engine for another
    /// route the routing key cannot name ([`QUERY_UNMATCHED_ROUTES`]).
    batch_delete_rum_metric_definitions:
        crate::ops::batch_delete_rum_metric_definitions::BatchDeleteRumMetricDefinitions<Eng>,
    /// `BatchGetRumMetricDefinitions`, held for the same reason: `GET
    /// /rummetrics/{AppMonitorName}/metrics` binds `Destination` to the query string.
    batch_get_rum_metric_definitions:
        crate::ops::batch_get_rum_metric_definitions::BatchGetRumMetricDefinitions<Eng>,
    /// `DeleteRumMetricsDestination`, held for the same reason: `DELETE
    /// /rummetrics/{AppMonitorName}/metricsdestination` binds `Destination` to the query
    /// string.
    delete_rum_metrics_destination:
        crate::ops::delete_rum_metrics_destination::DeleteRumMetricsDestination<Eng>,
}

/// Read one `crate::ops::<module>::<Operation>`'s refusal per line, choosing the arm by the
/// routing key — the operation shape name, which is the key the protocol resolves and the
/// generated routing table is keyed by. EVERY modelled operation is listed, because the
/// rule is the service's and not one operation's: an operation whose input declares no
/// constraint simply never has a violation to state, which [`collected_refusal`] reads off
/// that operation's own input schema. An operation added later is listed here too, or it
/// answers a refusal with the value the framework elided rather than the one its caller
/// sent.
///
/// Each line also names the checks THAT operation applies itself over its own input
/// (`crate::ops::<module>::self_applied`), so the gate can state them beside the walk's own
/// (see the module docs); [`no_self_applied`] is the line for an operation whose input
/// carries no member of a shape this service applies constraints for.
macro_rules! stated_refusal_for_key {
    (
        $key:expr, $protocol:expr, $req:expr, $engine:ident,
        $($module:ident :: $operation:ident => $self_applied:expr),+ $(,)?
    ) => {
        match $key {
            $(
                stringify!($operation) => collected_refusal::<
                    crate::ops::$module::$operation<$engine>,
                >($protocol, $req, $self_applied),
            )+
            _ => None,
        }
    };
}

impl<Eng: Send + Sync + 'static> AnnouncedMediaType<Eng> {
    fn new(engine: Arc<Eng>) -> Self {
        Self {
            untag_resource: crate::ops::untag_resource::UntagResource::new(Arc::clone(&engine)),
            batch_delete_rum_metric_definitions:
                crate::ops::batch_delete_rum_metric_definitions::BatchDeleteRumMetricDefinitions::new(
                    Arc::clone(&engine),
                ),
            batch_get_rum_metric_definitions:
                crate::ops::batch_get_rum_metric_definitions::BatchGetRumMetricDefinitions::new(
                    Arc::clone(&engine),
                ),
            delete_rum_metrics_destination:
                crate::ops::delete_rum_metrics_destination::DeleteRumMetricsDestination::new(engine),
        }
    }

    /// The operation this gate serves a [`QUERY_UNMATCHED_ROUTES`] request from, or `None`
    /// when `req` addresses none of those routes.
    ///
    /// WHICH operation serves which route is the model's answer twice over: the route table
    /// carries the operation the model binds to that method and path, and the operation it
    /// names is matched against each held operation's OWN modelled name
    /// ([`Operation::NAME`]), so this gate cannot serve a request from an operation the
    /// model does not bind to its route.
    fn unmatched_route_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let operation = query_unmatched_operation(req)?;
        let held: [(&'static str, &dyn ErasedOperation); 4] = [
            (
                <crate::ops::untag_resource::UntagResource<Eng> as Operation>::NAME,
                &self.untag_resource,
            ),
            (
                <crate::ops::batch_delete_rum_metric_definitions::BatchDeleteRumMetricDefinitions<
                    Eng,
                > as Operation>::NAME,
                &self.batch_delete_rum_metric_definitions,
            ),
            (
                <crate::ops::batch_get_rum_metric_definitions::BatchGetRumMetricDefinitions<
                    Eng,
                > as Operation>::NAME,
                &self.batch_get_rum_metric_definitions,
            ),
            (
                <crate::ops::delete_rum_metrics_destination::DeleteRumMetricsDestination<
                    Eng,
                > as Operation>::NAME,
                &self.delete_rum_metrics_destination,
            ),
        ];
        held.into_iter()
            .find(|(name, _)| *name == operation)
            .map(|(_, operation)| operation)
    }

    /// How this service states every modelled constraint `req` breaks, with the two facts
    /// a clause needs that a [`Violation`] does not carry, or `None` when the request
    /// breaks none.
    ///
    /// The collecting half is the tree's shared one,
    /// `service_common::validation::refusing_violations_with_lists`: ONE read of the
    /// request yields the framework's own violations — the protocol's deserializer for an
    /// absent `@required` member, `constraints::validate` over the built input — with each
    /// echoed value restored to the bytes the caller sent, plus the request's own lists,
    /// which is the value a clause reported at a LIST member states and no violation
    /// holds. Nothing is re-checked and nothing is refused that the framework's own walk
    /// would not refuse; the cost is that a request this gate does not refuse is read
    /// twice, once here and once in the operation's own sequence, because the framework
    /// hands no already-built input to an operation. The structurally right place for the
    /// collecting half is the frozen crate, which `services/common/src/validation.rs`
    /// records as the core seam this re-read exists only until.
    ///
    /// WHICH operation's input schema the violations are read against is the model's
    /// answer: the routing key the protocol resolves, the same key the generated routing
    /// table is keyed by. A key this service does not serve — and the routes that key
    /// cannot name ([`QUERY_UNMATCHED_ROUTES`]), which bind no body at all — is left to
    /// the operation's own sequence, whose refusal comes back through
    /// [`Service::override_refusal`].
    fn stated_refusal(
        &self,
        svc: &dyn Service,
        req: &RtRequest,
    ) -> Option<(Vec<Violation>, Refused)> {
        let key = svc.definition().protocol().routing_key(req).ok()?;
        let protocol = svc.definition().protocol();
        stated_refusal_for_key!(
            key,
            protocol,
            req,
            Eng,
            batch_create_rum_metric_definitions::BatchCreateRumMetricDefinitions
                => crate::ops::batch_create_rum_metric_definitions::self_applied,
            batch_delete_rum_metric_definitions::BatchDeleteRumMetricDefinitions
                => no_self_applied,
            batch_get_rum_metric_definitions::BatchGetRumMetricDefinitions => no_self_applied,
            create_app_monitor::CreateAppMonitor => crate::ops::create_app_monitor::self_applied,
            delete_app_monitor::DeleteAppMonitor => no_self_applied,
            delete_resource_policy::DeleteResourcePolicy => no_self_applied,
            delete_rum_metrics_destination::DeleteRumMetricsDestination => no_self_applied,
            get_app_monitor::GetAppMonitor => no_self_applied,
            get_app_monitor_data::GetAppMonitorData => no_self_applied,
            get_resource_policy::GetResourcePolicy => no_self_applied,
            list_app_monitors::ListAppMonitors => no_self_applied,
            list_rum_metrics_destinations::ListRumMetricsDestinations => no_self_applied,
            list_tags_for_resource::ListTagsForResource => no_self_applied,
            put_resource_policy::PutResourcePolicy => no_self_applied,
            put_rum_events::PutRumEvents => no_self_applied,
            put_rum_metrics_destination::PutRumMetricsDestination => no_self_applied,
            tag_resource::TagResource => crate::ops::tag_resource::self_applied,
            untag_resource::UntagResource => crate::ops::untag_resource::self_applied,
            update_app_monitor::UpdateAppMonitor => crate::ops::update_app_monitor::self_applied,
            update_rum_metric_definition::UpdateRumMetricDefinition
                => crate::ops::update_rum_metric_definition::self_applied,
        )
    }
}

/// The violations `req` breaks against operation `O`'s own input schema, and the words this
/// service states them in — the shared collection, this service's own checks over the same
/// request, and this service's one wording ([`stated`]).
fn collected_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    self_applied: fn(&O::Input) -> crate::state::validation::SelfApplied,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    if violations.is_empty() {
        // The framework's walk admits this request, so the operation runs and applies the
        // constraints this service applies itself over its own members — the ONE check per
        // request in the normal case. Nothing is read a second time for it.
        return None;
    }
    let (violations, lists) =
        with_self_applied::<O>(protocol, req, self_applied, violations, lists);
    // WHICH frontend words the refusal is the operation's own answer here, because the
    // operation the routing key named is in hand ([`TAGGING_OPERATIONS`]).
    let chosen = stated_by(
        &violations,
        &lists,
        TAGGING_OPERATIONS.contains(&<O as Operation>::NAME),
    )?;
    Some((violations, chosen))
}

/// The checks line of an operation whose input carries no member of a shape this service
/// applies constraints for itself (`crate::state::validation`), so there is nothing beside
/// the boundary walk's violations to state for it.
fn no_self_applied<I>(_input: &I) -> crate::state::validation::SelfApplied {
    crate::state::validation::SelfApplied::default()
}

/// `violations` and `lists` with the constraints THIS service applies itself added: one set
/// per request, so a request that breaks a boundary-applied constraint AND one of this
/// service's own is answered with one clause per failing (member, requirement) instead of
/// the boundary's half alone (`crate::state::validation`, rule 1) — a caller that fixes
/// what it was told about is not refused again on what the first answer withheld.
///
/// Reached only for a request the boundary walk already refuses, which is exactly the case
/// the operation's own checks cannot state themselves: that walk runs in front of the
/// handler, so the handler never sees such a request. A request it admits is left alone
/// here and judged by the handler, which is why this adds no check to the accepting path.
///
/// `self_applied` is the operation's own ([`stated_refusal_for_key`]), applied to the input
/// read against `O`'s own input schema ([`read_input`]) — the same members, from the same
/// schema, the handler would have judged. A body no codec can read builds no input, so the
/// walk's own clauses stand alone.
fn with_self_applied<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    self_applied: fn(&O::Input) -> crate::state::validation::SelfApplied,
    violations: Vec<Violation>,
    mut lists: HashMap<String, String>,
) -> (Vec<Violation>, HashMap<String, String>) {
    let Some(input) = read_input::<O>(protocol, req) else {
        return (violations, lists);
    };
    let found = self_applied(&input);
    if found.is_empty() {
        return (violations, lists);
    }
    let mut both = violations;
    for violation in found.violations {
        // Rule 1 is one clause per failing (member, requirement), and the two layers OVERLAP
        // on one position: a `@length` the walk applies to a member whose `@pattern` only
        // this service can apply is found by both, so the pair the walk already stated is
        // not stated twice.
        let stated_already = both
            .iter()
            .any(|seen| seen.path == violation.path && seen.kind == violation.kind);
        if !stated_already {
            both.push(violation);
        }
    }
    // The lists the checks have, for a clause this service states AT a list member (rule 4);
    // a list the shared collector already rendered keeps that rendering, the two being the
    // same list.
    for (member, rendered) in found.lists {
        lists.entry(member).or_insert(rendered);
    }
    (in_declared_order(O::INPUT_SCHEMA, both), lists)
}

/// `req`'s input as operation `O`'s own input shape, or `None` for a body no codec can read
/// — which is a failure the framework's own walk words.
///
/// The read is the framework's own: the protocol's deserializer over `O::INPUT_SCHEMA`,
/// which binds the URI and query members as well as the body, so the values judged here are
/// the values the operation's sequence would have judged. It is a SECOND read of a request
/// that is already being refused, because the shared collector drops the input it built
/// (`services/common/src/validation.rs`, which records the core seam that would hand one
/// out); no request this gate does not refuse pays for it.
fn read_input<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Option<O::Input> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    read.ok()
}

/// `violations` in the order the input DECLARES the members they are about.
///
/// Two walks found them — the framework's over the built input, and this service's own
/// checks — and the rule is one envelope in the order the walk found them
/// (`crate::state::validation`, rule 1), so the two sets are interleaved rather than
/// concatenated: a clause about a member the input declares earlier is stated earlier,
/// whichever layer applied the constraint. The order is the one the generated input schema
/// lists its members in, which is the model's declaration order and the order the
/// framework's own walk visits them in; the sort is STABLE, so several violations about one
/// member keep the order the walk that found them produced, and one about a position the
/// schema does not list keeps its place after the declared members.
///
/// Capped at the number of violations the framework's own walk collects to
/// (`service_common::validation::MAX_VIOLATIONS`), so a message assembled from two sets is
/// bounded by the same number as one.
fn in_declared_order(schema: &Schema, violations: Vec<Violation>) -> Vec<Violation> {
    let declared: Vec<String> = schema
        .members()
        .iter()
        .filter_map(|member| member.member_name().map(path::member))
        .collect();
    let mut ordered = violations;
    ordered.sort_by_key(|violation| {
        // A violation path is dotted, so the member it is about is its first segment.
        let member = violation.path.split('.').next().unwrap_or_default();
        declared
            .iter()
            .position(|declared| declared == member)
            .unwrap_or(usize::MAX)
    });
    ordered.truncate(service_common::validation::MAX_VIOLATIONS);
    ordered
}

/// How this service words a constraint refusal, wherever it was collected: the ONE place
/// that decision is made, so the refusal a request gets cannot depend on which of the two
/// paths found it ([`AnnouncedMediaType::stated_refusal`] before the operation runs, or
/// [`Service::override_refusal`] after the centralized walk).
///
/// A refused TAG call is worded by that frontend (see [`TAGGING_BAD_REQUEST_CODE`]): its
/// code is `BadRequestException`, and a `ResourceArn` that is not an ARN gets its own
/// sentence for it. Any other tag refusal keeps the framework's request-derived envelope
/// under that code — what that frontend words those in is not known, so nothing about them
/// is reworded. Every other constraint refusal is an app-monitor call's, worded the way
/// this service words one (`crate::state::validation`).
///
/// WHICH call is a tag call is read off the violations, which is all this path knows: the
/// refusal the centralized walk hands back names no operation
/// ([`is_tagging_refusal`]). The gate in front of the operation knows the operation and
/// states it through [`stated_by`], so a refusal about the tag MAP — a member both frontends
/// carry — is worded by the frontend that answers the call rather than by the member.
///
/// `lists` holds the request's lists where the caller has them, keyed by the member path
/// the walk reports that list at and already rendered.
fn stated(violations: &[Violation], lists: &HashMap<String, String>) -> Option<Refused> {
    stated_by(violations, lists, false)
}

/// [`stated`] for a caller that knows whether the operation refused is one the TAGGING
/// frontend answers ([`TAGGING_OPERATIONS`]).
///
/// `tagging` only ADDS what the violations cannot say — an operation of that frontend
/// refused on a member the app-monitor operations also declare — so a refusal the violations
/// already place with that frontend is worded identically either way, and a tag key is
/// stated in one dialect whether the call was refused in front of the operation or by the
/// handler.
fn stated_by(
    violations: &[Violation],
    lists: &HashMap<String, String>,
    tagging: bool,
) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    if tagging || is_tagging_refusal(violations) {
        return Some(Refused {
            code: TAGGING_BAD_REQUEST_CODE.to_string(),
            message: if is_resource_arn_syntax_violation(violations) {
                INVALID_RESOURCE_ARN_MESSAGE.to_string()
            } else {
                aws_smithy_schema_server::constraints::envelope(violations)?
            },
        });
    }
    Some(Refused {
        code: crate::state::validation::VALIDATION_EXCEPTION.to_string(),
        message: crate::state::validation::message(violations, lists)?,
    })
}

impl<Eng: Send + Sync + 'static> ErasedOperation for AnnouncedMediaType<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // A request that breaks a modelled constraint is stated BEFORE the operation
            // runs, because two of the facts such a clause states are only readable from
            // the request itself and not from the violations the centralized walk hands
            // `override_refusal`: the value a caller submitted, unelided, and the whole
            // LIST a clause about one of its elements is reported at (see
            // [`stated_refusal`]). Nothing is re-checked here — the violations are the
            // framework's own — and a request that breaks none is handed straight on, so
            // this cannot refuse a request the framework would accept. The definition is
            // read, used and dropped inside each statement: `&dyn
            // ErasedServiceDefinition` is not `Sync`, so holding one across the await
            // below would make this future non-`Send`.
            let refused = self.stated_refusal(svc, req);
            if let Some((violations, chosen)) = refused {
                let response = svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(violations), Some(chosen));
                return announced(req, response);
            }
            // The generated operation serves the request through the framework's own
            // deserialize -> validate -> handle -> serialize sequence, so a modelled
            // success, a handler's modelled error and a boundary refusal all pass back
            // out through here. Reached through the routing table rather than through
            // `override_operation`, which would route straight back to this gate.
            let response = match modeled_operation(svc, req).or_else(|| {
                // The routes the protocol's routing key cannot name, each served by the
                // operation the model binds to it (see [`QUERY_UNMATCHED_ROUTES`]).
                self.unmatched_route_operation(req)
            }) {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable: this gate is served only for a request that is one of the
                // cases above.
                None => svc.definition().routing_error_response(req),
            };
            announced(req, response)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // Consulted only when the modelled routing key names NO operation — the routes
        // above — because the router prefers the modelled operation's schemes for
        // every request whose key it can resolve. So the schemes reported here are an
        // operation's own modelled ones, read off it rather than written out: a gate
        // must not be able to weaken what the model requires. None of those operations
        // overrides the service's authentication, so the schemes are the same for all of
        // them and any one of them reports them.
        self.untag_resource.auth_schemes()
    }
}

/// Whether this request carried a payload — a body with bytes in it.
///
/// A request that carries one declares its media type (`restJson1` has exactly one for a
/// JSON payload, [`crate::state::JSON_CONTENT_TYPE`]); a request whose members are all
/// bound to the URI, the query string or the headers carries no body and declares nothing.
/// That is the distinction [`announced`] settles the response's media type on, so it is
/// read off the BODY rather than off a `content-type` header a client may send on a
/// payload-less request anyway.
///
/// A body whose bytes are not in memory (a stream of unknown length) is a payload: only a
/// body that is known to be empty is not one.
fn carries_a_payload(req: &RtRequest) -> bool {
    req.body()
        .bytes()
        .is_none_or(|bytes: &[u8]| !bytes.is_empty())
}

/// The response as this service announces it: one media type for every JSON body it
/// produces in answer to a payload-less request, successes and failures alike.
///
/// Only the `restJson1` JSON envelope is re-announced
/// ([`crate::state::JSON_CONTENT_TYPE`] -> [`crate::state::RESPONSE_CONTENT_TYPE`]), and
/// only for a request that declared no media type of its own by carrying no payload (see
/// [`carries_a_payload`] and the module docs). A body that is not that envelope, and an
/// answer to a request that did carry a payload, carries the type its own target shape
/// maps to and is left exactly as the serializer produced it.
fn announced(req: &RtRequest, mut response: RtResponse) -> RtResponse {
    if carries_a_payload(req) {
        return response;
    }
    if response.headers().get("content-type") == Some(crate::state::JSON_CONTENT_TYPE) {
        response
            .headers_mut()
            .insert("content-type", crate::state::RESPONSE_CONTENT_TYPE);
    }
    response
}

/// The operation the model's routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
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

    /// Serve every request this service answers through [`AnnouncedMediaType`], the gate
    /// that announces this service's media type on the response (see the module docs),
    /// including a request whose operation the protocol's routing key cannot name: one
    /// that binds no value for a `@required` `@httpQuery` member of the operation the
    /// model binds to its method and path (see [`QUERY_UNMATCHED_ROUTES`]).
    ///
    /// WHICH operation serves a request is still decided by the model: the gate resolves
    /// it from the generated routing table, exactly as the router would have, and a route
    /// the table cannot name is matched by method and path alone (see
    /// [`QUERY_UNMATCHED_ROUTES`] for why the framework's required-binding rule leaves
    /// those unmatched, and for the member each one binds). The operation then answers its
    /// OWN refusal for the absent member — the service's `ValidationException`, HTTP 400 —
    /// or, where the member is a list whose empty value the wire spells as no values at
    /// all, serves the call (`crate::ops::untag_resource` and
    /// `crate::ops::batch_delete_rum_metric_definitions` read the absent list that way).
    /// A request that names no modelled operation and is none of those routes is left to
    /// the framework, which refuses it in front of this service in its own
    /// protocol-correct envelope.
    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let gate = &self.announced_media_type as &dyn ErasedOperation;
        if modeled_operation(self, req).is_some() || query_unmatched_operation(req).is_some() {
            return Some(gate);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. The words are [`stated`]'s — the one place this service makes that
        // choice — so a refusal that reaches here and one the gate in front of the
        // operation stated ([`AnnouncedMediaType::stated_refusal`]) are worded the same
        // way. What is missing on THIS path is only what the violations do not carry: the
        // request's own lists, so a clause about a list element states the elements that
        // offended rather than the whole list.
        match refusal {
            Refusal::Invalid(violations) => stated(violations, &HashMap::new()),
            // An unauthenticated caller and every other refusal class keep the
            // framework's protocol-correct default.
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

// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig` service: how it claims requests and how it words the
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
//! # How a refused request is worded
//!
//! Two layers answer a refused request, and they do not answer alike.
//!
//! The FRONT DOOR validates the request's JSON BODY against the operation's modelled
//! input before this service's code sees it, and reports a value outside a declared
//! `@pattern`/`@length`, or outside the set an enumerated type admits, as the clause its
//! body-schema validator states for that constraint class, in brackets after
//! [`INVALID_REQUEST_BODY`], under
//! [`BAD_REQUEST_EXCEPTION`] — a shape this model declares for no operation, because the
//! layer is not the service. That is [`service_common::front_door`], which every
//! modelled operation is served behind: the clause has to tell a BODY member from one
//! bound to the URI or the query string, which is read off each operation's own input
//! schema, and it quotes the value the CALLER submitted — neither is reachable from the
//! `override_refusal` seam, which is handed the violations alone.
//!
//! Everything else is this SERVICE's own refusal, and every operation of this model
//! declares `ValidationException` (400) for it — the code the framework's default
//! already names — so it keeps the framework's wording: a `@range` on a `@httpQuery`
//! parameter, an absent member, and the rules the handlers enforce over stored state.

use service_common::front_door::{FrontDoorTable, BAD_REQUEST_EXCEPTION, INVALID_REQUEST_BODY};
use service_common::validation::SubmittedEcho;
use service_common::validation_message::{
    bracketed_findings, enum_value_not_found, pattern_mismatch, string_too_long,
    string_too_short,
};
use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The clause this service's front door states for one body violation, or `None` when it
/// states none.
///
/// It reports a value it READ and then measured against the declared rule: a regex it
/// does not match, a string length outside the declared bounds, or a value outside the set
/// its enumerated type admits — each sentence through
/// [`service_common::validation_message`], which is their home for the whole tree. Every
/// other class (a `@range`, an absent `@required` member) has no recorded sentence here and
/// keeps the framework's own wording rather than being given one. A value with no text
/// rendering is left alone for the same reason: no recorded clause quotes one.
fn clause(violation: &Violation, echo: &SubmittedEcho) -> Option<String> {
    let SubmittedEcho::Rendered(value) = echo else {
        return None;
    };
    match &violation.kind {
        // The expression as the MODEL writes it, and the string the caller submitted.
        ViolationKind::Pattern(pattern) => Some(pattern_mismatch(pattern, value)),
        // `@length` counts CHARACTERS, which is what the constraint walk counted. Every
        // `@length` in this model sits on a string shape — no list or map member of any
        // input declares one (`smithy_query(query="closure", ...)`) — so a length clause
        // here always describes a string.
        ViolationKind::MinLength(min) => Some(string_too_short(value, value.chars().count(), *min)),
        ViolationKind::MaxLength(max) => Some(string_too_long(value, value.chars().count(), *max)),
        // The values the MODEL declares for the enumerated type, in its declared order —
        // which is the order the validating walk carries them in — and the string the
        // caller submitted. The type is reached through the member, so a member at any
        // depth of the body (`AssertionRule.RuleConfig.Type` as much as `NetworkType`) is
        // stated the same way.
        ViolationKind::EnumValue(permitted) => Some(enum_value_not_found(value, permitted)),
        _ => None,
    }
}

/// How this service's front door words a body it refused: the validator's class and its
/// bracketed list of clauses, every variable part — the declared regex or bound, and the
/// value that failed it — taken from the request and the model.
///
/// A member that broke its `@pattern` as well as a `@length` bound is stated once, by the
/// regex clause: the declared pattern is what the validator measures such a value against
/// (`^\S+$` already excludes the empty string that a `@length` minimum of 1 does).
///
/// `None` when any violation is one this layer states no clause for: such a request is
/// left whole to the service's own `ValidationException` rather than reported half in one
/// voice and half in another.
fn body_wording(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    // The paths whose declared pattern already spoke for the value.
    let patterned: std::collections::BTreeSet<&str> = refused
        .iter()
        .filter(|(violation, _)| matches!(violation.kind, ViolationKind::Pattern(_)))
        .map(|(violation, _)| violation.path.as_str())
        .collect();
    let mut clauses: Vec<String> = Vec::new();
    for (violation, echo) in refused {
        // A length bound the same member's pattern already answers for.
        if !matches!(violation.kind, ViolationKind::Pattern(_))
            && patterned.contains(violation.path.as_str())
        {
            continue;
        }
        let stated = clause(violation, echo)?;
        if !clauses.contains(&stated) {
            clauses.push(stated);
        }
    }
    if clauses.is_empty() {
        return None;
    }
    Some(Refused {
        code: BAD_REQUEST_EXCEPTION.to_string(),
        message: format!("{INVALID_REQUEST_BODY} : {}", bracketed_findings(&clauses)),
    })
}

/// The `com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each served behind this
    /// service's front door ([`FrontDoorTable`]) so a request BODY that breaks a modelled
    /// `@pattern`/`@length` or enumerated set is refused by that layer, in its words,
    /// before the operation runs. Keyed by the routing key the protocol resolves — the key
    /// the generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    front_door: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut front_door = FrontDoorTable::worded(body_wording);
        // EVERY modelled operation, so the front door's rule is applied by ONE code path
        // rather than per operation: an operation that binds nothing to the body
        // (`DescribeCluster`, whose only member is its URI label) simply never has a body
        // violation to find, which the door reads off its own input schema. Each is keyed
        // by its own modelled name, which `FrontDoorTable` reads off the operation.
        front_door
            .serve(crate::ops::create_cluster::CreateCluster::new(Arc::clone(&engine)))
            .serve(crate::ops::create_control_panel::CreateControlPanel::new(Arc::clone(&engine)))
            .serve(crate::ops::create_routing_control::CreateRoutingControl::new(Arc::clone(
                &engine,
            )))
            .serve(crate::ops::create_safety_rule::CreateSafetyRule::new(Arc::clone(&engine)))
            .serve(crate::ops::delete_cluster::DeleteCluster::new(Arc::clone(&engine)))
            .serve(crate::ops::delete_control_panel::DeleteControlPanel::new(Arc::clone(&engine)))
            .serve(crate::ops::delete_routing_control::DeleteRoutingControl::new(Arc::clone(
                &engine,
            )))
            .serve(crate::ops::delete_safety_rule::DeleteSafetyRule::new(Arc::clone(&engine)))
            .serve(crate::ops::describe_cluster::DescribeCluster::new(Arc::clone(&engine)))
            .serve(crate::ops::describe_control_panel::DescribeControlPanel::new(Arc::clone(
                &engine,
            )))
            .serve(crate::ops::describe_routing_control::DescribeRoutingControl::new(Arc::clone(
                &engine,
            )))
            .serve(crate::ops::describe_safety_rule::DescribeSafetyRule::new(Arc::clone(&engine)))
            .serve(crate::ops::get_resource_policy::GetResourcePolicy::new(Arc::clone(&engine)))
            .serve(
                crate::ops::list_associated_route53_health_checks::ListAssociatedRoute53HealthChecks::new(
                    Arc::clone(&engine),
                ),
            )
            .serve(crate::ops::list_clusters::ListClusters::new(Arc::clone(&engine)))
            .serve(crate::ops::list_control_panels::ListControlPanels::new(Arc::clone(&engine)))
            .serve(crate::ops::list_routing_controls::ListRoutingControls::new(Arc::clone(&engine)))
            .serve(crate::ops::list_safety_rules::ListSafetyRules::new(Arc::clone(&engine)))
            .serve(crate::ops::list_tags_for_resource::ListTagsForResource::new(Arc::clone(
                &engine,
            )))
            .serve(crate::ops::tag_resource::TagResource::new(Arc::clone(&engine)))
            .serve(crate::ops::untag_resource::UntagResource::new(Arc::clone(&engine)))
            .serve(crate::ops::update_cluster::UpdateCluster::new(Arc::clone(&engine)))
            .serve(crate::ops::update_control_panel::UpdateControlPanel::new(Arc::clone(&engine)))
            .serve(crate::ops::update_routing_control::UpdateRoutingControl::new(Arc::clone(
                &engine,
            )))
            .serve(crate::ops::update_safety_rule::UpdateSafetyRule::new(Arc::clone(&engine)));
        Self {
            entry: crate::ServiceDefinition::new(engine),
            front_door: front_door.finish(),
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — behind this service's
        // front door, so a request BODY outside a modelled `@pattern`/`@length` or
        // enumerated set is refused by that layer, in its words, before the operation
        // runs. A key the
        // model does not name resolves to `None` here exactly as it does in the
        // generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The front door answers every refusal this service has its own wording for
        // before the framework's walk reaches here, and it is the only layer that can:
        // this seam is handed the violations ALONE, with no operation, so it cannot tell
        // a body member from one bound to the URI or the query string. What is left here
        // is the SERVICE's own refusal — a `@range` on a query parameter, an absent
        // member — for which every operation of this model declares
        // `ValidationException` (400), the code the framework's default already names,
        // so it keeps the framework's wording.
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
}

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `serve` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig.json"
    );

    /// How many operations this service's model declares, as a second, independent
    /// statement of its surface: a model that grows or shrinks fails here rather than
    /// quietly leaving the new operation out of the front door.
    const MODELLED_OPERATIONS: usize = 25;

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind this service's front door, under the same key the generated routing table
    /// is keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to the `serve` list fails here instead of silently
    /// answering a body-constraint refusal in the framework's wording — and a key spelled
    /// differently from the generated table's fails here instead of silently never being
    /// reached by `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_behind_the_front_door() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert_eq!(
            operations.len(),
            MODELLED_OPERATIONS,
            "case the model's surface: expected this service's {MODELLED_OPERATIONS} \
             operations, got {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the front door's key spelling is wrong"
            );
            assert!(
                service.front_door.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind the front door; \
                 this one is missing from the `serve` list, so a body that breaks a modelled \
                 constraint is refused in the framework's wording instead of this service's"
            );
        }
    }
}

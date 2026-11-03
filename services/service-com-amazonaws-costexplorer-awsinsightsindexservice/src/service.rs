// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-costexplorer-awsinsightsindexservice` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// What Cost Explorer answers for a `Granularity` outside the modelled enum.
///
/// A FIXED sentence: it states the granularities the service accepts in the service's own
/// words and carries nothing from the request — neither the rejected value, nor the member
/// path, nor the modelled value set — so it is the same sentence for every unsupported
/// granularity and for every operation that takes the member. The boundary validator's
/// generic enum report states all three of those, which is why `override_refusal` reroutes
/// a `granularity` enum refusal to this text.
pub const GRANULARITY_REJECTION_MESSAGE: &str = "Granularity can only be MONTHLY, DAILY or HOURLY";

/// Reword a boundary constraint violation on an enum member this service states in its
/// own fixed words rather than as a constraint report.
///
/// Two kinds of member qualify, and both are answered with a fixed sentence that echoes
/// neither the rejected value nor the member path:
///
/// * a member whose OPERATION admits only a proper subset of the enum shape it targets —
///   the boundary validator can only check the shape, so its report advertises values the
///   operation refuses;
/// * a member the service itself words, where the shape's whole value set is admissible
///   but the service states the rule in its own vocabulary instead of the framework's.
///
/// One row per such member; everything else keeps the framework's wording.
///
/// A row matches on the member path AND on the value set the member was checked against,
/// so the row is tied to the enum shape rather than to a name. The path alone would be
/// enough today, but a same-named member of an UNRELATED enum added later would then
/// silently inherit this service's sentence; the value-set test declines it instead.
fn service_enum_refusal(violation: &Violation) -> Option<Refused> {
    let (code, message) = match (violation.path.as_str(), &violation.kind) {
        // `GetDimensionValuesRequest$Context`: documented as "This can be RESERVATIONS or
        // COST_AND_USAGE", while the `Context` enum also carries `SAVINGS_PLANS`. A value
        // in the enum but outside the operation's set never reaches here (the shape
        // accepts it); the handler refuses that one with the SAME sentence.
        ("context", ViolationKind::EnumValue(checked))
            if narrows(
                &crate::ops::get_dimension_values::ADMISSIBLE_CONTEXTS,
                checked,
            ) =>
        {
            (
                "ValidationException",
                crate::ops::get_dimension_values::CONTEXT_REJECTION_MESSAGE,
            )
        }
        // Any operation's `Granularity` member (`GetCostAndUsage`,
        // `GetCostAndUsageWithResources`, the forecasts, the reservation and savings-plans
        // reports): the whole modelled enum is a value set the service names itself, so a
        // value outside it is refused with Cost Explorer's own sentence.
        ("granularity", ViolationKind::EnumValue(checked))
            if same_values(crate::ops::types::GRANULARITY_VALUES, checked) =>
        {
            ("ValidationException", GRANULARITY_REJECTION_MESSAGE)
        }
        // Every other refusal keeps the framework's generic constraint report.
        _ => return None,
    };
    Some(Refused {
        code: code.to_string(),
        message: message.to_string(),
    })
}

/// Whether `admissible` is a PROPER subset of the `checked` value set the boundary
/// validator reported — i.e. whether the member really is one whose operation narrows the
/// enum shape it was validated against.
fn narrows(admissible: &[&str], checked: &[String]) -> bool {
    checked.len() > admissible.len()
        && admissible
            .iter()
            .all(|value| checked.iter().any(|checked| checked == value))
}

/// Whether the `checked` value set the boundary validator reported is exactly `expected`,
/// as a SET: the validator reports a shape's values in the model's order, which need not
/// be the order the generated value list carries them in.
fn same_values(expected: &[&str], checked: &[String]) -> bool {
    checked.len() == expected.len()
        && expected
            .iter()
            .all(|value| checked.iter().any(|checked| checked == value))
}

/// The `com-amazonaws-costexplorer-awsinsightsindexservice` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it. Cost Explorer keeps the framework's generic
        // `ValidationException` report for almost everything — but not for the enum
        // members it states in its own fixed words: a member whose operation admits
        // only part of the enum shape it targets, where that report would name values
        // the operation refuses, and a member whose rule the service words itself.
        // `service_enum_refusal` holds one row per such member and declines the rest,
        // so the generic validator stays in place for every other operation and
        // member.
        match refusal {
            Refusal::Invalid(violations) => violations.iter().find_map(service_enum_refusal),
            // Authentication refusals, and any refusal kind added later, keep the
            // framework's wording.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The value set the boundary validator reports for the `Context` enum shape
    /// (`smithy_query(query="shape", shape="Context")`).
    fn context_enum() -> Vec<String> {
        ["COST_AND_USAGE", "RESERVATIONS", "SAVINGS_PLANS"]
            .iter()
            .map(|value| value.to_string())
            .collect()
    }

    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: Some("whatever the caller sent".to_string()),
            sensitive: false,
        }
    }

    /// The value set the boundary validator reports for the `Granularity` enum shape, in
    /// the model's own order (`smithy_query(query="shape", shape="Granularity")`) — which
    /// is NOT the order the generated value list carries.
    fn granularity_enum() -> Vec<String> {
        ["DAILY", "MONTHLY", "HOURLY"]
            .iter()
            .map(|value| value.to_string())
            .collect()
    }

    /// The narrowed member is reworded to the operation's fixed sentence.
    #[test]
    fn a_narrowed_enum_member_takes_the_operation_scoped_sentence() {
        let refused = service_enum_refusal(&violation(
            "context",
            ViolationKind::EnumValue(context_enum()),
        ))
        .expect("case \"context against the whole Context enum\": expected a rewording");
        assert_eq!(
            (refused.code.as_str(), refused.message.as_str()),
            (
                "ValidationException",
                crate::ops::get_dimension_values::CONTEXT_REJECTION_MESSAGE
            ),
            "case \"context against the whole Context enum\": got {refused:?}"
        );
    }

    /// A `granularity` member checked against the whole `Granularity` enum is reworded to
    /// the service's own sentence, whatever order the value set arrives in.
    #[test]
    fn a_granularity_member_takes_the_services_own_sentence() {
        let refused = service_enum_refusal(&violation(
            "granularity",
            ViolationKind::EnumValue(granularity_enum()),
        ))
        .expect("case \"granularity against the Granularity enum\": expected a rewording");
        assert_eq!(
            (refused.code.as_str(), refused.message.as_str()),
            ("ValidationException", GRANULARITY_REJECTION_MESSAGE),
            "case \"granularity against the Granularity enum\": got {refused:?}"
        );
    }

    /// Everything else keeps the framework's generic report — including a `context`
    /// member of some UNRELATED enum, which is what keeps this row from spreading to a
    /// same-named member added to another operation later.
    #[test]
    fn every_other_violation_keeps_the_generic_report() {
        let unrelated_enum = ["ALPHA", "BETA", "GAMMA", "DELTA"]
            .iter()
            .map(|value| value.to_string())
            .collect();
        let cases = [
            (
                "a context member of an unrelated enum",
                violation("context", ViolationKind::EnumValue(unrelated_enum)),
            ),
            (
                "a context member checked against exactly the admissible set",
                violation(
                    "context",
                    ViolationKind::EnumValue(
                        crate::ops::get_dimension_values::ADMISSIBLE_CONTEXTS
                            .iter()
                            .map(|value| value.to_string())
                            .collect(),
                    ),
                ),
            ),
            (
                "a context member failing a constraint that is not the enum",
                violation("context", ViolationKind::MaxLength(8)),
            ),
            (
                "another enum member this service does not word itself",
                violation("dimension", ViolationKind::EnumValue(context_enum())),
            ),
            (
                "a granularity member of an unrelated enum",
                violation(
                    "granularity",
                    ViolationKind::EnumValue(
                        ["HOURLY", "WEEKLY"]
                            .iter()
                            .map(|value| value.to_string())
                            .collect(),
                    ),
                ),
            ),
            (
                "a granularity member failing a constraint that is not the enum",
                violation("granularity", ViolationKind::MaxLength(7)),
            ),
            (
                "an absent required member",
                Violation {
                    path: "timePeriod".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
            ),
        ];
        for (label, violation) in cases {
            assert_eq!(
                service_enum_refusal(&violation),
                None,
                "case {label:?}: expected the framework's own wording"
            );
        }
    }
}

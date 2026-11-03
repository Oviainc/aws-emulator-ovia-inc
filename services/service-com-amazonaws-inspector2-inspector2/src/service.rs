// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-inspector2-inspector2` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-inspector2-inspector2` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it. Returning `None` keeps the framework's default
        // wording. The one thing this service words differently is how a
        // `@pattern` requirement is stated; [`pattern_refusal`] is the single place
        // that choice is made, and it answers `None` for every refusal that states
        // no such requirement.
        pattern_refusal(refusal)
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

/// How this service states a refusal of a value one of its modelled `@pattern`s does not
/// admit, or `None` to keep the framework's wording for a refusal that states no such
/// requirement.
///
/// A pattern requirement is stated over the EXPRESSION alone: the member's expression is
/// quoted with the whole-value anchors the model wraps it in removed, and the clause names
/// no value — the requirement is the expression every value of that member must match, not
/// the one value that missed it, so the sentence is the same whatever a caller submitted and
/// the refusal is stable across requests.
///
/// Both of those renderings, the envelope and its error count, the clause, the member path
/// and every other constraint class's requirement belong to the shared home for this
/// wording and to the framework behind it — nothing about them is spelled here, so this
/// service cannot drift from the services that state the same sentence. What this service
/// chooses, and all it chooses, is WHICH clauses state no value: the `@pattern` class,
/// whatever member broke it ([`breaks_a_pattern`]).
///
/// A refusal carrying no pattern violation is left to the framework entirely, so every
/// other class keeps the clause the framework wrote for it, value and all. That guard is
/// deliberately narrow, and it has one consequence worth stating: the OTHER rendering the
/// shared body owns — a collection's `@length` clause, which has no scalar value to quote —
/// is reached only when a pattern violation sits in the same refusal, so such a clause is
/// worded one way alone and the other way beside one. Only a recording of a refusal over a
/// collection of this service's model can settle which of the two it states, and until
/// there is one this stays the conservative choice: the class this issue has a recording
/// for is reworded, and nothing else is.
fn pattern_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if !violations.iter().any(breaks_a_pattern) {
        return None;
    }
    service_common::validation_message::aws_validation_refusal_withholding_violations(
        refusal,
        breaks_a_pattern,
    )
}

/// Whether `violation` is a value outside its member's `@pattern` — the one class this
/// service states without a value.
fn breaks_a_pattern(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::Pattern(_))
}

#[cfg(test)]
mod tests {
    use super::pattern_refusal;
    use aws_smithy_schema_server::{Violation, ViolationKind};
    use smithy_server_runtime::Refusal;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: &str) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some(value.to_owned()),
            sensitive: false,
        }
    }

    /// RULE: a pattern clause states the unanchored expression and no value; a clause of
    /// any other class is the framework's own, value and all.
    #[test]
    fn a_pattern_clause_states_the_expression_and_no_value() {
        let pattern = |path: &str| {
            violation(
                path,
                ViolationKind::Pattern(r"^\d{12}$".to_owned()),
                "not-an-account",
            )
        };
        let length = |path: &str| violation(path, ViolationKind::MinLength(1), "");

        /// One row: the case, the violations the walk collected, and the message stated.
        type Row = (&'static str, Vec<Violation>, Option<&'static str>);

        let rows: [Row; 4] = [
            (
                "one pattern violation",
                vec![pattern("accountId")],
                Some(
                    "1 validation error detected: Value at 'accountId' failed to satisfy \
                     constraint: Member must satisfy regular expression pattern: \\d{12}",
                ),
            ),
            (
                "two members outside their patterns",
                vec![pattern("accountId"), pattern("kmsKeyId")],
                Some(
                    "2 validation errors detected: Value at 'accountId' failed to satisfy \
                     constraint: Member must satisfy regular expression pattern: \\d{12}; Value \
                     at 'kmsKeyId' failed to satisfy constraint: Member must satisfy regular \
                     expression pattern: \\d{12}",
                ),
            ),
            (
                "a pattern beside another class, which keeps the framework's clause",
                vec![length("name"), pattern("accountId")],
                Some(
                    "2 validation errors detected: Value '' at 'name' failed to satisfy \
                     constraint: Member must have length greater than or equal to 1; Value at \
                     'accountId' failed to satisfy constraint: Member must satisfy regular \
                     expression pattern: \\d{12}",
                ),
            ),
            (
                "no pattern violation at all keeps the framework's wording",
                vec![length("name")],
                None,
            ),
        ];

        for (case, violations, expected) in rows {
            let stated = pattern_refusal(&Refusal::Invalid(violations));
            match expected {
                Some(message) => {
                    let stated = stated.unwrap_or_else(|| {
                        panic!("case {case:?}: expected a stated refusal, got none")
                    });
                    assert_eq!(
                        stated.code, "ValidationException",
                        "case {case:?}: unexpected code, got {:?}",
                        stated.code
                    );
                    assert_eq!(
                        stated.message, message,
                        "case {case:?}: expected {message:?}, got {:?}",
                        stated.message
                    );
                }
                None => assert!(
                    stated.is_none(),
                    "case {case:?}: expected the framework's own wording, got {stated:?}"
                ),
            }
        }
    }

    /// RULE: a refusal that is not a constraint refusal keeps the framework's own code and
    /// wording — an unauthenticated caller is answered 403, not 400.
    #[test]
    fn only_a_constraint_refusal_is_reworded() {
        for (case, refusal) in [
            ("an unauthenticated caller", Refusal::Unauthenticated),
            ("no violation at all", Refusal::Invalid(Vec::new())),
        ] {
            let stated = pattern_refusal(&refusal);
            assert!(
                stated.is_none(),
                "case {case:?}: expected the framework's own wording, got {stated:?}"
            );
        }
    }
}

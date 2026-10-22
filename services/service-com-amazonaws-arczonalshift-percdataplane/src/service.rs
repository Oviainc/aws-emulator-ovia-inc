// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-arczonalshift-percdataplane` service: how it claims requests and how it words the
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
//! - `override_refusal` — EDITED HERE. A request refused for violating a modelled
//!   constraint is stated the way ARC states it: the framework's own envelope and
//!   requirement, with the SUBMITTED VALUE left out of the clause and a `@pattern`
//!   reported without the anchors a model writes to demand a whole-string match. See
//!   "How this service words a refusal" below.
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
//! # How this service words a refusal
//!
//! The envelope and the requirement stay the framework's — `N validation error(s)
//! detected: Value at '<member path>' failed to satisfy constraint: <requirement>`,
//! rendered by [`aws_smithy_schema_server::constraints::Violation::message`] — so a
//! constraint class the framework gains later cannot arrive with a sentence of this
//! service's own. Two parts of the clause are ARC's, and both are rules rather than
//! one recorded string:
//!
//! * the SUBJECT names the member and the requirement it failed and does NOT quote the
//!   submitted value back ([`clause`]): the refusals recorded from this service — a
//!   `zonalShiftId` outside `ZonalShiftId`'s `@pattern`, a `status` outside
//!   `ZonalShiftStatus`'s value set — both state `Value at '<member>'` with no echo, and
//!   an absent member keeps `Value null at '<member>'`, which is the only clause with no
//!   value to leave out;
//! * a `@pattern` is reported as the expression a value must satisfy, WITHOUT the
//!   `^`/`$` anchors the model writes to make the match total ([`unanchored`]): that
//!   anchoring is how Smithy turns a find into a whole-string match, not part of the
//!   expression a client shows a user or matches on.
//!
//! What stays the framework's, deliberately: the ORDER an enum's admissible set is
//! listed in is the model's declaration order (`ZonalShiftStatus` =
//! `[ACTIVE, EXPIRED, CANCELED]`). A recorded ARC refusal lists that set in another
//! order, which no documented or model-stated rule produces and nothing in the request
//! determines, so there is no rule to implement — only one recording's text to copy, and
//! the set this emulator reports is the set the model declares.
//!
//! Both rules above are the SAME two rules ~24 other service crates in this tree carry
//! inlined (`grep -rln 'fn unanchored' services/*/src`), and
//! `services/common/src/validation.rs` ("Still to migrate here") is where they belong.
//! Migrating all of them is one change of its own rather than a side effect of this
//! service's issue, so they are inlined here with this note, exactly as that module asks.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The wire error code for a request this service refuses as invalid: the shape
/// `com.amazonaws.arczonalshift#ValidationException`, which every operation of this
/// service declares and whose `@httpError` is 400.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors a model may spell it with.
///
/// Only a `^…$` PAIR bracketing the whole expression is dropped: `^` alone, or `$`
/// alone, is an expression anchored at one end — a different language, not an anchored
/// spelling of this one — and an anchor left INSIDE after the pair is removed means the
/// pair bracketed one branch of an alternation (`^a$|^b$`) rather than the whole
/// expression, so such a pattern is reported exactly as the model spells it. A `$`
/// escaped by a backslash is a literal dollar sign, not the end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    let Some(body) = without_end_anchor(body) else {
        return pattern;
    };
    if body.contains('^') || body.contains('$') {
        return pattern;
    }
    body
}

/// `pattern` without its end anchor, or `None` when it carries none. A trailing `$`
/// preceded by an ODD number of backslashes is escaped — a literal dollar sign inside
/// the expression — so it is not an anchor.
fn without_end_anchor(pattern: &str) -> Option<&str> {
    let body = pattern.strip_suffix('$')?;
    let escapes = body.len() - body.trim_end_matches('\\').len();
    (escapes % 2 == 0).then_some(body)
}

/// The same violation with a `@pattern` reported unanchored; every other class is
/// untouched, so the requirement each one words stays the framework's.
fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(unanchored(pattern).to_string()),
            value: violation.value.clone(),
        },
        _ => violation.clone(),
    }
}

/// One violation's clause as ARC states it: the framework's own clause with the
/// submitted value lifted out of its opening, so `Value '<what was sent>' at` becomes
/// `Value at`.
///
/// The replaced text is built from the SAME two fields the framework's renderer
/// interpolated, so it matches whatever the value holds — including the elided rendering
/// a long value is stored as — and the one replacement is confined to this clause's own
/// opening. A violation with no value to echo (an absent `@required` member) is left
/// exactly as the framework words it: `Value null at '<member>' …`.
fn clause(violation: &Violation) -> String {
    let violation = as_stated(violation);
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

/// `violations` as this service states them, or `None` when there is nothing to state.
///
/// The envelope is the framework's own
/// ([`aws_smithy_schema_server::constraints::envelope`]): the violation count, then the
/// clauses joined by `"; "`. It is rebuilt here rather than post-processed because the
/// CLAUSES are this service's, and because rewriting a whole rendered envelope once per
/// violation would cost the product of the two — both of which a request controls
/// through the number of members it puts outside their constraints. Each clause is
/// touched exactly once, so the wording is linear in what the walk collected (which the
/// framework caps at 100 violations and 256 echoed characters each).
fn stated(violations: &[Violation]) -> Option<String> {
    match violations {
        [] => None,
        [only] => Some(format!("1 validation error detected: {}", clause(only))),
        many => Some(format!(
            "{} validation errors detected: {}",
            many.len(),
            many.iter().map(clause).collect::<Vec<_>>().join("; ")
        )),
    }
}

/// The `com-amazonaws-arczonalshift-percdataplane` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) to let this service
        // reword it. ARC states such a refusal in the framework's own envelope with the
        // submitted value left out of the clause and a `@pattern` reported without its
        // anchors — see "How this service words a refusal" in the module docs. The code
        // stays `ValidationException`, which is both what the framework would answer and
        // the shape this service's model declares (`@httpError` 400), because a client
        // branches on the code and `Refused` replaces the code and the message together.
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

#[cfg(test)]
mod tests {
    use super::{clause, stated, unanchored};
    use smithy_server_runtime::{Violation, ViolationKind};

    /// The anchoring a model spells a `@pattern` with is dropped only when the `^…$`
    /// pair brackets the WHOLE expression; every other form is the expression itself and
    /// is reported as the model writes it. The two anchored rows are this service's own
    /// patterns (`ZonalShiftId`, `ExpiresIn`); the rest are the forms the rule has to
    /// leave alone, which this service's model does not happen to contain.
    #[test]
    fn a_pattern_loses_only_a_whole_expression_anchor_pair() {
        let cases = [
            ("an anchored character class", "^[A-Za-z0-9-]+$", "[A-Za-z0-9-]+"),
            (
                "an anchored group sequence",
                "^([1-9][0-9]*)(m|h)$",
                "([1-9][0-9]*)(m|h)",
            ),
            ("anchored at the start only", "^arn:", "^arn:"),
            ("anchored at the end only", "[0-9]+$", "[0-9]+$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            ("a fully anchored alternation", "^a$|^b$", "^a$|^b$"),
            ("an escaped trailing dollar", r"^cost\$", r"^cost\$"),
            (
                "an escaped dollar before the anchor",
                r"^cost\\$",
                r"cost\\",
            ),
            ("the empty expression", "", ""),
        ];
        for (case, modelled, expected) in cases {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A violation carrying a value is stated without it; one with none keeps the
    /// framework's `Value null at …`, and the count and joining are the framework's.
    #[test]
    fn a_clause_states_the_member_and_the_requirement_without_the_value() {
        let outside_pattern = Violation {
            path: "zonalShiftId".to_string(),
            kind: ViolationKind::Pattern("^[A-Za-z0-9-]+$".to_string()),
            value: Some("bad_id_1".to_string()),
        };
        let absent = Violation {
            path: "comment".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
        };
        assert_eq!(
            clause(&outside_pattern),
            "Value at 'zonalShiftId' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: [A-Za-z0-9-]+",
            "a present value is left out of the clause and the pattern is unanchored"
        );
        assert_eq!(
            clause(&absent),
            "Value null at 'comment' failed to satisfy constraint: Member must not be null",
            "an absent member has no value to leave out"
        );
        assert_eq!(stated(&[]), None, "nothing to state");
        assert_eq!(
            stated(std::slice::from_ref(&outside_pattern)).as_deref(),
            Some(
                "1 validation error detected: Value at 'zonalShiftId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+"
            ),
            "one violation takes the singular envelope"
        );
        assert_eq!(
            stated(&[outside_pattern, absent]).as_deref(),
            Some(
                "2 validation errors detected: Value at 'zonalShiftId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+; \
                 Value null at 'comment' failed to satisfy constraint: Member must not be null"
            ),
            "several violations are counted and joined"
        );
    }
}

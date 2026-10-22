// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator` service: how it claims requests and how it words the
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
//! ## This service's refusal dialect
//!
//! `override_refusal` below is edited: a modeled-constraint violation keeps the
//! framework's code and status (`ValidationException`, HTTP 400) but is WORDED the way
//! this service words it — one clause per violation inside the AWS envelope
//! (`N validation error(s) detected: …`), each naming the member path and the violated
//! constraint and nothing of the value that was rejected, and quoting a `@pattern` as
//! the expression a value has to satisfy without the `^` / `$` anchors a model writes
//! to make that match total. The framework's default clause does the opposite on both
//! points (it interpolates the rejected value and quotes the pattern verbatim), so the
//! choice is made here, once, for every operation and every constrained member of this
//! service rather than at a call site. The suite
//! `tests/all/validation_refusal_framing.rs` pins both halves.
//!
//! STILL TO MIGRATE, carrying the note `services/common/src/validation.rs` asks every
//! new copy to carry: the requirement half, the no-value subject half and the
//! unanchored `@pattern` below are NOT a dialect — they are the same three rules that
//! module's header stages for itself, already inlined in ~24 service crates
//! (`grep -rln 'fn unanchored' services/*/src`), each pinned by its own recordings and
//! tests. This crate inlines them a further time because moving them is one change of
//! its own rather than a side effect of this service's issue; the fix that owns that
//! migration moves the rules into `services/common` and calls them from here. What
//! stays with this service either way is how the clauses are counted, ordered and
//! joined into the envelope, and the fact that this service echoes no submitted value.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the modeled
/// `ValidationException`, which this service declares as a common error on EVERY
/// operation (smithy_query `query="service_info"`: `common_errors` includes
/// `ValidationException`, `errorKind` client, `@httpError 400`). Restated here because
/// [`Service::override_refusal`] replaces the code and the message together, and it is
/// the same code the framework would have chosen.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Render one constraint violation the way this service states it: the member path and
/// the violated constraint, and NOTHING of the value that was rejected.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
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
        // The requirement reported is the expression itself, without the `^` / `$` the
        // model spells it with: the service states the pattern a value has to satisfy,
        // not the anchoring the model uses to make that match total. That is RECORDED
        // behaviour for this service — the recorded AWS refusal of a pattern-violating
        // name quotes `[a-zA-Z0-9-]+` where the model declares `^[a-zA-Z0-9-]+$` — not
        // a local preference, so a client that recompiles the quoted expression gets
        // the same regex object the real service quoted. A client re-validating with
        // it still has to anchor the match itself, exactly as it would against AWS.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    // An absent member has no submitted value at all, so it is reported as the null it
    // was: that is a fact of the request's SHAPE, not a rendering of a value the caller
    // sent. Every other violation names the member only — the submitted value is never
    // echoed back into the 400 body, so a caller's own input cannot reach a log through
    // this service's error text.
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// A `@pattern` expression without the pair of anchors a model spells a whole-string
/// match with. Only a BOTH-ends anchored expression is unwrapped: a model that anchored
/// one end alone states a prefix or suffix rule, and dropping that anchor would widen
/// the language the expression describes. A `$` escaped by a backslash is a literal
/// dollar sign rather than the end anchor, so it is not one of the pair.
///
/// Stripping is positional, so it is only safe while the anchors it removes are the
/// WHOLE expression's: an alternation whose branches are individually anchored
/// (`^a$|^b$`) would otherwise become a different regex (`a$|^b`). An expression left
/// holding an anchor of its own after the strip is therefore returned as the model
/// spells it, rather than rewritten into something that matches a different language; a
/// `^` inside a character class is a negation, not an anchor, and does not trigger that.
/// No member of this service's model has the alternation shape today; the guard is what
/// keeps the helper correct for the next one.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    let Some(trimmed) = body.strip_suffix('$').filter(|rest| unescaped_tail(rest)) else {
        return pattern;
    };
    if has_interior_anchor(trimmed) {
        pattern
    } else {
        trimmed
    }
}

/// Whether the character following `head` was NOT escaped by a backslash: an even run of
/// trailing backslashes is a run of escaped backslashes, so the next character stands on
/// its own.
fn unescaped_tail(head: &str) -> bool {
    (head.len() - head.trim_end_matches('\\').len()).is_multiple_of(2)
}

/// Whether `expression` still carries an anchor of its own — an `^` or `$` that is
/// neither escaped nor inside a character class — after the outer pair was stripped.
///
/// A class is skipped because `^` is a NEGATION there (`[^a-z]`) and `$` an ordinary
/// character, so neither anchors anything and an expression built from one must still
/// have its outer anchors removed.
fn has_interior_anchor(expression: &str) -> bool {
    let mut escaped = false;
    let mut in_class = false;
    for ch in expression.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '^' | '$' if !in_class => return true,
            _ => {}
        }
    }
    false
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, joined with
/// `; ` in the order the validating walk found them.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator` emulated service: the generated [`ServiceDefinition`] plus
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
        // service reword it.
        //
        // This service states a constraint violation WITHOUT the value that violated
        // it: the message names the member path and the constraint only, and quotes a
        // `@pattern` as the expression itself rather than the anchored form the model
        // writes. The code stays the modeled `ValidationException` (HTTP 400), which is
        // what the framework would have answered; only the wording changes.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `@pattern` this service's model declares, as the model spells it, paired
    /// with the expression a clause has to quote. Read with `smithy_query`
    /// (`query="shape"` on `WorkloadEstimateName`, `ResourceId`, `Arn`, `ClientToken`,
    /// `NextPageToken`, `Tags` — whose KEY is `^[\w\s:+=@/-]+$` and whose VALUE is the
    /// same class with `*` — and `AccountId`).
    #[test]
    fn a_modelled_pattern_is_quoted_without_its_outer_anchors() {
        let cases = [
            // `WorkloadEstimateName` / `BillScenarioName` / `BillEstimateName`.
            ("^[a-zA-Z0-9-]+$", "[a-zA-Z0-9-]+"),
            // `ResourceId`.
            (
                "^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$",
                "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}",
            ),
            // `Arn`.
            (
                "^arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+$",
                "arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+",
            ),
            // `ClientToken`.
            (r"^[\u0021-\u007E]+$", r"[\u0021-\u007E]+"),
            // `NextPageToken`.
            (r"^[\S\s]*$", r"[\S\s]*"),
            // `ResourceTagKey`, the `Tags` map's key.
            (r"^[\w\s:+=@/-]+$", r"[\w\s:+=@/-]+"),
            // `ResourceTagValue`, its value: the same class, `*` rather than `+`.
            (r"^[\w\s:+=@/-]*$", r"[\w\s:+=@/-]*"),
            // `AccountId`.
            (r"^\d{12}$", r"\d{12}"),
        ];
        for (declared, quoted) in cases {
            let actual = unanchored(declared);
            assert_eq!(
                actual, quoted,
                "case {declared:?}: expected {quoted:?}, got {actual:?}"
            );
        }
    }

    /// The cases the positional strip must NOT rewrite, and the ones it still must: an
    /// expression whose own anchors are not the outer pair, or that anchors one end
    /// only, keeps the model's spelling, while an escaped `$` and a negated character
    /// class are ordinary content.
    #[test]
    fn an_expression_whose_anchors_are_not_the_outer_pair_is_left_alone() {
        let cases = [
            // Individually anchored alternation: stripping would change the language.
            ("^a$|^b$", "^a$|^b$"),
            ("^(^a)$", "^(^a)$"),
            // A trailing escaped `$` is a literal dollar, so the expression anchors its
            // START only: a prefix rule, which dropping the `^` would widen.
            (r"^cost\$", r"^cost\$"),
            ("suffix$", "suffix$"),
            // `^` inside a class is a negation, not an anchor.
            ("^[^ ]+$", "[^ ]+"),
            (r"^[\]^]+$", r"[\]^]+"),
            // No anchors at all: quoted exactly as declared.
            ("[0-9]+", "[0-9]+"),
            // An escaped backslash before the end anchor: the `$` is still the anchor.
            (r"^a\\$", r"a\\"),
        ];
        for (declared, quoted) in cases {
            let actual = unanchored(declared);
            assert_eq!(
                actual, quoted,
                "case {declared:?}: expected {quoted:?}, got {actual:?}"
            );
        }
    }

    /// The envelope's counting and joining, which is this service's own dialect.
    #[test]
    fn the_envelope_counts_and_joins_its_clauses() {
        let violation = |path: &str, kind| Violation {
            path: path.to_string(),
            kind,
            // The framework attaches the rejected value; this service drops it, which is
            // what these cases pin.
            value: Some("submitted".to_string()),
        };
        let one = [violation("name", ViolationKind::MaxLength(64))];
        assert_eq!(
            constraint_envelope(&one).as_deref(),
            Some(
                "1 validation error detected: Value at 'name' failed to satisfy \
                 constraint: Member must have length less than or equal to 64"
            ),
            "one violation: got {:?}",
            constraint_envelope(&one)
        );

        let two = [
            violation("name", ViolationKind::MaxLength(64)),
            Violation {
                path: "identifier".to_string(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
            },
        ];
        assert_eq!(
            constraint_envelope(&two).as_deref(),
            Some(
                "2 validation errors detected: Value at 'name' failed to satisfy \
                 constraint: Member must have length less than or equal to 64; \
                 Value null at 'identifier' failed to satisfy constraint: Member must \
                 not be null"
            ),
            "two violations: got {:?}",
            constraint_envelope(&two)
        );

        assert_eq!(
            constraint_envelope(&[]),
            None,
            "no violations: the framework's own wording stands"
        );
    }
}

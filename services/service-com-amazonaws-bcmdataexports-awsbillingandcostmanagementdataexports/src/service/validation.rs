// SPDX-License-Identifier: CC0-1.0

//! How this service states a request the boundary validator refused, in the modelled
//! `ValidationException`.
//!
//! The framework walks every modelled input constraint and hands the result over
//! STRUCTURALLY — one [`Violation`] per offending value, carrying the path it sits at,
//! what it failed and what it held — and leaves the wording to the service, because no
//! model records it. This module is that wording for every modelled input constraint of
//! this service: ONE formatter, reached by every boundary refusal this service answers,
//! so no operation and no member can be worded its own way.
//!
//! The rules it implements, as AWS states a constraint violation:
//!
//! * a clause names the offending value, the MEMBER PATH it sits at and the CONSTRAINT
//!   that member violated — `Value '<value>' at '<path>' failed to satisfy constraint:
//!   <requirement>`. An absent member has no value to quote and is reported as the `null`
//!   it was;
//! * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors a model
//!   spells it with to demand a whole-string match: a client that lifts the advertised
//!   expression out of the message to repair its request, or asserts on it, must see the
//!   expression the service advertises and not the model's anchored source;
//! * the clauses are counted and joined into the AWS envelope — `1 validation error
//!   detected: <clause>` for one, `N validation errors detected: <clause>; <clause>` for
//!   several — in the order the walk found them.
//!
//! Nothing here is keyed to an operation, a member name or a value: the rule is applied
//! to whatever [`Violation`] the walk produces, so it holds for every constrained member
//! of every operation of this service — `Export.Name`'s pattern and length, the `Arn`
//! patterns of every ARN-addressed operation, `MaxResults`' range, `ResourceTags`' and
//! `ResourceTagKeys`' length bounds alike.
//!
//! A violation class this module has no wording for declines the WHOLE refusal, which
//! keeps the framework's own sentence rather than inventing one: [`ViolationKind`] is
//! `#[non_exhaustive]`, so a class can be added upstream.
//!
//! The rule above is inlined in ~24 other service crates (`grep -rln 'fn unanchored'
//! services/*/src`); `services/common/src/validation.rs` is the staged home for it, and
//! this module now CALLS its `unanchored` rather than carrying a copy — the crate depends
//! on `service-common` for the IAM evaluation anyway. What stays here is the envelope and
//! the requirement wording, which that header calls a service DIALECT.
//!
//! That crate's OTHER half — `with_submitted_values`, which restores a value the
//! framework elided (`… [n bytes]`) to the bytes the caller sent — is NOT reachable from
//! here: it needs the BUILT INPUT and its operation's schema to re-walk, and
//! `override_refusal` is handed the violations alone (no operation, no request). It is
//! callable only from a per-operation wrapper, as `services/common/src/front_door.rs` is;
//! this service serves its operations unwrapped, so an elided echo stands until the rules
//! and that wrapper move together.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// The error code this service answers an input-constraint violation with: its own
/// modelled `ValidationException`, `@error("client")` with `httpError 400`
/// (`smithy_query(query="shape",shape="ValidationException")`). Restated here because
/// `override_refusal` replaces the code and the message TOGETHER.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How many violations one refusal states. The bound mirrors the boundary validator's own
/// (`MAX_VIOLATIONS` in `aws_smithy_schema_server::constraints`), which is private there,
/// so a caller cannot turn a small request into an arbitrarily long refusal. If upstream
/// ever raises its own cap this one simply stays the tighter of the two, which is the safe
/// direction.
const MAX_VIOLATIONS: usize = 100;

/// A `@pattern` expression without the anchors a model spells it with.
///
/// The ONE implementation in the tree: `services/common/src/validation.rs`, which this
/// crate already depends on for the IAM evaluation. It is the stricter rendering — it
/// keeps an escaped `$`, and keeps a `^`/`$` pair that governs only one branch of an
/// alternation rather than the whole expression.
fn unanchored(pattern: &str) -> String {
    ::service_common::validation::unanchored(pattern)
}

/// The requirement a violation states, worded as AWS words it. `None` for a class this
/// service has no wording for.
fn requirement(kind: &ViolationKind) -> Option<String> {
    let text = match kind {
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
    Some(text)
}

/// One violation as the clause that goes into the envelope, or `None` for a class this
/// service has no wording for.
///
/// The value is quoted back as the walk rendered it — AWS's own clause echoes the
/// offending input, which is the only place the server reports it back — while an absent
/// member is reported as the `null` it was, a fact of the request's shape rather than a
/// value the caller sent.
pub(crate) fn clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let path = &violation.path;
    Some(match &violation.value {
        Some(shown) => {
            format!("Value '{shown}' at '{path}' failed to satisfy constraint: {requirement}")
        }
        None => format!("Value null at '{path}' failed to satisfy constraint: {requirement}"),
    })
}

/// The `message` of a refused request: the AWS envelope over one clause per violation, in
/// the order the walk found them. `None` declines the whole refusal — see [`clause`].
pub(crate) fn envelope(violations: &[Violation]) -> Option<String> {
    let clauses = violations
        .iter()
        .map(clause)
        .collect::<Option<Vec<String>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a different
/// failure with a different code and status, and nothing about it is this service's to
/// word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(&violations[..violations.len().min(MAX_VIOLATIONS)])?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchor stripping, over the forms a `@pattern` takes. The forms this service's
    /// own model spells are covered end-to-end by
    /// `tests/all/validation_message_rendering.rs`; these pin the ones no input shape of
    /// THIS model spells today, so the rule stays stated for a model that grows one.
    ///
    /// The shared implementation is the STRICTER rendering: it drops the `^`/`$` pair only
    /// when BOTH ends carry it and the pair wraps the whole expression, because an
    /// expression anchored at one end is a different expression rather than an anchored
    /// spelling of this one.
    #[test]
    fn anchors_are_stripped_and_an_escaped_dollar_is_kept() {
        for (pattern, expected) in [
            // The two forms this model spells: `Export$Name` and every `GenericString`.
            (r"^[0-9A-Za-z\-_]+$", r"[0-9A-Za-z\-_]+"),
            (r"^[\S\s]*$", r"[\S\s]*"),
            // Unanchored already, and anchored at one end only — the one-sided forms are
            // emitted verbatim.
            (r"[a-z]+", "[a-z]+"),
            (r"^[a-z]+", "^[a-z]+"),
            (r"[a-z]+$", "[a-z]+$"),
            (r"^", "^"),
            // An escaped `$` is a literal character of the expression, so the pair is not
            // a pair and nothing is dropped.
            (r"^\$", r"^\$"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            (r"^[a-z]\\$", r"[a-z]\\"),
            (r"^\$[0-9]+$", r"\$[0-9]+"),
        ] {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A clause quotes the value, the member path and the requirement; an absent member
    /// keeps the `null` the request's shape states.
    #[test]
    fn a_clause_states_the_value_the_member_and_the_requirement() {
        let cases = [
            (
                Violation {
                    path: "export.name".to_string(),
                    kind: ViolationKind::Pattern(r"^[0-9A-Za-z\-_]+$".to_string()),
                    value: Some("bad name!".to_string()),
                },
                "Value 'bad name!' at 'export.name' failed to satisfy constraint: Member \
                 must satisfy regular expression pattern: [0-9A-Za-z\\-_]+",
            ),
            (
                Violation {
                    path: "resourceTags".to_string(),
                    kind: ViolationKind::MaxLength(200),
                    value: Some("201 items".to_string()),
                },
                "Value '201 items' at 'resourceTags' failed to satisfy constraint: Member \
                 must have length less than or equal to 200",
            ),
            (
                Violation {
                    path: "export.name".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                },
                "Value null at 'export.name' failed to satisfy constraint: Member must \
                 not be null",
            ),
        ];
        for (violation, expected) in cases {
            let actual = clause(&violation).expect("a modelled violation class is worded");
            assert_eq!(
                actual, expected,
                "case {violation:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The envelope counts and joins the clauses the AWS way.
    #[test]
    fn the_envelope_counts_the_clauses() {
        let one = vec![Violation {
            path: "maxResults".to_string(),
            kind: ViolationKind::MaxValue("300".to_string()),
            value: Some("301".to_string()),
        }];
        let two = vec![
            one[0].clone(),
            Violation {
                path: "exportArn".to_string(),
                kind: ViolationKind::MinLength(20),
                value: Some("arn:aws".to_string()),
            },
        ];
        for (violations, expected) in [
            (
                &one,
                "1 validation error detected: Value '301' at 'maxResults' failed to \
                 satisfy constraint: Member must have value less than or equal to 300",
            ),
            (
                &two,
                "2 validation errors detected: Value '301' at 'maxResults' failed to \
                 satisfy constraint: Member must have value less than or equal to 300; \
                 Value 'arn:aws' at 'exportArn' failed to satisfy constraint: Member must \
                 have length greater than or equal to 20",
            ),
        ] {
            let actual = envelope(violations).expect("a worded refusal");
            assert_eq!(
                actual, expected,
                "case {violations:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

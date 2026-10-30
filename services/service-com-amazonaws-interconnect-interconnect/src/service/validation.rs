// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The framework walks the modelled constraints and hands [`Refusal`] to `service.rs`
//! STRUCTURALLY — one [`Violation`] per offending value, carrying the path it sits at,
//! what it failed and what it held — and leaves the wording to the service, because no
//! model records it. This module is that wording for every modelled input constraint of
//! this service: one formatter, reached by every operation's boundary validation, so no
//! operation can word a refusal its own way.
//!
//! The rule it implements, as this service states a rejection:
//!
//! * the clause names the offending MEMBER PATH and the CONSTRAINT that member
//!   violated, and NOTHING of the value the caller sent. An absent member is the one
//!   exception, reported as the `null` it was — a fact of the request's shape, not a
//!   rendering of a value. The value is wrong to echo whatever the member is: this
//!   service models `ActivationKey` `@sensitive`, so reproducing caller input in an
//!   error body is the wrong default for the whole service, and the clause's job is to
//!   say what the request must satisfy, which the value does not help with;
//! * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors the
//!   model spells it with: a caller that lifts the advertised expression out of the
//!   message and re-applies it locally must get the character class the service
//!   advertises.
//!
//! Nothing here is keyed to an operation, a member name or a value: the rule is applied
//! to whatever [`Violation`] the walk produces, so it holds for every constrained member
//! of every operation in this service — the `@range` on a `MaxResults`, the
//! `@length`/`@pattern` on a connection id, an ARN, a description or a bandwidth, and
//! the constraints of a tag key or value alike.
//!
//! A violation class this module has no wording for declines the WHOLE refusal
//! (`None`), which keeps the framework's default sentence rather than inventing one:
//! [`ViolationKind`] is `#[non_exhaustive]`, so a class can be added upstream.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// The error code this service answers an input-constraint violation with, which is the
/// code the framework's own default names for one too — restated because
/// `override_refusal` replaces code and message TOGETHER, so a reworded message must
/// carry the code a client already branches on.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The requirement a violation states, worded as this service words it. `None` for a
/// class it has no wording for.
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

/// A `@pattern` expression without the anchors the model spells it with.
///
/// A `$` preceded by an odd number of backslashes is an ESCAPED dollar sign — a literal
/// character of the expression, not the end anchor — so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let escapes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if escapes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// One violation as a clause of the envelope, or `None` for a class this service has no
/// wording for.
fn clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    // The value is never rendered. An absent member is reported as the `null` it was,
    // which is the shape of the request rather than a value the caller sent.
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// The message for a refused request: the envelope (`N validation error(s) detected: `)
/// over one clause per violation, in the order the walk found them.
///
/// One clause per violation, and the count is the number of violations detected — the
/// framework's own count, so a caller counting the clauses and a caller reading the
/// number see the same thing. `None` declines the whole refusal — see [`clause`].
pub(crate) fn envelope(violations: &[Violation]) -> Option<String> {
    let mut clauses: Vec<String> = Vec::with_capacity(violations.len());
    for violation in violations {
        clauses.push(clause(violation)?);
    }
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
/// Only an input-constraint refusal is reworded. An authentication refusal is a
/// different failure with a different code and status, and nothing about it is this
/// service's to word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(violations)?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchor forms this service's own patterns reach are covered end-to-end by
    /// `tests/all/client_fault_status_and_validation_wording.rs`. These pin the branches
    /// no input shape of THIS model can produce, so the rule stays stated for a model
    /// that grows one: a `$` that is part of the expression, and an expression with no
    /// anchors at all.
    #[test]
    fn an_escaped_dollar_is_part_of_the_expression() {
        for (pattern, expected) in [
            (r"^\d+[MG]bps$", r"\d+[MG]bps"),
            (r"[a-z]+", "[a-z]+"),
            (r"^\$[0-9]+$", r"\$[0-9]+"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            (r"^[a-z]\\$", r"[a-z]\\"),
            (r"^\$", r"\$"),
            (r"^", ""),
        ] {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

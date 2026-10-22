// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The framework walks the modelled constraints and hands [`Refusal`] to
//! `service.rs` STRUCTURALLY — one [`Violation`] per offending value, carrying
//! the path it sits at, what it failed and what it held — and leaves the wording
//! to the service, because no model records it. This module is that wording for
//! every modelled input constraint of this service: one formatter, reached by
//! every operation's boundary validation, so no operation can word a refusal its
//! own way.
//!
//! The rule it implements, as WorkSpaces Instances states a rejection:
//!
//! * the clause names the offending MEMBER PATH and the CONSTRAINT that member
//!   violated, and NOTHING of the value the caller sent. An absent member is the
//!   one exception, reported as the `null` it was — a fact of the request's
//!   shape, not a rendering of a value. Echoing the value back is wrong here
//!   whatever the member: this service models `NextToken` and `ClientToken`
//!   `@sensitive`, so reproducing caller input in an error body is the wrong
//!   default for the whole service, and the clause's job is to say what the
//!   request must satisfy, which the value does not help with;
//! * a violation on an ELEMENT of a collection is reported against the
//!   COLLECTION MEMBER itself — no element index and no `member` step — with the
//!   element's constraint nested inside `Member must satisfy constraint: [ … ]`,
//!   which is what tells a caller the violation is on an element rather than on
//!   the collection. A violation on the collection itself (its `@length`, its
//!   `@uniqueItems`) is NOT nested, and neither is one on a member INSIDE a
//!   structure element, which keeps its own full path;
//! * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$`
//!   anchors the model spells it with: a caller that lifts the advertised
//!   expression out of the message and re-applies it locally must get the
//!   character class the service advertises.
//!
//! Nothing here is keyed to an operation, a member name or a value: the rule is
//! applied to whatever [`Violation`] the walk produces, so it holds for every
//! constrained member of every operation in this service — the `@range` on a
//! `MaxResults`, the `@length`/`@pattern` on an id, a tag key or a client token,
//! and the element constraints of `ProvisionStates` or `TagKeys` alike.
//!
//! A violation class this module has no wording for declines the WHOLE refusal
//! (`None`), which keeps the framework's default sentence rather than inventing
//! one: [`ViolationKind`] is `#[non_exhaustive]`, so a class can be added
//! upstream.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// The error code this service answers an input-constraint violation with: its
/// own modelled `ValidationException`, which every operation declares with
/// `@httpError 400` (`smithy_query(query="digest")` lists it among the service's
/// common errors). It is the code the framework's default would have named too,
/// restated because `override_refusal` replaces code and message TOGETHER.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The suffix the framework's validation path carries for the step from a
/// collection into one of its elements: `path::element` spells an element as its
/// 1-based position followed by this segment (`provisionStates.1.member`).
const COLLECTION_STEP: &str = ".member";

/// The requirement a violation states, worded as a service words it. `None` for
/// a class this service has no wording for.
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
/// A `$` preceded by an odd number of backslashes is an ESCAPED dollar sign —
/// a literal character of the expression, not the end anchor — so it stays.
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

/// The path of the collection a violation's path points INTO, when the violation
/// is on one of that collection's elements.
///
/// An element path ends with the collection step preceded by the element's
/// 1-based position (`provisionStates.1.member` -> `provisionStates`). The
/// position is what identifies the step, so a member that happens to be NAMED
/// `member` (`filters.1.member.member`) is not mistaken for one.
fn enclosing_collection(path: &str) -> Option<&str> {
    let (head, position) = path.strip_suffix(COLLECTION_STEP)?.rsplit_once('.')?;
    if position.is_empty() || !position.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(head)
}

/// One violation as a clause of the envelope, or `None` for a class this service
/// has no wording for.
fn clause(violation: &Violation) -> Option<String> {
    let mut requirement = requirement(&violation.kind)?;
    // Walk out of every collection step the path ends with, nesting the
    // requirement once per step: a list of lists nests twice, for the same
    // reason it nests once.
    let mut path = violation.path.as_str();
    while let Some(collection) = enclosing_collection(path) {
        requirement = format!("Member must satisfy constraint: [{requirement}]");
        path = collection;
    }
    // The value is never rendered. An absent member is reported as the `null` it
    // was, which is the shape of the request rather than a value the caller sent.
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{path}' failed to satisfy constraint: {requirement}"
    ))
}

/// The `ValidationException` message for a refused request: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the
/// order the walk found them.
///
/// Two elements of one collection failing the same constraint state the SAME
/// clause once their paths collapse onto the collection member, so an identical
/// clause is reported once: the member is named once however many of its
/// elements were refused. `None` declines the whole refusal — see [`clause`].
pub(crate) fn envelope(violations: &[Violation]) -> Option<String> {
    let mut clauses: Vec<String> = Vec::with_capacity(violations.len());
    for violation in violations {
        let clause = clause(violation)?;
        if !clauses.contains(&clause) {
            clauses.push(clause);
        }
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
/// different failure with a different code and status, and nothing about it is
/// this service's to word.
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

    /// The two path forms this service's own model reaches are covered
    /// end-to-end by `tests/all/validation_messages.rs`. These pin the branches
    /// no input shape of THIS model can produce, so the rule stays stated for a
    /// model that grows one: a `$` that is part of the expression, a step whose
    /// position is not an index (a map entry is spelled `<key>.member` by
    /// `constraints::path::entry`), and a member that is itself named `member`.
    #[test]
    fn an_escaped_dollar_is_part_of_the_expression() {
        for (pattern, expected) in [
            (r"^[a-z]+$", "[a-z]+"),
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

    #[test]
    fn only_an_indexed_step_is_a_collection_element() {
        for (path, expected) in [
            ("provisionStates.1.member", Some("provisionStates")),
            ("filters.12.member", Some("filters")),
            // A list of lists: one step out at a time.
            ("outer.2.member.3.member", Some("outer.2.member")),
            // Not an element: a map entry, a member named `member`, a plain
            // member, and a member inside a structure element.
            ("tags.someKey.member", None),
            ("filters.1.member.member", None),
            ("maxResults", None),
            ("tags.1.member.key", None),
        ] {
            let actual = enclosing_collection(path);
            assert_eq!(
                actual, expected,
                "case {path:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

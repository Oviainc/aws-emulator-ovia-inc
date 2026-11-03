// SPDX-License-Identifier: CC0-1.0

//! How this service states a constraint violation.
//!
//! `1 validation error detected: <clause>` over one clause per violation, each
//! `Value '<value>' at '<member path>' failed to satisfy constraint: <requirement>`. An
//! AWS constraint-validation message is BUILT FROM THE REQUEST, so each part below is the
//! service's behaviour rather than prose.
//!
//! What this module decides, and nothing else: the SENTENCE. Which violations a request
//! has, and which value each one is about, come from the framework's own machinery and
//! from `services/common`'s single walk over the built input
//! ([`service_common::validation::refusing_violations_with_echoes`]).
//!
//! The three parts this service does not take from the framework's own wording:
//!
//! * a `@pattern` requirement names the expression AWS reports, not the model literal
//!   ([`rendered_pattern`]);
//! * the value is quoted only when the service HAS it as text and it is short enough to
//!   carry; a blob member, and anything past [`MAX_QUOTED_VALUE`], is stated as
//!   `Value at '<member path>' …` with no value at all, never as a byte count or an
//!   element count ([`clause`]);
//! * a list-valued member is quoted AS THE LIST, with a structure element rendered
//!   `<ShapeName>(<member>=<value>, …)` — which is what [`SubmittedEcho::Rendered`]
//!   carries for it.

use service_common::validation::{unanchored, SubmittedEcho};
use smithy_server_runtime::{Violation, ViolationKind};

/// The longest value a clause quotes back. Past it the clause states the member and the
/// requirement and NO value.
///
/// A bound has to exist for the reason the framework's own renderer has one
/// (`MAX_ECHOED_VALUE` in `vendor/smithy-schema-server/src/constraints.rs`): a caller
/// controls the length of what it sends and a clause interpolates it, so an unbounded echo
/// turns one request into a response sized by that request. Where it sits is what the
/// service's own answers show: a 116-character id and a two-element list of structures
/// (263 bytes rendered) are quoted back, a 1000-plus-character access token is not, so the
/// bound is between those two — 512 bytes is the power of two inside that window. It is
/// evidence-bounded rather than AWS-published: a recording between 263 and 1000 bytes
/// would settle it exactly.
pub(crate) const MAX_QUOTED_VALUE: usize = 512;

/// The requirement one violation states.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`envelope`] decline the whole refusal and keep the framework's wording rather than
/// invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a class can be
/// added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
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
            rendered_pattern(pattern)
        ),
        ViolationKind::EnumValue(allowed) => format!(
            "Member must satisfy enum value set: [{}]",
            allowed.join(", ")
        ),
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// A `@pattern` as the service reports it, rather than as the model spells it.
///
/// A Smithy `@pattern` matches anywhere in the value, so a model demands a whole-string
/// match by writing the anchors itself. The expression the service reports is the one a
/// caller re-validates against: an expression that OPENS with `^` and CLOSES with `$`
/// reports without that pair — [`unanchored`], the tree's single home for that rule, which
/// drops the pair positionally and keeps a trailing escaped `\$`, a literal rather than an
/// anchor — because the anchoring is how the model says "all of it" and not part of the
/// expression; an expression carrying no such pair is reported wrapped in `.*` … `.*`, the
/// whole-string form of the partial match it expresses. That wrapping is this service's own
/// dialect on top of the shared rule.
///
/// The wrapping is TEXTUAL and adds no group of its own, because that is the form the
/// service's own answers carry: for its two alternating id patterns the reported expression
/// is `.*(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$).*` — the `.*` pair concatenated onto the
/// first and last branch rather than around a `(?:…)` of the whole alternation. Grouping it
/// would change the reported TEXT, which is the thing a caller reads, so the wrapping stays
/// as recorded even though an alternation then binds as
/// `(.*(^waba-.*$)) | ((^arn:…$).*)`; the unit table below pins both forms.
fn rendered_pattern(pattern: &str) -> String {
    let expression = unanchored(pattern);
    if expression == pattern {
        // Nothing was dropped, so the model's expression is not a whole-string one.
        return format!(".*{pattern}.*");
    }
    expression
}

/// Render one constraint violation the way this service states it.
///
/// `echo` decides the subject: a member the request never sent keeps `Value null at
/// '<path>'`, because the `null` is a fact of the request's SHAPE rather than a rendering
/// of anything the caller sent; a value this service holds as text, and is willing to
/// quote, is stated back verbatim; anything else — a blob's bytes, an oversize value — is
/// stated as `Value at '<path>'`, with the value left out rather than summarised.
fn clause(violation: &Violation, echo: &SubmittedEcho) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let path = &violation.path;
    let subject = match echo {
        SubmittedEcho::Absent => format!("Value null at '{path}'"),
        SubmittedEcho::Rendered(value) if value.len() <= MAX_QUOTED_VALUE => {
            format!("Value '{value}' at '{path}'")
        }
        SubmittedEcho::Rendered(_) | SubmittedEcho::Unrendered => format!("Value at '{path}'"),
    };
    Some(format!(
        "{subject} failed to satisfy constraint: {requirement}"
    ))
}

/// The `ValidationException` message for a refusal: the AWS envelope over one clause per
/// violation, in the order the validating walk found them.
///
/// `None` declines the refusal — see [`requirement`] — and is also the answer for a
/// request with no violations at all, so a caller can use it to decide whether to refuse.
pub(crate) fn envelope(refused: &[(Violation, SubmittedEcho)]) -> Option<String> {
    let clauses: Vec<String> = refused
        .iter()
        .map(|(violation, echo)| clause(violation, echo))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an expression that opens with `^` and closes with `$` reports without that
    /// pair; one carrying no such pair reports wrapped in `.*` … `.*`.
    ///
    /// The rows span the forms a `@pattern` takes rather than this service's members
    /// (those are asserted end-to-end in `tests/all/validation_message_rendering.rs`):
    /// anchored at both ends, at one end, at neither, branches carrying an anchor of their
    /// own inside the outer pair, and an expression ending in an escaped `$`.
    #[test]
    fn a_pattern_reports_the_expression_a_value_must_satisfy() {
        let rows: [(&str, &str, &str); 9] = [
            ("anchored at both ends", "^[0-9]+$", "[0-9]+"),
            // The outer pair is dropped positionally, whatever the branches inside it
            // carry: an anchor inside a group is part of the expression, not the model's
            // "all of it".
            (
                "a branch carrying an anchor inside the outer pair",
                "^(a$|b)$",
                "(a$|b)",
            ),
            (
                "branches anchored individually, sharing the outer pair",
                "^a$|^b$",
                "a$|^b",
            ),
            ("anchored at the start only", "^abc", ".*^abc.*"),
            ("anchored at the end only", "abc$", ".*abc$.*"),
            ("anchored at neither end", "[a-z]+", ".*[a-z]+.*"),
            (
                "alternating over anchored branches",
                "(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$)",
                ".*(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$).*",
            ),
            ("an escaped dollar at the end", r"^price\$", r".*^price\$.*"),
            (
                "an escaped backslash before the end anchor",
                r"^path\\$",
                r"path\\",
            ),
        ];
        for (case, pattern, expected) in rows {
            let actual = rendered_pattern(pattern);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the subject of a clause is the value only when this service holds it as text
    /// and it is at most [`MAX_QUOTED_VALUE`] bytes; otherwise the clause states the
    /// member alone, and an ABSENT member states `null`.
    #[test]
    fn a_clause_quotes_only_a_value_it_has_and_can_carry() {
        let at_bound = "a".repeat(MAX_QUOTED_VALUE);
        let past_bound = "a".repeat(MAX_QUOTED_VALUE + 1);
        let rows: [(&str, SubmittedEcho, String); 5] = [
            (
                "an absent member",
                SubmittedEcho::Absent,
                "Value null at 'id' failed to satisfy constraint: Member must have length \
                 less than or equal to 115"
                    .to_string(),
            ),
            (
                "a short value",
                SubmittedEcho::Rendered("waba-x".to_string()),
                "Value 'waba-x' at 'id' failed to satisfy constraint: Member must have \
                 length less than or equal to 115"
                    .to_string(),
            ),
            (
                "a value at the quoting bound",
                SubmittedEcho::Rendered(at_bound.clone()),
                format!(
                    "Value '{at_bound}' at 'id' failed to satisfy constraint: Member must \
                     have length less than or equal to 115"
                ),
            ),
            (
                "a value one byte past the quoting bound",
                SubmittedEcho::Rendered(past_bound),
                "Value at 'id' failed to satisfy constraint: Member must have length less \
                 than or equal to 115"
                    .to_string(),
            ),
            (
                "a value with no text rendering",
                SubmittedEcho::Unrendered,
                "Value at 'id' failed to satisfy constraint: Member must have length less \
                 than or equal to 115"
                    .to_string(),
            ),
        ];
        for (case, echo, expected) in rows {
            let violation = Violation {
                path: "id".to_string(),
                kind: ViolationKind::MaxLength(115),
                value: match echo {
                    SubmittedEcho::Absent => None,
                    _ => Some("whatever the framework rendered".to_string()),
                },
                sensitive: false,
            };
            let actual = clause(&violation, &echo);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

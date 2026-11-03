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
//!   <requirement>`. The value is the one the CALLER SUBMITTED, quoted whole, including
//!   where the member is a LIST or a STRUCTURE, which the framework's own violation carries
//!   only a count summary of (`<n> items`) — see [`clause`]. An absent member has no value
//!   to quote and is reported as the `null` it was;
//! * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors a model
//!   spells it with to demand a whole-string match: a client that lifts the advertised
//!   expression out of the message to repair its request, or asserts on it, must see the
//!   expression the service advertises and not the model's anchored source;
//! * the clauses are counted and joined into the AWS envelope — `1 validation error
//!   detected: <clause>` for one, `N validation errors detected: <clause>; <clause>` for
//!   several — in the order the walk found them: the count is of every violation the walk
//!   found, while the clauses spelled after it are budgeted
//!   ([`ENVELOPE_LENGTH_LIMIT`], `crate::state::clip_list_for_message`) and whatever does
//!   not fit is reported as a remainder, because a clause echoes a value the caller sent
//!   and one request can be refused for many.
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
//! That crate's OTHER half — the walk that restores the value a violation is about to the
//! one the caller SUBMITTED (`refusing_violations_with_echoes`, which also renders a
//! composite member) — needs the BUILT INPUT and its operation's schema, while
//! `override_refusal` is handed the violations alone (no operation, no request). So it is
//! called from the per-operation wrapper [`crate::service::WordedRefusals`], which words
//! every constraint refusal of a modelled operation through the clause builder here; this
//! seam stays the fallback for a refusal that does not reach that wrapper.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

use ::service_common::validation::SubmittedEcho;

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
pub(crate) const MAX_VIOLATIONS: usize = 100;

/// How long the envelope of a boundary refusal may run, in bytes.
///
/// RECORDED AWS behaviour: the constraint envelope quotes the value the caller SUBMITTED,
/// and it quotes it WHOLE — a list one element past its `@length` maximum comes back with
/// every element spelled out, in a sentence far longer than the `@length(0, 1024)` the
/// member it travels in declares. So this envelope is not held to
/// [`crate::state::MESSAGE_LENGTH_LIMIT`], which is what every refusal this service WORDS
/// ITSELF is held to: what bounds it instead is the echo the restoring walk may produce,
/// per value and over one message (`service_common::validation`, whose own ceilings are 16
/// KiB and 64 KiB). That number is restated here because that module keeps it private, and
/// it is what makes this sentence finite without clipping it to a length AWS does not clip
/// it to.
const ENVELOPE_LENGTH_LIMIT: usize = 64 * 1024;

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
/// The value is quoted as the CALLER SUBMITTED it (`echo`), which is what AWS's own clause
/// echoes: a composite member — a list, a structure — reaches a service with the
/// framework's count summary (`<n> items`) in place of its value, which is wire text AWS
/// never emits, so where the restoring walk rendered the submitted value that rendering is
/// quoted instead. A value the walk could not render keeps the framework's own text, which
/// for every scalar member IS the submitted value, and an absent member is reported as the
/// `null` it was — a fact of the request's shape rather than a value the caller sent.
pub(crate) fn clause(violation: &Violation, echo: &SubmittedEcho) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let path = &violation.path;
    let value = match echo {
        SubmittedEcho::Rendered(rendered) => Some(rendered.as_str()),
        SubmittedEcho::Absent => None,
        SubmittedEcho::Unrendered => violation.value.as_deref(),
    };
    Some(match value {
        Some(shown) => {
            format!("Value '{shown}' at '{path}' failed to satisfy constraint: {requirement}")
        }
        None => format!("Value null at '{path}' failed to satisfy constraint: {requirement}"),
    })
}

/// Each violation paired with the only echo this service can offer when it is handed the
/// violations WITHOUT the request they came from ([`refused`]): an absent member has no
/// value, and a present one keeps whatever the framework stored for it.
fn without_echoes(violations: &[Violation]) -> Vec<(Violation, SubmittedEcho)> {
    violations
        .iter()
        .map(|violation| {
            let echo = match violation.value {
                None => SubmittedEcho::Absent,
                Some(_) => SubmittedEcho::Unrendered,
            };
            (violation.clone(), echo)
        })
        .collect()
}

/// The `message` of a refused request: the AWS envelope over one clause per violation, in
/// the order the walk found them. `None` declines the whole refusal — see [`clause`].
///
/// The count is of every clause the walk produced, while the clauses the sentence SPELLS
/// are budgeted by `crate::state::clip_list_for_message` for the reason stated there: a
/// clause echoes a caller-supplied value and a refused request carries up to
/// [`MAX_VIOLATIONS`] of them — so the count is stated whole and the clauses that do not
/// fit are reported as a remainder. The budget is [`ENVELOPE_LENGTH_LIMIT`], the bound this
/// sentence really carries, rather than the message member's own modelled length.
pub(crate) fn envelope(refused: &[(Violation, SubmittedEcho)]) -> Option<String> {
    let clauses = refused
        .iter()
        .map(|(violation, echo)| clause(violation, echo))
        .collect::<Option<Vec<String>>>()?;
    let opening = match clauses.len() {
        0 => return None,
        1 => "1 validation error detected: ".to_string(),
        n => format!("{n} validation errors detected: "),
    };
    let clauses: Vec<&str> = clauses.iter().map(String::as_str).collect();
    let stated = crate::state::clip_list_for_message(
        &clauses,
        "; ",
        ENVELOPE_LENGTH_LIMIT.saturating_sub(opening.len()),
    );
    Some(format!("{opening}{stated}"))
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a different
/// failure with a different code and status, and nothing about it is this service's to
/// word.
///
/// This is the FALLBACK path: [`crate::service::WordedRefusals`] answers every constraint
/// refusal of a modelled operation before the centralized walk reaches here, because it is
/// the only layer that still has the REQUEST and so can quote a composite member's
/// submitted value. What reaches here is worded by the same clause builder, from the only
/// echo this seam can offer ([`without_echoes`]).
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(&without_echoes(
                &violations[..violations.len().min(MAX_VIOLATIONS)],
            ))?,
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

    /// A clause quotes the value the CALLER SUBMITTED, the member path and the
    /// requirement; an absent member keeps the `null` the request's shape states, and a
    /// value the restoring walk could not render keeps the framework's own text.
    #[test]
    fn a_clause_states_the_value_the_member_and_the_requirement() {
        let cases = [
            (
                Violation {
                    path: "export.name".to_string(),
                    kind: ViolationKind::Pattern(r"^[0-9A-Za-z\-_]+$".to_string()),
                    value: Some("bad name!".to_string()),
                    sensitive: false,
                },
                SubmittedEcho::Rendered("bad name!".to_string()),
                "Value 'bad name!' at 'export.name' failed to satisfy constraint: Member \
                 must satisfy regular expression pattern: [0-9A-Za-z\\-_]+",
            ),
            // A LIST member: the framework stored its count summary, and the clause quotes
            // the rendered list the caller sent instead — structure elements and all.
            (
                Violation {
                    path: "resourceTags".to_string(),
                    kind: ViolationKind::MaxLength(200),
                    value: Some("201 items".to_string()),
                    sensitive: false,
                },
                SubmittedEcho::Rendered(
                    "[ResourceTag(key=k0, value=v0), ResourceTag(key=k1, value=v1)]"
                        .to_string(),
                ),
                "Value '[ResourceTag(key=k0, value=v0), ResourceTag(key=k1, value=v1)]' at \
                 'resourceTags' failed to satisfy constraint: Member must have length less \
                 than or equal to 200",
            ),
            // The same violation where the walk rendered nothing: the framework's own
            // text stands rather than no value at all.
            (
                Violation {
                    path: "resourceTags".to_string(),
                    kind: ViolationKind::MaxLength(200),
                    value: Some("201 items".to_string()),
                    sensitive: false,
                },
                SubmittedEcho::Unrendered,
                "Value '201 items' at 'resourceTags' failed to satisfy constraint: Member \
                 must have length less than or equal to 200",
            ),
            (
                Violation {
                    path: "export.name".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                SubmittedEcho::Absent,
                "Value null at 'export.name' failed to satisfy constraint: Member must \
                 not be null",
            ),
        ];
        for (violation, echo, expected) in cases {
            let actual =
                clause(&violation, &echo).expect("a modelled violation class is worded");
            assert_eq!(
                actual, expected,
                "case {violation:?} with {echo:?}: expected {expected:?}, got {actual:?}"
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
            sensitive: false,
        }];
        let two = vec![
            one[0].clone(),
            Violation {
                path: "exportArn".to_string(),
                kind: ViolationKind::MinLength(20),
                value: Some("arn:aws".to_string()),
                sensitive: false,
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
            let actual = envelope(&without_echoes(violations)).expect("a worded refusal");
            assert_eq!(
                actual, expected,
                "case {violations:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

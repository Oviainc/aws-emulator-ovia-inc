// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The framework walks the modelled constraints and hands [`Refusal`] to
//! `service.rs` STRUCTURALLY — one [`Violation`] per offending value, carrying
//! the dotted path it sits at, what it failed and the value it held as the
//! framework rendered it — and leaves the wording to the service, because no
//! model records it. This module is that wording for every modelled input
//! constraint of this service: one formatter, reached by every operation's
//! boundary validation, so no operation can word a refusal its own way.
//!
//! Step Functions answers the framework's own validation envelope VERBATIM, with
//! ONE difference: the clause echoes the REJECTED VALUE as the member's MODELLED
//! TYPE renders it. A member whose target shape is a Smithy `float`/`double`
//! renders with a fractional part (`150` -> `150.0`), where an integral member
//! (`integer`, `long`) renders without one — and Rust's own `Display` for a float
//! drops a zero fraction (`150f32.to_string() == "150"`), so the framework's
//! default clause reports a float member's value in an integer's spelling.
//!
//! That one difference is ALL this module does. It does not restate the
//! requirement wording, the subject, the clause shape or the envelope: it hands
//! the framework a [`Violation`] whose `value` is spelled in the member's
//! modelled type and lets [`aws_smithy_schema_server::envelope`] — the same
//! renderer the default path uses — write the message. So this wording cannot
//! drift from the framework's, and a [`ViolationKind`] added upstream is worded
//! by the framework without a change here. (That is also why there is no copy of
//! the rules staged in `services/common/src/validation.rs`, whose note asks each
//! new inlining to cross-reference it: nothing is inlined here to migrate.)
//!
//! Nothing here is keyed to an operation or to a particular value: the rule is
//! applied to whatever [`Violation`] the walk produces, so it holds for every
//! constrained member of every operation of this service.
//!
//! The structural fix is still owed upstream — a `Violation` carrying the
//! member's modelled numeric kind, or a framework that renders a value per
//! modelled type — which would remove this module entirely.

use smithy_server_runtime::{Refusal, Refused, Violation};

use crate::state::FRACTIONAL_MEMBERS;

/// The error code this service answers an input-constraint violation with: the
/// modelled `ValidationException`, which every operation of this service lists
/// with `@httpError 400`. It is the code the framework's default would have
/// named too, restated because `override_refusal` replaces code and message
/// TOGETHER.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The step segment the framework's paths carry for a move from a collection
/// into one of its elements (`list.1.member`), which is not a member name.
const COLLECTION_STEP: &str = "member";

/// The STRUCTURE MEMBER a dotted path's violated leaf names, or `None` when the
/// leaf is a position inside a collection rather than a member of the model.
///
/// The framework spells a collection element as `<index-or-key>.member`
/// (`constraints::path::element` / `::entry`), so a path ending in that step
/// reports a position — an index, or a MAP KEY the CALLER chose — whose
/// constraints are the collection's own, not a member whose modelled type could
/// be read off the model. Only a structure member selects a rendering here, so
/// no caller-chosen key can name itself a member of this model.
fn member_of(path: &str) -> Option<&str> {
    let member = path.split('.').next_back()?;
    (member != COLLECTION_STEP).then_some(member)
}

/// The rejected value as the member at `path` renders it in its modelled type.
///
/// Only a fractional member is re-rendered, and only when the framework's
/// display dropped the fraction from a value a `float`/`double` member could
/// actually carry: a value that already shows one, that carries an exponent,
/// that is not a finite number, or that is longer than a float's own decimal
/// display can be (so an elided or absurdly long echo is left alone) is echoed
/// exactly as it arrived.
fn rendered_value(path: &str, shown: &str) -> String {
    if !member_of(path).is_some_and(|member| FRACTIONAL_MEMBERS.contains(&member)) {
        return shown.to_string();
    }
    // An `f64`'s widest fraction-less decimal display is its integral part, and
    // `f64::MAX` has 309 digits; a sign takes one more. Anything longer is not a
    // value the member carried, so its spelling is not this rule's to change.
    const WIDEST_FRACTION_LESS_FLOAT: usize = 310;
    if shown.len() > WIDEST_FRACTION_LESS_FLOAT
        || shown.contains('.')
        || shown.contains(['e', 'E'])
        || !shown
            .strip_prefix(['-', '+'])
            .unwrap_or(shown)
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return shown.to_string();
    }
    match shown.parse::<f64>() {
        Ok(number) if number.is_finite() => format!("{shown}.0"),
        _ => shown.to_string(),
    }
}

/// `violation` with its rejected value spelled in the member's modelled type.
///
/// Everything else about the violation — its path, what it failed, and the fact
/// that an absent member carries no value at all — is left exactly as the walk
/// reported it, so the framework words the clause.
fn in_modelled_type(violation: &Violation) -> Violation {
    let mut rendered = violation.clone();
    if let Some(shown) = &rendered.value {
        let in_type = rendered_value(&rendered.path, shown);
        if &in_type != shown {
            rendered.value = Some(in_type);
        }
    }
    rendered
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a
/// different failure with a different code and status, and nothing about it is
/// this service's to word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => {
            let in_modelled_types: Vec<Violation> =
                violations.iter().map(in_modelled_type).collect();
            Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: aws_smithy_schema_server::envelope(&in_modelled_types)?,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path forms this model's own input shapes reach are covered
    /// end-to-end by `tests/all/error_messages.rs`. These pin the branches no
    /// input shape of THIS model produces, so the rule stays stated for a model
    /// that grows one: a leaf that is a collection POSITION rather than a
    /// member (an element index, and a map key — which a caller chooses, so it
    /// must never select a member's rendering), and a member inside a structure
    /// element, which keeps its own name.
    #[test]
    fn only_a_structure_member_names_a_member_of_the_model() {
        for (path, expected) in [
            (
                "toleratedFailurePercentage",
                Some("toleratedFailurePercentage"),
            ),
            ("maxConcurrency", Some("maxConcurrency")),
            ("tags.1.member.key", Some("key")),
            // A collection position, not a member: no modelled type to read.
            ("percentages.3.member", None),
            ("tags.toleratedFailurePercentage.member", None),
            ("tags.someKey.member", None),
        ] {
            let actual = member_of(path);
            assert_eq!(
                actual, expected,
                "case {path:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A fractional member's value is rendered with a fraction only when the
    /// display dropped one and the value is a finite number.
    #[test]
    fn only_a_finite_fraction_less_float_gains_a_fraction() {
        for (path, shown, expected) in [
            ("toleratedFailurePercentage", "150", "150.0"),
            ("toleratedFailurePercentage", "-1", "-1.0"),
            ("toleratedFailurePercentage", "100.5", "100.5"),
            ("toleratedFailurePercentage", "1e9", "1e9"),
            ("toleratedFailurePercentage", "inf", "inf"),
            ("toleratedFailurePercentage", "NaN", "NaN"),
            // An integral member keeps the integer spelling.
            ("maxConcurrency", "-1", "-1"),
            ("toleratedFailureCount", "-1", "-1"),
            // Not a value a float member could carry fraction-less: an elided or
            // absurdly long echo keeps the spelling it arrived with.
            (
                "toleratedFailurePercentage",
                "1... [4096 bytes]",
                "1... [4096 bytes]",
            ),
            // A caller-chosen map key spelling a fractional member's name is a
            // collection position, so the value it carries is echoed as sent.
            ("tags.toleratedFailurePercentage.member", "150", "150"),
        ] {
            let actual = rendered_value(path, shown);
            assert_eq!(
                actual, expected,
                "case {path:?}/{shown:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

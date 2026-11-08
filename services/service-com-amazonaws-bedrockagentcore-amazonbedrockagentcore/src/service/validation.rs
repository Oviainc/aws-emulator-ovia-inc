// SPDX-License-Identifier: CC0-1.0

//! How this service states the framework's boundary refusal of a request.
//!
//! The words are NOT written here. `service_common::validation_message` is the
//! cross-service home for them — the envelope, the member paths and the requirements —
//! and this module only makes the choice that belongs to a service: WHICH of those
//! renderings this service's recordings show, and for which refusals.
//!
//! This service states a violated numeric `@range` bound WITHOUT the value that violated
//! it: the message names the member path and the bound only, inside AWS's validation
//! envelope. The code stays the modelled `ValidationException` (HTTP 400) — what the
//! framework would have answered — so only the wording changes, and only for a refusal
//! that carries such a bound: a refusal of any other class keeps the framework's own
//! wording, which this service has no recorded wording to replace it with.
//!
//! Nothing here is keyed to an operation or to a submitted value, so the rule holds for
//! every constrained member of every operation of this service rather than for the
//! members one report happened to cite.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// Whether `violation` is a violated numeric `@range` bound, which is the one class this
/// service states WITHOUT the value that violated it: such a refusal names the member
/// path and the bound only.
fn is_numeric_range(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)
    )
}

/// How this service states `refusal`, or `None` to keep the framework's own wording.
///
/// `None` covers what this service does not reword — an authentication refusal is not an
/// input-constraint failure, and a constraint class it has no recorded wording for keeps
/// the framework's sentence, value and all.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) if violations.iter().any(is_numeric_range) => {
            service_common::validation_message::aws_validation_refusal_withholding_violations(
                refusal,
                is_numeric_range,
            )
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of `kind` at `path`, holding `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: value.map(str::to_owned),
            sensitive: false,
        }
    }

    /// The choice this module makes, stated where a wire case cannot see it: a violated
    /// numeric bound withholds the value on EITHER side of the range, and a violation of
    /// any other class keeps the framework's own clause.
    #[test]
    fn a_violated_numeric_bound_withholds_its_value_and_no_other_class_does() {
        // (case, the violation, the clause the message must carry)
        let rows = [
            (
                "a value above its range maximum",
                violation(
                    "sessionTimeoutSeconds",
                    ViolationKind::MaxValue("28800".to_owned()),
                    Some("28801"),
                ),
                "Value at 'sessionTimeoutSeconds' failed to satisfy constraint: Member must \
                 have value less than or equal to 28800",
            ),
            (
                "a value below its range minimum",
                violation("viewPort.width", ViolationKind::MinValue("320".to_owned()), Some("319")),
                "Value at 'viewPort.width' failed to satisfy constraint: Member must have \
                 value greater than or equal to 320",
            ),
            (
                "a member past its length maximum, another class",
                violation("name", ViolationKind::MaxLength(100), Some("n")),
                "Value 'n' at 'name' failed to satisfy constraint: Member must have length \
                 less than or equal to 100",
            ),
        ];
        for (case, offence, clause) in rows {
            let expected = format!("1 validation error detected: {clause}");
            let stated = match refused(&Refusal::Invalid(vec![offence.clone()])) {
                Some(stated) => stated.message,
                // A class this service does not reword keeps the framework's own
                // sentence, which is what the row then states.
                None => format!("1 validation error detected: {}", offence.message()),
            };
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// Nothing but a constraint refusal carrying a numeric bound is reworded.
    #[test]
    fn nothing_but_a_numeric_bound_refusal_is_reworded() {
        for (case, refusal) in [
            ("an unauthenticated caller", Refusal::Unauthenticated),
            ("no violations at all", Refusal::Invalid(Vec::new())),
            (
                "a member outside its pattern",
                Refusal::Invalid(vec![violation(
                    "eventId",
                    ViolationKind::Pattern("^[0-9]+$".to_owned()),
                    Some("not-an-event-id"),
                )]),
            ),
        ] {
            let stated = refused(&refusal);
            assert!(
                stated.is_none(),
                "case {case:?}: expected the framework's own wording, got {stated:?}"
            );
        }
    }
}

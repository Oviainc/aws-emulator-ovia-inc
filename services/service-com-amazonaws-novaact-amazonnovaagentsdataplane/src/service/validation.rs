// SPDX-License-Identifier: CC0-1.0

//! How this service states a refusal of a value one of its modelled `@pattern`s does not
//! admit.
//!
//! The centralized walk enforces the modelled input constraints and hands [`Refusal`] to
//! `service.rs` STRUCTURALLY — one [`Violation`] per offending value, carrying the member
//! path it sits at, what it failed and what it held — because no model records the sentence
//! a service says. This module is the ONE choice this service makes about that sentence,
//! and it makes it for a constraint CLASS rather than for an operation or a member:
//!
//! * a `@pattern` requirement is stated over the EXPRESSION alone — the expression quoted
//!   with the whole-value anchors the model wraps it in removed, and the clause naming no
//!   value, because the requirement is the expression every value of that member must match
//!   rather than the one value that missed it. So the sentence is the same whatever a caller
//!   submitted, and the refusal is stable across requests. It holds for whichever member of
//!   whichever operation broke it: a body member, a URI label and a query parameter alike.
//!
//! Both of those renderings, the counted envelope, the clause, the member path and every
//! other constraint class's requirement belong to [`service_common::validation_message`] —
//! the shared home for this wording — and to the framework behind it. None of them is
//! spelled here, so this service cannot word a constraint differently from the services that
//! state the same sentence.
//!
//! A refusal carrying no pattern violation is left to the framework entirely, so every other
//! class — a `@length` or `@range` bound, an enum value set, an absent `@required` member —
//! keeps the clause the framework wrote for it, value and all. The pattern class is the one
//! this service has a recording of, and the one it restates.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// `None` for anything that is not an input-constraint refusal (an authentication refusal is
/// a different failure, with its own code and status, and nothing about it is this service's
/// to word) and for a constraint refusal that states no `@pattern` requirement.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
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
    use super::*;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some("submitted".to_owned()),
            sensitive: false,
        }
    }

    /// The narrowness of the choice, stated where the wire cases in
    /// `tests/all/validation_message_wording.rs` cannot see it: a refusal this service does
    /// not restate is declined whole, so the framework words it.
    #[test]
    fn a_refusal_stating_no_pattern_requirement_is_declined() {
        for (label, refusal) in [
            (
                "a length bound alone",
                Refusal::Invalid(vec![violation("name", ViolationKind::MaxLength(40))]),
            ),
            (
                "a range bound alone",
                Refusal::Invalid(vec![violation(
                    "maxResults",
                    ViolationKind::MinValue("1".to_owned()),
                )]),
            ),
            (
                "an absent required member",
                Refusal::Invalid(vec![Violation {
                    path: "name".to_owned(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                }]),
            ),
            ("an unauthenticated caller", Refusal::Unauthenticated),
        ] {
            let stated = refused(&refusal);
            assert!(
                stated.is_none(),
                "case {label:?}: expected the framework's own wording, got {stated:?}"
            );
        }
    }
}

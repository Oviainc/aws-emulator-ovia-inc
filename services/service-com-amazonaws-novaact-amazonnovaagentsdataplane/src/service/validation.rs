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
//! * an enumerated type's value SET is stated the same way: the clause lists the values the
//!   type admits, in the order the model declares them, and names no submitted value,
//!   because the set is the requirement every value of that member must be in rather than
//!   the one value that was not. So this refusal too reads the same whatever a caller sent.
//!
//! Both of those renderings, the counted envelope, the clause, the member path and every
//! other constraint class's requirement belong to [`service_common::validation_message`] —
//! the shared home for this wording — and to the framework behind it. None of them is
//! spelled here, so this service cannot word a constraint differently from the services that
//! state the same sentence.
//!
//! A refusal carrying neither of those classes is left to the framework entirely, so every
//! other class — a `@length` or `@range` bound, an absent `@required` member — keeps the
//! clause the framework wrote for it, value and all. Those two are the classes this service
//! has recordings of, and the ones it restates.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// `None` for anything that is not an input-constraint refusal (an authentication refusal is
/// a different failure, with its own code and status, and nothing about it is this service's
/// to word) and for a constraint refusal that states neither a `@pattern` requirement nor an
/// enumerated type's value set.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if !violations.iter().any(states_no_value) {
        return None;
    }
    service_common::validation_message::aws_validation_refusal_withholding_violations(
        refusal,
        states_no_value,
    )
}

/// Whether `violation` is one of the classes this service states without a value: a value
/// outside its member's `@pattern`, and a value outside an enumerated type's value set.
fn states_no_value(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::Pattern(_) | ViolationKind::EnumValue(_)
    )
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
    fn a_refusal_stating_neither_restated_class_is_declined() {
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

    /// An enumerated type's value set is stated without the value the request sent, listing
    /// the values the model declares in the order it declares them.
    #[test]
    fn an_enum_value_set_is_stated_without_the_submitted_value() {
        let refusal = Refusal::Invalid(vec![violation(
            "status",
            ViolationKind::EnumValue(vec![
                "RUNNING".to_owned(),
                "PENDING_CLIENT_ACTION".to_owned(),
                "SUCCEEDED".to_owned(),
            ]),
        )]);
        let stated = refused(&refusal).expect("an enum value set is this service's to state");
        assert!(
            stated
                .message
                .contains("[RUNNING, PENDING_CLIENT_ACTION, SUCCEEDED]"),
            "expected the value set listed as the model declares it, got {:?}",
            stated.message
        );
        assert!(
            !stated.message.contains("submitted"),
            "expected no submitted value in the clause, got {:?}",
            stated.message
        );
    }
}

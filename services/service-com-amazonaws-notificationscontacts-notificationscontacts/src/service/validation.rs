// SPDX-License-Identifier: CC0-1.0

//! How this service states the framework's boundary refusal of a request.
//!
//! The words are NOT written here. `service_common::validation_message` is the
//! cross-service home for them — the envelope, the member paths, the requirements, the
//! unanchored `@pattern` expression and the schema layer's property-count finding — and
//! this module only makes the two choices that belong to a service: WHICH of those
//! renderings this service's recordings show, and in which order they are asked.
//!
//! The dialect those recordings show:
//!
//! * a MAP carrying more entries than its modelled maximum is a refusal of the request's
//!   SHAPE against the schema, stated as that layer's property-count finding and asked
//!   FIRST, because the members of a request whose shape that layer already refuses are
//!   never reported;
//! * every clause of the per-member envelope states the member path and the requirement
//!   broken and NO submitted value — whichever member and whichever constraint class — so
//!   the decision is made about the violation rather than about a set of member paths. A
//!   member that was not there at all has no value to withhold and keeps the `null` the
//!   framework states an absent member with.
//!
//! Nothing here is keyed to an operation or to a submitted value, so one rule covers every
//! constrained member of every operation of this service; the `@pattern`s this service has
//! to apply ITSELF (`crate::state::pattern_validation_failure`) are worded through the
//! same renderings, so a constraint the walk applies and one a handler applies read alike.

use smithy_server_runtime::{Refusal, Refused};

/// How this service states `refusal`, or `None` to keep the framework's own wording.
///
/// `None` covers both what this service does not reword (an authentication refusal is not
/// an input-constraint failure, and its code and status are not this service's to change)
/// and a refusal with nothing to state.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    if let Some(refused) = service_common::validation_message::property_count_refusal(refusal) {
        return Some(refused);
    }
    service_common::validation_message::aws_validation_refusal_withholding_violations(
        refusal,
        |violation| violation.value.is_some() || violation.sensitive,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithy_server_runtime::{Violation, ViolationKind};

    /// One violation of `kind` at `path`, holding `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: value.map(str::to_owned),
            sensitive: false,
        }
    }

    /// The choice this module makes, stated where the wire cases in
    /// `tests/all/error_message_templates.rs` cannot see it: a value the request carried is
    /// withheld for EVERY constraint class, and a member the request did not carry is still
    /// the `null` it was.
    #[test]
    fn a_submitted_value_is_withheld_and_an_absent_member_is_not() {
        // (case, the violation, the clause the message must carry)
        let rows = [
            (
                "a sensitive member past its length maximum",
                violation("name", ViolationKind::MaxLength(64), Some(&"a".repeat(65))),
                "Value at 'name' failed to satisfy constraint: Member must have length less \
                 than or equal to 64",
            ),
            (
                "a member outside its pattern",
                violation(
                    "emailAddress",
                    ViolationKind::Pattern("^(.+)@(.+)$".to_owned()),
                    Some("not-an-email"),
                ),
                "Value at 'emailAddress' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: (.+)@(.+)",
            ),
            (
                "an absent required member",
                violation("code", ViolationKind::MissingRequiredMember, None),
                "Value null at 'code' failed to satisfy constraint: Member must not be null",
            ),
        ];
        for (case, offence, clause) in rows {
            let stated = refused(&Refusal::Invalid(vec![offence]))
                .unwrap_or_else(|| panic!("case {case:?}: a constraint refusal is stated"));
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// A map over its entry maximum is stated by the schema layer, so its message is asked
    /// for before the per-member envelope and carries none of it.
    #[test]
    fn a_map_over_its_entry_maximum_is_stated_by_the_schema_layer() {
        let over = violation("tags", ViolationKind::MaxLength(200), Some("201 entries"));
        let stated =
            refused(&Refusal::Invalid(vec![over])).expect("a constraint refusal is stated");
        assert_eq!(
            stated.message,
            "[object has too many properties (found 201 but schema requires at most 200)]",
            "case \"a tag map one entry past the maximum\": got {:?}",
            stated.message
        );
    }

    /// Nothing but a constraint refusal is reworded, and a refusal with nothing to state
    /// keeps the framework's wording.
    #[test]
    fn nothing_but_a_constraint_refusal_is_reworded() {
        for (case, refusal) in [
            ("an unauthenticated caller", Refusal::Unauthenticated),
            ("no violations at all", Refusal::Invalid(Vec::new())),
        ] {
            let stated = refused(&refusal);
            assert!(
                stated.is_none(),
                "case {case:?}: expected the framework's own wording, got {stated:?}"
            );
        }
    }
}

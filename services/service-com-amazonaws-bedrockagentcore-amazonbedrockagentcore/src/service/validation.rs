// SPDX-License-Identifier: CC0-1.0

//! How this service states the framework's boundary refusal of a request.
//!
//! The words are NOT written here. `service_common::validation_message` is the
//! cross-service home for them — the envelope, the member paths and the requirements —
//! and this module only makes the choices that belong to a service: WHICH of those
//! renderings this service's recordings show, for which refusals, and which value sets it
//! publishes an order of.
//!
//! THE RULE this service states: a refusal of a value outside a modelled `@pattern`, a
//! modelled `@range` bound or an enumerated type's value set names the MEMBER PATH and the
//! REQUIREMENT only — never the value that was rejected — with a `@pattern` requirement
//! quoted WITHOUT the anchors a whole-value expression wraps it in, and an enumerated
//! type's value set stated in the order this service publishes that set
//! ([`STATED_ENUM_VALUE_SETS`]). The code stays the modelled `ValidationException`
//! (HTTP 400) — what the framework would have answered — so only the wording changes.
//!
//! A refusal of any other class keeps the framework's own wording, value and all: this
//! service has no recorded wording to replace it with, and a sentence invented here would
//! be worse than the framework's consistent default.
//!
//! Nothing here is keyed to an operation or to a submitted value, so the rule holds for
//! every constrained member of every operation of this service — whatever binding carried
//! the value and at whatever depth — rather than for the members one report happened to
//! cite. [`stated_envelope`] is the same wording over violations a HANDLER collected, for
//! the constraints no walk ahead of a handler reports (a member inside a map entry's
//! value), so the two refusal paths of this service cannot word one constraint two ways.

use service_common::validation_message::{
    self, in_stated_enum_order, StatedEnumValueSets,
};
use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

use crate::ops::types::ROLE_VALUES;

/// `ValidationExceptionReason`'s value for a member whose value broke what its shape
/// declares — the reason a modelled `ValidationException` this service raises for an input
/// constraint carries (source: smithy_query query="shape"
/// shape="ValidationExceptionReason", values CannotParse | FieldValidationFailed |
/// IdempotentParameterMismatchException | EventInOtherSession | ResourceConflict).
pub(crate) const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The values AWS lists for the `Role` enumerated type when it refuses a value outside it,
/// in the order it lists them.
///
/// OBSERVED: the `Member must satisfy enum value set: […]` clause a `CreateEvent` payload
/// carrying a role outside the enum earns (recorded trace
/// `bedrock-agentcore.create-event-malformed-payload-rejected`). It is the service's own
/// published order, so it is neither sorted nor the order the model declares the enum's
/// members in; every value here IS one the model declares
/// (`smithy_query(query="shape",shape="Role")`), which the suite below checks.
pub(crate) const STATED_ROLE_VALUES: [&str; 4] = ["USER", "OTHER", "TOOL", "ASSISTANT"];

/// Every enumerated type this service publishes its own order of the value set for, keyed
/// by the set the MODEL declares for that type (`crate::ops::types::*_VALUES`).
///
/// One row per enum whose stated order is known to differ from the declared one. An enum
/// absent from this table is stated as the model declares it, because this service has
/// shown no other statement of it. The row is matched as a SET, so every member targeting
/// one enum — in any operation, in whatever binding carried the value, at any depth — is
/// stated alike.
pub(crate) const STATED_ENUM_VALUE_SETS: &StatedEnumValueSets<'static> =
    &[(ROLE_VALUES, &STATED_ROLE_VALUES)];

/// Whether `violation` is one of the classes this service states WITHOUT the value that
/// violated it: a `@pattern` miss, a violated numeric `@range` bound, or a value outside
/// an enumerated type's set. Such a refusal names the member path and the requirement
/// only.
fn withholds_value(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_)
            | ViolationKind::MaxValue(_)
            | ViolationKind::Pattern(_)
            | ViolationKind::EnumValue(_)
    )
}

/// One violation as THIS SERVICE states it: an enumerated type's value set replaced by the
/// order this service publishes for that set. Every other violation — and an enum this
/// service publishes no order of — comes back unchanged.
fn as_this_service_states_it(violation: &Violation) -> Violation {
    in_stated_enum_order(violation, STATED_ENUM_VALUE_SETS)
}

/// How this service states `refusal`, or `None` to keep the framework's own wording.
///
/// `None` covers what this service does not reword — an authentication refusal is not an
/// input-constraint failure, and a refusal holding only constraint classes it has no
/// recorded wording for keeps the framework's sentence, value and all.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if !violations.iter().any(withholds_value) {
        return None;
    }
    let stated: Vec<Violation> = violations.iter().map(as_this_service_states_it).collect();
    validation_message::aws_validation_refusal_withholding_violations(
        &Refusal::Invalid(stated),
        withholds_value,
    )
}

/// The same refusal the boundary states, over violations a HANDLER collected rather than
/// ones the framework's walk reported.
///
/// For the constraints that walk cannot report — a map KEY, and a leaf inside a map
/// entry's value — which a handler therefore applies itself against the model's own
/// constraints. The clauses, the envelope and the two renderings above are the shared
/// ones, so a constraint a handler enforces is worded exactly as the same constraint
/// reported one level up. Empty only for no violation at all.
///
/// Bounded by the same ceiling the framework's own walk collects under, so the number of
/// clauses one message carries is a constant of the tree rather than something a request
/// chooses.
pub(crate) fn stated_envelope(violations: &[Violation]) -> String {
    let bounded: Vec<Violation> = violations
        .iter()
        .take(service_common::validation::MAX_VIOLATIONS)
        .cloned()
        .collect();
    refused(&Refusal::Invalid(bounded.clone()))
        .map(|stated| stated.message)
        .or_else(|| {
            let clauses: Vec<String> = bounded
                .iter()
                .map(|violation| violation.message())
                .collect();
            validation_message::envelope_over(&clauses)
        })
        .unwrap_or_default()
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

    /// The choices this module makes, stated where a wire case cannot see them: a violated
    /// numeric bound, a `@pattern` miss and a value outside an enum withhold the value —
    /// the pattern unanchored and the value set in the published order — and a violation of
    /// any other class keeps the framework's own clause.
    #[test]
    fn a_withheld_class_states_its_requirement_only_and_no_other_class_does() {
        let role: Vec<String> = ["ASSISTANT", "USER", "TOOL", "OTHER"]
            .iter()
            .map(|value| (*value).to_string())
            .collect();
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
                 have value less than or equal to 28800"
                    .to_string(),
            ),
            (
                "a value below its range minimum",
                violation("viewPort.width", ViolationKind::MinValue("320".to_owned()), Some("319")),
                "Value at 'viewPort.width' failed to satisfy constraint: Member must have \
                 value greater than or equal to 320"
                    .to_string(),
            ),
            (
                "a value outside a whole-value pattern",
                violation(
                    "eventId",
                    ViolationKind::Pattern("^[0-9]+#[a-fA-F0-9]+$".to_owned()),
                    Some("not-an-event-id"),
                ),
                "Value at 'eventId' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: [0-9]+#[a-fA-F0-9]+"
                    .to_string(),
            ),
            (
                "a value outside an enum this service publishes an order of",
                violation(
                    "payload.1.member.conversational.role",
                    ViolationKind::EnumValue(role),
                    Some("NOT_A_VALID_ROLE"),
                ),
                format!(
                    "Value at 'payload.1.member.conversational.role' failed to satisfy \
                     constraint: Member must satisfy enum value set: [{}]",
                    STATED_ROLE_VALUES.join(", ")
                ),
            ),
            (
                "a member past its length maximum, another class",
                violation("name", ViolationKind::MaxLength(100), Some("n")),
                "Value 'n' at 'name' failed to satisfy constraint: Member must have length \
                 less than or equal to 100"
                    .to_string(),
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

    /// Nothing but a constraint refusal of one of the withheld classes is reworded.
    #[test]
    fn nothing_but_a_withheld_class_refusal_is_reworded() {
        for (case, refusal) in [
            ("an unauthenticated caller", Refusal::Unauthenticated),
            ("no violations at all", Refusal::Invalid(Vec::new())),
            (
                "a member past its length maximum",
                Refusal::Invalid(vec![violation(
                    "actorId",
                    ViolationKind::MaxLength(255),
                    Some("an actor id"),
                )]),
            ),
            (
                "an absent required member",
                Refusal::Invalid(vec![violation(
                    "eventTimestamp",
                    ViolationKind::MissingRequiredMember,
                    None,
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

    /// A published order lists exactly the values the model declares, each once: a
    /// statement advertising a value the member would refuse, or missing one it accepts,
    /// would be worse than the declaration order it replaces.
    #[test]
    fn every_published_order_lists_the_declared_values() {
        for (declared, stated) in STATED_ENUM_VALUE_SETS {
            for value in *stated {
                assert!(
                    declared.contains(value),
                    "stated value {value:?} is not one the model declares ({declared:?})"
                );
            }
            let unique: std::collections::BTreeSet<&&str> = stated.iter().collect();
            assert_eq!(
                unique.len(),
                stated.len(),
                "a stated set lists no value twice, got {stated:?}"
            );
            assert_eq!(
                unique.len(),
                declared.len(),
                "a stated set lists every declared value, got {stated:?} for {declared:?}"
            );
        }
    }

    /// The handler path words a violation exactly as the boundary path does, whichever
    /// class it carries, and says nothing for no violation at all.
    #[test]
    fn the_handler_path_and_the_boundary_path_agree() {
        for (case, offence) in [
            (
                "a withheld class",
                violation(
                    "metadata.channel.member.stringValue",
                    ViolationKind::Pattern("^[a-z]*$".to_owned()),
                    Some("web#chat"),
                ),
            ),
            (
                "a class that keeps the framework's wording",
                violation(
                    "metadata.channel.member.stringValue",
                    ViolationKind::MaxLength(256),
                    Some("web"),
                ),
            ),
        ] {
            let expected = match refused(&Refusal::Invalid(vec![offence.clone()])) {
                Some(stated) => stated.message,
                None => format!("1 validation error detected: {}", offence.message()),
            };
            let stated = stated_envelope(std::slice::from_ref(&offence));
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
        assert_eq!(
            stated_envelope(&[]),
            String::new(),
            "no violation is no sentence"
        );
    }

    /// The code travels with the message: a reworded refusal stays the 400
    /// `ValidationException` a client branches on.
    #[test]
    fn a_reworded_refusal_keeps_the_modelled_code() {
        let stated = refused(&Refusal::Invalid(vec![violation(
            "eventId",
            ViolationKind::Pattern("^[0-9]+$".to_owned()),
            Some("nope"),
        )]))
        .expect("a pattern refusal is reworded");
        assert_eq!(
            stated.code, "ValidationException",
            "expected the modelled code, got {:?}",
            stated.code
        );
    }
}

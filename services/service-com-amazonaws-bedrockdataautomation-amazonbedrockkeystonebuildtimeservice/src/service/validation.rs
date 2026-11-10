// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the framework's boundary refusal of a request — the
//! `Service::override_refusal` decision, kept here rather than inline so the rule is stated
//! once for every operation.
//!
//! The words are NOT written here: `service_common::validation_message` is the
//! cross-service home for the envelope, the clauses, the member paths and the requirement
//! sentences. This module makes only the choices that belong to a service.
//!
//! THE RULE this service states: a clause whose requirement is the SET OF ADMISSIBLE
//! VALUES itself — a `@pattern` expression, or an enumerated type's value set — names the
//! member path and that set, and states NO value; a clause whose requirement is a BOUND on
//! the value (a `@length`, a `@range`, a missing member) keeps the value half the framework
//! renders. The set already says what the member admits, so the one value that missed it
//! adds nothing, and a message built that way is bounded by the model instead of by the
//! request. A `@pattern` is quoted as this service advertises the expression, WITHOUT the
//! `^`/`$` a whole-value pattern wraps it in, and a clause about a member the model marks
//! `@sensitive` ([`WITHHELD_MEMBERS`]) states no value whatever its requirement.
//!
//! The code stays the modelled `ValidationException` (HTTP 400) — what the framework
//! already answered — so nothing a client branches on changes, only the words.
//!
//! Nothing here is keyed to an operation, a member or a submitted value, so the rule holds
//! for every constrained member of every operation of this service, at whatever depth and
//! through whatever binding carried the value.

use std::sync::LazyLock;

use service_common::validation_message;
use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// This service's Smithy model: the one source for which of its members are
/// `@sensitive`, and so for which values a boundary refusal states none.
const MODEL_JSON: &str = include_str!(
    "../../smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json"
);

/// The members of this model whose value a refusal WITHHOLDS whatever its requirement, read
/// off [`MODEL_JSON`] rather than listed here, so the decision cannot drift from the model
/// as the model grows a `@sensitive` member.
///
/// This model marks the two resource NAMES (`BlueprintName`,
/// `DataAutomationProjectName`), the library name beside them and the blueprint schema, so
/// a request refused for the shape of a value it submitted is told which member and which
/// requirement, and never handed that value back.
static WITHHELD_MEMBERS: LazyLock<service_common::sensitive_members::WithheldMembers> =
    LazyLock::new(|| service_common::sensitive_members::WithheldMembers::from_model(MODEL_JSON));

/// Whether the clause for `violation` states NO value.
///
/// True for the two classes whose requirement IS the set of values the member admits — a
/// `@pattern` expression and an enumerated type's value set — and for every violation of a
/// member the model marks `@sensitive`.
fn withholds_value(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::Pattern(_) | ViolationKind::EnumValue(_)
    ) || WITHHELD_MEMBERS.withholds_value(&violation.path)
}

/// How this service states `refusal`, or `None` to keep the framework's own wording for
/// anything that is not a constraint refusal (an unauthenticated caller keeps the
/// framework's own code, status and words).
pub fn refused(refusal: &Refusal) -> Option<Refused> {
    validation_message::aws_validation_refusal_withholding_violations(refusal, withholds_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of `kind` at `path`, holding `value` — a framework refusal as the
    /// validating walk reports it.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    #[test]
    fn a_pattern_clause_states_the_expression_and_no_value() {
        let refusal = Refusal::Invalid(vec![violation(
            "libraryArn",
            ViolationKind::Pattern("^arn:aws:bedrock:[a-z0-9-]*$".to_string()),
            Some("arn:aws:bedrock:us-west-2:000000000000:blueprint/x"),
        )]);
        let stated = refused(&refusal).expect("a constraint refusal is restated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'libraryArn' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: arn:aws:bedrock:[a-z0-9-]*",
            "expected the pattern clause to state no value, got {:?}",
            stated.message
        );
    }

    #[test]
    fn an_enum_clause_states_the_value_set_and_no_value() {
        let refusal = Refusal::Invalid(vec![violation(
            "entityType",
            ViolationKind::EnumValue(vec!["VOCABULARY".to_string(), "RECOGNIZED_PERSON".to_string()]),
            Some("NOPE"),
        )]);
        let stated = refused(&refusal).expect("a constraint refusal is restated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'entityType' failed to satisfy constraint: \
             Member must satisfy enum value set: [VOCABULARY, RECOGNIZED_PERSON]",
            "expected the enum clause to state no value, got {:?}",
            stated.message
        );
    }

    #[test]
    fn a_bound_clause_keeps_the_value_the_framework_renders() {
        let refusal = Refusal::Invalid(vec![violation(
            "clientToken",
            ViolationKind::MinLength(33),
            Some("abc"),
        )]);
        let stated = refused(&refusal).expect("a constraint refusal is restated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value 'abc' at 'clientToken' failed to satisfy \
             constraint: Member must have length greater than or equal to 33",
            "expected a bound clause to keep its value, got {:?}",
            stated.message
        );
    }

    #[test]
    fn a_sensitive_member_states_no_value_for_a_bound_either() {
        let refusal = Refusal::Invalid(vec![violation(
            "blueprintName",
            ViolationKind::MaxLength(128),
            Some("a-name-the-caller-sent"),
        )]);
        let stated = refused(&refusal).expect("a constraint refusal is restated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'blueprintName' failed to satisfy \
             constraint: Member must have length less than or equal to 128",
            "expected a withheld member to state no value, got {:?}",
            stated.message
        );
    }
}

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
//! Almost all of it is the tree's shared wording
//! ([`::service_common::validation_message::aws_validation_refusal_withholding_with`]),
//! which is the framework's envelope over the framework's clauses with the two
//! differences AWS states: the value half dropped from a clause that states none
//! — a collection's `@length`, and a member whose value the refusal WITHHOLDS —
//! and the `@pattern` expression quoted as this service's recordings show it,
//! which is the model's own spelling, anchors and all. WHICH members are
//! withheld is read off this service's MODEL
//! ([`::service_common::sensitive_members::WithheldMembers`]), so nothing here
//! lists a member name and the decision cannot fall behind a model that grows a
//! `@sensitive` one.
//!
//! What is left for this module is ONE difference, which no shared entry point
//! can make for it: Step Functions echoes a rejected value as the member's
//! MODELLED TYPE renders it. A member whose target shape is a Smithy
//! `float`/`double` renders with a fractional part (`150` -> `150.0`), where an
//! integral member (`integer`, `long`) renders without one — and Rust's own
//! `Display` for a float drops a zero fraction (`150f32.to_string() == "150"`),
//! so the framework's default clause reports a float member's value in an
//! integer's spelling. The violations are re-rendered that way and then handed
//! to the shared wording, so the envelope, the clauses, the member paths, the
//! requirements and the withholding are all stated in one place.
//!
//! Nothing here is keyed to an operation or to a particular value: the rule is
//! applied to whatever [`Violation`] the walk produces, so it holds for every
//! constrained member of every operation of this service.
//!
//! The structural fix is still owed upstream — a `Violation` carrying the
//! member's modelled numeric kind, or a framework that renders a value per
//! modelled type — which would remove this module entirely.

use std::sync::LazyLock;

use smithy_server_runtime::{Refusal, Refused, Violation};
use ::aws_smithy_schema_server::ViolationKind;

use crate::state::FRACTIONAL_MEMBERS;

/// This service's Smithy model: the one source for which of its members are
/// `@sensitive`, and so for which values a refusal states none.
const MODEL_JSON: &str = include_str!("../../smithy-model/com-amazonaws-sfn-awsstepfunctions.json");

/// The members of this model whose value a refusal WITHHOLDS, read off
/// [`MODEL_JSON`] rather than listed here.
///
/// This model marks the documents a caller submits and the data they carry:
/// `Definition`, the `SensitiveData` members (an execution `input`, a tested
/// state's `input`, `variables`, `context`, a mock's `result`, a task's
/// `output`), a task token, a description, and a failure's `error` and `cause`.
static WITHHELD_MEMBERS: LazyLock<::service_common::sensitive_members::WithheldMembers> =
    LazyLock::new(|| {
        ::service_common::sensitive_members::WithheldMembers::from_model(MODEL_JSON)
    });

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
/// reported it, so the shared wording and the framework word the clause.
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
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let in_modelled_types: Vec<Violation> = violations.iter().map(in_modelled_type).collect();
    let mut stated = shared_wording(&in_modelled_types)?;
    // A member whose bound is a payload SIZE states the requirement about that size
    // rather than the character-length sentence the framework states for every other
    // member, so a refusal carrying one is restated clause by clause: each clause by
    // the rule its own member's bound follows, inside the one envelope.
    if in_modelled_types.iter().any(|violation| payload_limit(violation).is_some()) {
        let clauses: Vec<String> = in_modelled_types.iter().map(stated_clause).collect();
        stated.message = ::service_common::validation_message::envelope_over(&clauses)?;
    }
    Some(stated)
}

/// The shared wording's statement of `violations`: the framework's envelope over the
/// framework's clauses, with this service's two differences — the `@pattern`
/// expression as the model writes it, and the value half dropped from a `@sensitive`
/// member's clause.
fn shared_wording(violations: &[Violation]) -> Option<Refused> {
    ::service_common::validation_message::aws_validation_refusal_withholding_with(
        &Refusal::Invalid(violations.to_vec()),
        // The expression a `@pattern` clause advertises is the model's own, as
        // the framework already quotes it: this service is recorded stating the
        // alias-name pattern with the anchors the model writes, so only the
        // value half below is restated.
        |pattern| pattern.to_string(),
        |path| WITHHELD_MEMBERS.withholds_value(path),
    )
}

/// The maximum this violation's member is bounded by in UTF-8 BYTES, or `None` when
/// its bound is not a payload size — including a violation of another class, since a
/// payload member is only measured in bytes against its `@length` MAXIMUM.
fn payload_limit(violation: &Violation) -> Option<usize> {
    match violation.kind {
        ViolationKind::MaxLength(_) => {
            crate::state::payload_size::PAYLOAD_MEMBERS.limit_at(&violation.path)
        }
        _ => None,
    }
}

/// The clause `violation` is stated by: the shared wording's own, with the REQUIREMENT
/// restated for a member whose bound is a payload size in UTF-8 bytes.
///
/// Everything else about the clause — the member path, the value half and the fact
/// that a `@sensitive` member states none — is the shared wording's, which is why the
/// member half is taken from the clause it already stated rather than rebuilt here:
/// only the requirement sentence differs, and it is swapped for the one the model's
/// own documentation states ("Length constraints apply to the payload size, and are
/// expressed as bytes in UTF-8 encoding").
fn stated_clause(violation: &Violation) -> String {
    let shared = shared_clause(violation);
    let Some(max_bytes) = payload_limit(violation) else {
        return shared;
    };
    let framework = ::service_common::validation_message::requirement(violation.kind.clone());
    let requirement = crate::state::payload_size::byte_size_requirement(max_bytes);
    match shared.strip_suffix(&framework) {
        Some(member_half) => format!("{member_half}{requirement}"),
        None => shared,
    }
}

/// The shared wording's clause for ONE violation: its whole statement with the
/// envelope that wording puts around a single clause removed, so a clause this module
/// restates and a clause it does not can sit inside one envelope.
///
/// The envelope's own opening is read back out of the shared wording
/// ([`::service_common::validation_message::envelope_over`] over one empty clause)
/// rather than spelled here, so nothing about the envelope is stated twice.
fn shared_clause(violation: &Violation) -> String {
    let stated = shared_wording(::std::slice::from_ref(violation))
        .map(|refused| refused.message)
        .unwrap_or_default();
    let opening = ::service_common::validation_message::envelope_over(&[String::new()])
        .unwrap_or_default();
    stated
        .strip_prefix(&opening)
        .unwrap_or(stated.as_str())
        .to_string()
}

/// The standard validation envelope for ONE violation a HANDLER found, as the
/// whole message it states.
///
/// A modelled constraint the centralized walk cannot enforce is still a modelled
/// constraint, so a handler that enforces one words its refusal through the SAME
/// body the boundary's refusals are worded by — [`refused`] — rather than
/// building a second envelope: the clause, the requirement, the member path, the
/// `@pattern` quoting and the withholding are all decided in the one place.
pub(crate) fn one_violation_message(violation: Violation) -> String {
    refused(&Refusal::Invalid(vec![violation]))
        .map(|refused| refused.message)
        // One violation always renders: the envelope answers `None` only for an
        // empty list.
        .unwrap_or_default()
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

    /// The members this service withholds are the ones its MODEL marks
    /// `@sensitive` on every input member of that name — read off the model
    /// here, so the shared reading and this service's model cannot drift.
    ///
    /// The rows span the forms the model carries: the trait on the shape a
    /// member targets (a definition, a document, a token), a member of a
    /// structure the caller sends nested inside another
    /// (`mock.result`), a member sensitive only in an OUTPUT shape (whose
    /// clause still states its value), and members the model marks nowhere.
    #[test]
    fn the_model_decides_which_members_withhold_their_value() {
        // (case, the member path a refusal reports, withheld?)
        for (case, path, expected) in [
            ("a state machine definition", "definition", true),
            ("an execution input", "input", true),
            ("a tested state's variables", "variables", true),
            ("the state under test", "stateName", true),
            ("a mocked result, nested in its structure", "mock.result", true),
            ("an alias description", "description", true),
            ("a version description", "versionDescription", true),
            ("a task token", "taskToken", true),
            ("a failure cause", "cause", true),
            // Not withheld: the names this model constrains and echoes.
            ("a state machine name", "name", false),
            ("a state machine ARN", "stateMachineArn", false),
            ("a Map Run's concurrency", "maxConcurrency", false),
            (
                "a Map Run's tolerated failure percentage",
                "toleratedFailurePercentage",
                false,
            ),
            ("a tag key", "tags.1.member.key", false),
            ("a pagination token", "nextToken", false),
        ] {
            let withheld = WITHHELD_MEMBERS.withholds_value(path);
            assert_eq!(
                withheld, expected,
                "case {case:?}: expected withholds_value({path:?}) to be {expected}, got \
                 {withheld} (the withheld set is {:?})",
                WITHHELD_MEMBERS.names()
            );
        }
    }
}

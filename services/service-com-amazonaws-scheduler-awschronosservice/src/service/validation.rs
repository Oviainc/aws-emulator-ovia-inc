// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-validation refusal: how the constraint violations the
//! framework's own boundary walk found become the `message` a 400 `ValidationException`
//! carries.
//!
//! [`crate::service`] owns the SEAM — which requests are validated, and how the violations
//! are collected with the values the caller submitted — and this module owns the sentence,
//! so the clause rules live in one place instead of in each operation that trips them. What
//! is ENFORCED is always the model's: every violation handed to [`refusal`] comes from the
//! framework's own walk, and nothing here accepts or refuses anything.
//!
//! The rules, each holding for every member of every operation rather than for the members
//! one report happened to cite:
//!
//! 1. the envelope is AWS's `<N> validation error(s) detected: ` over one clause per
//!    violation, joined with `; ` ([`envelope`]);
//! 2. a clause states the value as SUBMITTED, the member's dotted path and the requirement
//!    it failed, in the framework's own words ([`clause`]) — the value in full, since the
//!    message is the only place the service reports the offending input back and a caller
//!    that compares the echo against what it sent has to see its own bytes;
//! 3. a reported `@pattern` is the expression without the `^`/`$` anchors a model writes to
//!    demand a whole-string match (`service_common::validation::unanchored`);
//! 4. an enum's value set is listed as the service's OWN enum declares it, which for one of
//!    this service's enums is not the order the public model publishes
//!    ([`REPORTED_VALUE_SETS`]).
//!
//! The status (400) and the error code ([`VALIDATION_EXCEPTION`]) are what the framework
//! already answers a constraint failure with on this `restJson1` service and are restated,
//! not changed: a chosen [`Refused`] replaces the code and the message together.

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use ::smithy_server_runtime::Refused;

/// The error code a constraint failure is named by: the modelled `ValidationException`,
/// which every operation that can be refused declares with `@httpError 400`. It is also the
/// framework's own default code for a refusal, restated here because a chosen [`Refused`]
/// carries both halves — a message without its matching code is worse than leaving both
/// alone.
pub(super) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The value sets this service's own validator REPORTS, keyed by the set the public Smithy
/// model declares.
///
/// A value set is identified by the values it holds, because a runtime [`Violation`] carries
/// the set rather than the shape it came from. Every enum NOT listed here is reported exactly
/// as the model declares it — `ScheduleState` and `AssignPublicIp` (`[ENABLED, DISABLED]`),
/// `FlexibleTimeWindowMode` (`[OFF, FLEXIBLE]`), `ScheduleGroupState` (`[ACTIVE, DELETING]`),
/// `LaunchType` (`[EC2, FARGATE, EXTERNAL]`) and `PropagateTags` (`[TASK_DEFINITION]`)
/// (`smithy_query(query="shape", shape="<Enum>")` for each).
const REPORTED_VALUE_SETS: &[(&[&str], &[&str])] = &[
    // `ActionAfterCompletion`. The public model declares `[NONE, DELETE]`
    // (`smithy_query(query="shape", shape="com.amazonaws.scheduler#ActionAfterCompletion")`),
    // while the service's recorded refusal of an out-of-range value lists the set as
    // `[DELETE, NONE]`: it validates against its own enum, whose values are declared in that
    // order. Only the REPORTED order changes — which values a request may carry stays the
    // model's, which is what the boundary walk enforces.
    (&["NONE", "DELETE"], &["DELETE", "NONE"]),
];

/// This service's wording for the violations the framework found, or `None` when there is
/// nothing to word (no violations), which keeps the framework's own.
pub(super) fn refusal(violations: &[Violation]) -> Option<Refused> {
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: message(violations)?,
    })
}

/// The `message` for a set of violations: the count, then one clause per violation in the
/// order the walk found them.
fn message(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    envelope(&clauses)
}

/// AWS's envelope over the clauses of one refusal: `1 validation error detected: <clause>`
/// for one, `<N> validation errors detected: <clause>; <clause>` for several.
fn envelope(clauses: &[String]) -> Option<String> {
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// One clause, in the framework's own sentence — the value, the member path and the
/// requirement — with the constraint restated as this service reports it ([`as_reported`]).
///
/// The sentence is read off [`Violation::message`] rather than re-spelled here, so a
/// constraint class the framework words differently (or one it gains) cannot drift out of
/// step with it; and restating the KIND rather than editing the rendered sentence means a
/// caller that submits the pattern text as its value cannot have its own value rewritten.
fn clause(violation: &Violation) -> String {
    Violation {
        path: violation.path.clone(),
        kind: as_reported(&violation.kind),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
    .message()
}

/// A violated constraint as this service reports it: a `@pattern` without its whole-value
/// anchors, an enum value set as the service's own enum declares it, and every other class
/// exactly as the framework describes it.
fn as_reported(kind: &ViolationKind) -> ViolationKind {
    match kind {
        ViolationKind::Pattern(pattern) => {
            ViolationKind::Pattern(::service_common::validation::unanchored(pattern))
        }
        ViolationKind::EnumValue(values) => ViolationKind::EnumValue(reported_value_set(values)),
        other => other.clone(),
    }
}

/// `values` as the service's own enum declares them ([`REPORTED_VALUE_SETS`]), or unchanged
/// when the model's set is the one the service reports.
fn reported_value_set(values: &[String]) -> Vec<String> {
    for (declared, reported) in REPORTED_VALUE_SETS {
        if is_set_of(values, declared) {
            return reported.iter().map(|value| (*value).to_string()).collect();
        }
    }
    values.to_vec()
}

/// Whether `values` holds exactly the values `expected` does, in any order.
fn is_set_of(values: &[String], expected: &[&str]) -> bool {
    values.len() == expected.len()
        && expected
            .iter()
            .all(|wanted| values.iter().any(|value| value == wanted))
}

#[cfg(test)]
mod tests {
    //! The clause rules this module owns, each over a SET of inputs rather than the one
    //! recorded case. The same rules are pinned on the wire, through real requests, in
    //! `tests/all/schedule_definition_rules.rs` and `tests/all/reserved_schedule_group.rs`.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-scheduler-awschronosservice.json"
    );

    /// Every enum this model declares, as (shape id, the values it declares).
    ///
    /// A `@enum`-trait string is how this model spells an enum
    /// (`"traits": {"smithy.api#enum": [{"value": …}]}`), which is also the form the
    /// framework's schema carries into a [`ViolationKind::EnumValue`] set.
    fn modelled_enums() -> Vec<(String, Vec<String>)> {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: ::serde_json::Value =
            ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes")
            .clone();
        shapes
            .into_iter()
            .filter_map(|(id, shape)| {
                let values = shape["traits"]["smithy.api#enum"].as_array()?;
                let values: Vec<String> = values
                    .iter()
                    .filter_map(|entry| entry["value"].as_str().map(str::to_string))
                    .collect();
                Some((id, values))
            })
            .collect()
    }

    /// GUARD on how [`REPORTED_VALUE_SETS`] identifies an enum: a runtime violation carries
    /// the value SET rather than the shape it came from, so each key must be a set exactly
    /// ONE enum of this model declares — otherwise a second enum sharing it would silently
    /// be reported with the wrong set.
    #[test]
    fn each_reported_value_set_names_exactly_one_enum_of_the_model() {
        let enums = modelled_enums();
        assert!(
            enums.len() >= 4,
            "case the model's enums: expected this service's enums, got {enums:?}"
        );

        for (declared, reported) in REPORTED_VALUE_SETS {
            let matching: Vec<&str> = enums
                .iter()
                .filter(|(_, values)| is_set_of(values, declared))
                .map(|(id, _)| id.as_str())
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "case the set {declared:?} (reported as {reported:?}): exactly one enum of \
                 this model may declare it, got {matching:?}"
            );
        }
    }

    /// A reported set holds exactly the values the model declares: the ORDER is the
    /// service's, the membership is the model's.
    #[test]
    fn a_reported_value_set_holds_the_values_the_model_declares() {
        for (declared, reported) in REPORTED_VALUE_SETS {
            let declared: Vec<String> = declared.iter().map(|value| (*value).to_string()).collect();
            let reported: Vec<String> = reported.iter().map(|value| (*value).to_string()).collect();
            assert!(
                is_set_of(&reported, &declared.iter().map(String::as_str).collect::<Vec<_>>()),
                "case the set {declared:?}: a reported set may reorder the model's values, \
                 never change them, got {reported:?}"
            );
            assert_eq!(
                reported_value_set(&declared),
                reported,
                "case the set {declared:?}: expected it reported as {reported:?}"
            );
        }
    }

    /// Every other class is worded exactly as the framework words it, and a set no entry
    /// names is reported as the model declares it.
    #[test]
    fn a_class_this_service_does_not_restate_keeps_the_frameworks_wording() {
        let untouched = [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxLength(512),
            ViolationKind::MinValue("1".to_string()),
            ViolationKind::MaxValue("1440".to_string()),
            ViolationKind::EnumValue(vec!["ENABLED".to_string(), "DISABLED".to_string()]),
            ViolationKind::UniqueItems,
        ];
        for kind in untouched {
            assert_eq!(
                as_reported(&kind),
                kind,
                "case {kind:?}: expected the framework's own description, unchanged"
            );
        }
    }

    /// A `@pattern` loses the whole-value anchors the model spells the total match with.
    #[test]
    fn a_pattern_is_reported_without_its_whole_value_anchors() {
        // (case, modelled pattern, reported pattern). Source: `smithy_query(query="shape",
        // shape="com.amazonaws.scheduler#<Shape>")` for each.
        let rows: [(&str, &str, &str); 4] = [
            ("ScheduleGroupName", r"^[0-9a-zA-Z-_.]+$", r"[0-9a-zA-Z-_.]+"),
            ("ClientToken", r"^[a-zA-Z0-9-_]+$", r"[a-zA-Z0-9-_]+"),
            (
                "RoleArn",
                r"^arn:aws(-[a-z]+)?:iam::\d{12}:role\/[\w+=,.@\/-]+$",
                r"arn:aws(-[a-z]+)?:iam::\d{12}:role\/[\w+=,.@\/-]+",
            ),
            // An expression with no anchors at all is reported as written.
            ("an unanchored expression", r"[a-z]+", r"[a-z]+"),
        ];

        for (case, modelled, reported) in rows {
            assert_eq!(
                as_reported(&ViolationKind::Pattern(modelled.to_string())),
                ViolationKind::Pattern(reported.to_string()),
                "case {case:?}: expected the expression reported as {reported:?}"
            );
        }
    }

    /// The envelope counts the clauses and joins them AWS's way.
    #[test]
    fn the_envelope_counts_and_joins_the_clauses() {
        // (case, clauses, envelope)
        let rows: [(&str, &[&str], Option<&str>); 3] = [
            ("nothing to word", &[], None),
            (
                "one clause",
                &["Value 'x' at 'name' failed"],
                Some("1 validation error detected: Value 'x' at 'name' failed"),
            ),
            (
                "two clauses",
                &["a", "b"],
                Some("2 validation errors detected: a; b"),
            ),
        ];

        for (case, clauses, expected) in rows {
            let clauses: Vec<String> = clauses.iter().map(|text| (*text).to_string()).collect();
            assert_eq!(
                envelope(&clauses).as_deref(),
                expected,
                "case {case:?}: expected {expected:?}"
            );
        }
    }
}

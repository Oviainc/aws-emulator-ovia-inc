// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-validation refusal: how the constraint violations
//! the framework found become the `message` a 400 `ValidationException` carries.
//!
//! [`crate::service`] owns the SEAM — which requests are validated, and how the violations
//! and the submitted values are collected — and this module owns the sentence, so the
//! clause rules live in one place instead of in each operation that trips them. What is
//! ENFORCED is always the model's: every violation handed to [`refusal`] comes from the
//! framework's own boundary walk, and nothing here accepts or refuses anything.
//!
//! The rules, each holding for every member of every operation rather than for the members
//! one report happened to cite:
//!
//! 1. the envelope is AWS's `<N> validation error(s) detected: ` over one clause per
//!    failing (member, requirement), joined with `; ` ([`envelope`]);
//! 2. a clause states the value as SUBMITTED, the member's dotted path and the requirement
//!    it failed, in the framework's own words ([`framework_requirement`]);
//! 3. a reported `@pattern` is the expression without the `^`/`$` anchors a model writes to
//!    demand a whole-string match ([`unanchored`]);
//! 4. a violation on the ELEMENTS of a list is stated once, at the LIST member, with the
//!    element's requirement nested as `Member must satisfy constraint: [<requirement>]` and
//!    the list itself as the value (`service_common::validation`'s [`list_of_element`] and
//!    `submitted_lists`, which own both the path spelling and the list rendering);
//! 5. an enum's value set is listed as the service's OWN enum declares it — which for two
//!    of this service's enums is neither the order nor the content the public model
//!    publishes ([`REPORTED_VALUE_SETS`]).
//!
//! The status (400) and the error code ([`VALIDATION_EXCEPTION`]) are what the framework
//! already answers a constraint failure with on this `restJson1` service and are restated,
//! not changed: a chosen [`Refused`] replaces the code and the message together.

use std::collections::{HashMap, HashSet};

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use service_common::validation::list_of_element;
use smithy_server_runtime::Refused;

/// The error code a constraint failure is named by, unchanged from the one the framework's
/// refusal channel picks for a `restJson1` service. Restated here because a chosen
/// [`Refused`] carries both halves: a message without its matching code is worse than
/// leaving both alone.
pub(super) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text that separates a clause's subject from its requirement, in the framework's own
/// wording ([`Violation::message`]). Split on, never written: see
/// [`framework_requirement`].
const CLAUSE_MARKER: &str = "failed to satisfy constraint: ";

/// The value sets this service's own validator REPORTS, keyed by the set the public Smithy
/// model declares.
///
/// A value set is identified by the values it holds, because a runtime [`Violation`]
/// carries the set rather than the shape it came from — the same identification
/// `service-com-amazonaws-apigateway-backplanecontrolservice` makes for the one enum its
/// recordings pin. Every enum NOT listed here is reported exactly as the model declares
/// it: `ComputePlatform` (`[Default, AWSLambda]`), `EventPublisher`
/// (`[AnomalyDetection]`), `OrderBy` (`[TimestampDescending, TimestampAscending]`),
/// `FeedbackType`, `MetricType`, `AgentParameterField` and `MetadataField`
/// (`smithy_query(query="shape", shape="<Enum>")` for each).
const REPORTED_VALUE_SETS: &[(&[&str], &[&str])] = &[
    // `AggregationPeriod`. The model declares `[PT5M, PT1H, P1D]`
    // (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#AggregationPeriod")`);
    // every recorded `ListProfileTimes` refusal of an invalid `period` lists the set in the
    // order below, so that is the order the service's own enum declares its values in.
    (&["PT5M", "PT1H", "P1D"], &["P1D", "PT1H", "PT5M"]),
    // `ActionGroup`. The public model declares the single value `[agentPermissions]`, while
    // the recorded `PutPermission` refusal of an invalid `actionGroup` reports
    // `[internalAgentPermissions, agentPermissions]`: the service validates against its own
    // enum, which carries an `internalAgentPermissions` value the public model omits. Only
    // the REPORTED set widens — what the request may carry stays the model's, which is what
    // the boundary walk enforces.
    (
        &["agentPermissions"],
        &["internalAgentPermissions", "agentPermissions"],
    ),
];

/// This service's wording for the violations the framework found, or `None` when there is
/// nothing to word (no violations) or a violation class this module has no wording for
/// ([`ViolationKind`] is `#[non_exhaustive]`), which keeps the framework's own.
///
/// `lists` yields every list of scalar elements the request carried, keyed by the member
/// path the walk reports that list at and already rendered
/// (`service_common::validation::submitted_lists`). It is a closure because it is only
/// needed for rule 4 — a violation found on a list ELEMENT, whose clause states the whole
/// list — so a refusal that has none never walks the request again.
pub(super) fn refusal(
    violations: &[Violation],
    lists: impl FnOnce() -> HashMap<String, String>,
) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let lists = if violations
        .iter()
        .any(|violation| list_of_element(&violation.path).is_some())
    {
        lists()
    } else {
        HashMap::new()
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: message(violations, &lists)?,
    })
}

/// The `message` for a set of violations: the count, then one clause per failing
/// (member, requirement) in the order the walk found them.
///
/// A violation found on a list element is stated at the LIST member with its requirement
/// nested, which is why two bad elements of one list are ONE clause rather than two, and
/// why a member that broke two different rules still states both.
fn message(violations: &[Violation], lists: &HashMap<String, String>) -> Option<String> {
    let mut stated: HashSet<(String, String)> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let (path, element) = match list_of_element(&violation.path) {
            Some(list) => (list.to_string(), true),
            None => (violation.path.clone(), false),
        };
        let requirement = match requirement_of(&violation.kind) {
            Some(requirement) if element => {
                format!("Member must satisfy constraint: [{requirement}]")
            }
            Some(requirement) => requirement,
            // A class with no wording here would otherwise be stated as a clause with no
            // requirement at all; the framework's whole message stands instead.
            None => return None,
        };
        if !stated.insert((path.clone(), requirement.clone())) {
            continue;
        }
        let subject = subject(violation, element.then(|| lists.get(&path)).flatten());
        clauses.push(format!(
            "{subject} at '{path}' failed to satisfy constraint: {requirement}"
        ));
    }
    envelope(&clauses)
}

/// AWS's envelope over the clauses of one refusal: `1 validation error detected: <clause>`
/// for one, `<N> validation errors detected: <clause>; <clause>` for several.
///
/// The wording is `aws_smithy_schema_server::envelope`'s, which cannot be CALLED here: it
/// renders each clause from a [`Violation`], and rule 4's nested requirement is not a
/// [`ViolationKind`] any violation can carry. The two must stay identical, which
/// [`tests::the_envelope_counts_and_joins_clauses_the_way_the_framework_does`] pins against
/// that function itself.
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

/// The rule half of a clause: what the member had to satisfy, with the two renderings this
/// service does not share with the framework applied first.
fn requirement_of(kind: &ViolationKind) -> Option<String> {
    framework_requirement(&as_reported(kind))
}

/// A violated constraint as this service reports it: a `@pattern` without its whole-value
/// anchors, an enum value set as the service's own enum declares it, and every other class
/// exactly as the framework describes it.
fn as_reported(kind: &ViolationKind) -> ViolationKind {
    match kind {
        ViolationKind::Pattern(pattern) => ViolationKind::Pattern(unanchored(pattern)),
        ViolationKind::EnumValue(values) => ViolationKind::EnumValue(reported_value_set(values)),
        other => other.clone(),
    }
}

/// The framework's own sentence for `kind`, recovered from a violation with NO value and an
/// EMPTY path so that neither a rejected value nor a caller-chosen map key in a member path
/// can reach the requirement half.
///
/// Read off [`Violation::message`] rather than re-spelled here, so a constraint class the
/// framework words differently (or one it gains) cannot drift out of step with it;
/// `None` when the marker is absent, which leaves the framework's whole message standing.
fn framework_requirement(kind: &ViolationKind) -> Option<String> {
    let stated = Violation {
        path: String::new(),
        kind: kind.clone(),
        value: None,
        sensitive: false,
    }
    .message();
    stated
        .rsplit_once(CLAUSE_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// The value half of a clause.
///
/// * an ABSENT member has no value at all and is reported as the null it was;
/// * a clause stated at a LIST member because one of its ELEMENTS failed states the list
///   the caller sent, when `service_common::validation::submitted_lists` could render that
///   list in full;
/// * everything else echoes the value carried on the violation, which the seam has already
///   restored to the bytes the caller submitted.
fn subject(violation: &Violation, rendered_list: Option<&String>) -> String {
    match (&violation.value, rendered_list) {
        (None, _) => "Value null".to_string(),
        (Some(_), Some(rendered)) => format!("Value '{rendered}'"),
        (Some(shown), None) => format!("Value '{shown}'"),
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

/// `regex` without the whole-value anchors a model adds to express the match AWS's own
/// pattern makes implicitly — read from the shared module, which is the staged home for a
/// rendering that is AWS's rather than any one service's.
fn unanchored(regex: &str) -> String {
    service_common::validation::unanchored(regex)
}

#[cfg(test)]
mod tests {
    //! The clause rules this module owns, each over a SET of inputs rather than the one
    //! recorded case. The same rules are pinned on the wire, through real requests, in
    //! `tests/all/refusal_message_rules.rs`.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-codeguruprofiler-codeguruprofiler.json"
    );

    /// Every enum this model declares, as (shape id, the values it declares).
    ///
    /// A `@enum`-trait string is how this model spells an enum
    /// (`"traits": {"smithy.api#enum": [{"value": …}]}`), which is also the form the
    /// framework's schema carries into a [`ViolationKind::EnumValue`] set.
    fn modelled_enums() -> Vec<(String, Vec<String>)> {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
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
            enums.len() >= 9,
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

    /// A pattern loses the whole-value anchor PAIR and nothing else.
    #[test]
    fn a_pattern_is_reported_without_its_whole_value_anchors() {
        // (case, modelled pattern, reported pattern)
        let rows: [(&str, &str, &str); 12] = [
            // Source: `smithy_query(query="shape",
            // shape="com.amazonaws.codeguruprofiler#ProfilingGroupName")`.
            ("the modelled group name", r"^[\w-]+$", r"[\w-]+"),
            // Source: the same query for `com.amazonaws.codeguruprofiler#ChannelId`, which
            // the model writes with NO anchors at all.
            (
                "the modelled channel id",
                "[a-f0-9]{8}-[a-f0-9]{4}",
                "[a-f0-9]{8}-[a-f0-9]{4}",
            ),
            ("an anchored character class", "^[a-z]+$", "[a-z]+"),
            ("anchored at the start only", "^arn:aws:", "^arn:aws:"),
            ("anchored at the end only", "[0-9]+$", "[0-9]+$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            ("an escaped trailing dollar", r"^[a-z]+\$$", r"[a-z]+\$"),
            // The pair is dropped POSITIONALLY — see
            // `service_common::validation::unanchored`, which these rows read the rule
            // from: the published expression is the service's own with that pair written
            // around it, so an alternation does not keep it (a lambdacore recording shows
            // AWS dropping it from a top-level alternation).
            ("a fully anchored alternation", "^a$|^b$", "a$|^b"),
            ("a half-anchored alternation", "^a|b$", "a|b"),
            ("an alternation inside a group", "^(a|b)$", "(a|b)"),
            ("a class holding a literal caret", r"^[\^a-z]+$", r"[\^a-z]+"),
            ("a negated character class", r"^[^/]+$", r"[^/]+"),
        ];

        for (case, modelled, expected) in rows {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?} ({modelled}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A value set is listed as the service's own enum declares it: the recorded set for the
    /// two enums the recordings pin, the model's own for every other.
    #[test]
    fn an_enum_value_set_is_listed_as_the_service_declares_it() {
        // (case, the set as the model declares it, the set as the service reports it)
        let rows: [(&str, &[&str], &[&str]); 6] = [
            (
                "AggregationPeriod, in the model's declaration order",
                &["PT5M", "PT1H", "P1D"],
                &["P1D", "PT1H", "PT5M"],
            ),
            (
                "AggregationPeriod, already in the service's order",
                &["P1D", "PT1H", "PT5M"],
                &["P1D", "PT1H", "PT5M"],
            ),
            (
                "ActionGroup, which the model publishes narrower",
                &["agentPermissions"],
                &["internalAgentPermissions", "agentPermissions"],
            ),
            // Source: `smithy_query(query="shape",
            // shape="com.amazonaws.codeguruprofiler#ComputePlatform")`.
            (
                "ComputePlatform",
                &["Default", "AWSLambda"],
                &["Default", "AWSLambda"],
            ),
            // Source: the same query for `#EventPublisher` — a single-value enum that is
            // NOT `ActionGroup`, so it keeps its own set.
            (
                "EventPublisher",
                &["AnomalyDetection"],
                &["AnomalyDetection"],
            ),
            // Source: the same query for `#OrderBy`.
            (
                "OrderBy",
                &["TimestampDescending", "TimestampAscending"],
                &["TimestampDescending", "TimestampAscending"],
            ),
        ];

        for (case, declared, expected) in rows {
            let given: Vec<String> = declared.iter().map(|value| (*value).to_string()).collect();
            let actual = reported_value_set(&given);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every constraint class is stated in the framework's own words, with only the two
    /// renderings above changed — and an element failure is stated at its list.
    #[test]
    fn every_constraint_class_is_stated_in_the_frameworks_words() {
        // A value longer than the framework's own echo cap (256 bytes), to pin that the
        // clause carries it whole. Bound outside the table: a `format!` temporary inside an
        // array literal does not outlive the statement that builds it.
        let long_id = format!("i-{}", "0".repeat(300));
        let long_clause = format!(
            "1 validation error detected: Value '{long_id}' at 'fleetInstanceId' failed to \
             satisfy constraint: Member must have length less than or equal to 255"
        );

        // (case, the violation the framework found, the clause this service states)
        let rows: [(&str, Violation, &str); 7] = [
            (
                "a pattern on a plain member",
                Violation {
                    path: "profilingGroupName".to_string(),
                    kind: ViolationKind::Pattern(r"^[\w-]+$".to_string()),
                    value: Some("has a space".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value 'has a space' at 'profilingGroupName' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: [\\w-]+",
            ),
            (
                "a length bound, with the value echoed whole",
                Violation {
                    path: "fleetInstanceId".to_string(),
                    kind: ViolationKind::MaxLength(255),
                    value: Some(long_id.clone()),
                    sensitive: false,
                },
                &long_clause,
            ),
            (
                "an absent required member",
                Violation {
                    path: "channels".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                "1 validation error detected: Value null at 'channels' failed to satisfy \
                 constraint: Member must not be null",
            ),
            (
                "a collection bound, stated at the collection",
                Violation {
                    path: "channels".to_string(),
                    kind: ViolationKind::MaxLength(2),
                    value: Some("3 items".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value '3 items' at 'channels' failed to \
                 satisfy constraint: Member must have length less than or equal to 2",
            ),
            (
                "a numeric bound",
                Violation {
                    path: "maxResults".to_string(),
                    kind: ViolationKind::MaxValue("1000".to_string()),
                    value: Some("5000".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value '5000' at 'maxResults' failed to \
                 satisfy constraint: Member must have value less than or equal to 1000",
            ),
            (
                "an enum on a plain member",
                Violation {
                    path: "period".to_string(),
                    kind: ViolationKind::EnumValue(
                        ["PT5M", "PT1H", "P1D"]
                            .iter()
                            .map(|value| (*value).to_string())
                            .collect(),
                    ),
                    value: Some("P2D".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value 'P2D' at 'period' failed to satisfy \
                 constraint: Member must satisfy enum value set: [P1D, PT1H, PT5M]",
            ),
            (
                "an enum on a list element, stated at the list",
                Violation {
                    path: "channels.1.member.eventPublishers.1.member".to_string(),
                    kind: ViolationKind::EnumValue(vec!["AnomalyDetection".to_string()]),
                    value: Some("NotARealPublisher".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value '[NotARealPublisher]' at \
                 'channels.1.member.eventPublishers' failed to satisfy constraint: Member \
                 must satisfy constraint: [Member must satisfy enum value set: \
                 [AnomalyDetection]]",
            ),
        ];

        for (case, found, expected) in rows {
            let stated = refusal(std::slice::from_ref(&found), || {
                HashMap::from([(
                    "channels.1.member.eventPublishers".to_string(),
                    "[NotARealPublisher]".to_string(),
                )])
            })
            .unwrap_or_else(|| panic!("case {case:?}: expected a stated refusal"));
            assert_eq!(
                stated.code, VALIDATION_EXCEPTION,
                "case {case:?}: expected the code {VALIDATION_EXCEPTION}, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// Several elements of ONE list are one clause, and the clause states the WHOLE list —
    /// the valid elements included — because that is the value the list member holds.
    #[test]
    fn element_failures_of_one_list_are_one_clause_stating_the_whole_list() {
        // (case, the submitted list, the positions the walk reported a failure at)
        let rows: [(&str, &[&str], &[usize]); 4] = [
            ("one element, all of it bad", &["bad"], &[1]),
            ("two elements, the second bad", &["AnomalyDetection", "bad"], &[2]),
            ("two elements, both bad", &["bad", "worse"], &[1, 2]),
            (
                "three elements, the middle one bad",
                &["AnomalyDetection", "bad", "AnomalyDetection"],
                &[2],
            ),
        ];

        for (case, submitted, failed_at) in rows {
            // The list as `service_common::validation::submitted_lists` renders it: every
            // element the caller sent, the valid ones included.
            let lists = HashMap::from([(
                "eventPublishers".to_string(),
                format!("[{}]", submitted.join(", ")),
            )]);
            let violations: Vec<Violation> = failed_at
                .iter()
                .map(|position| Violation {
                    path: format!("eventPublishers.{position}.member"),
                    kind: ViolationKind::EnumValue(vec!["AnomalyDetection".to_string()]),
                    value: Some(submitted[position - 1].to_string()),
                    sensitive: false,
                })
                .collect();
            let expected = format!(
                "1 validation error detected: Value '[{}]' at 'eventPublishers' failed to \
                 satisfy constraint: Member must satisfy constraint: [Member must satisfy \
                 enum value set: [AnomalyDetection]]",
                submitted.join(", ")
            );

            let stated = refusal(&violations, || lists.clone())
                .unwrap_or_else(|| panic!("case {case:?}: expected a stated refusal"));
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// Two members of one request are two clauses, counted and joined AWS's way — and the
    /// envelope is the framework's own, which this compares against directly.
    #[test]
    fn the_envelope_counts_and_joins_clauses_the_way_the_framework_does() {
        // Clauses the framework can also render (no list element among them), so its
        // envelope is the reference for the count and the joiner.
        let found = [
            Violation {
                path: "profilingGroupName".to_string(),
                kind: ViolationKind::MinLength(1),
                value: Some(String::new()),
                sensitive: false,
            },
            Violation {
                path: "fleetInstanceId".to_string(),
                kind: ViolationKind::MaxLength(255),
                value: Some("x".repeat(256)),
                sensitive: false,
            },
        ];

        for count in 1..=found.len() {
            let slice = &found[..count];
            let reference = aws_smithy_schema_server::envelope(slice)
                .unwrap_or_else(|| panic!("case {count} clauses: the framework words these"));
            let stated = refusal(slice, HashMap::new)
                .unwrap_or_else(|| panic!("case {count} clauses: expected a stated refusal"));
            assert_eq!(
                stated.message, reference,
                "case {count} clauses: expected the framework's envelope {reference:?}, got \
                 {:?}",
                stated.message
            );
        }
    }

    /// Nothing to word is no refusal: a request that breaks no constraint is not answered
    /// here.
    #[test]
    fn no_violations_is_no_refusal() {
        let stated = refusal(&[], HashMap::new);
        assert!(
            stated.is_none(),
            "case no violations: expected None, got {stated:?}"
        );
    }

    /// A list the shared walk could not render — because its elements were not collected
    /// whole, or have no string rendering at all — leaves the clause with the framework's
    /// own echo of the element that failed, still stated at the LIST member.
    #[test]
    fn a_list_that_was_not_rendered_keeps_the_frameworks_own_echo() {
        let element = "x".repeat(1024);
        let stated = refusal(
            &[Violation {
                path: "eventPublishers.1.member".to_string(),
                kind: ViolationKind::EnumValue(vec!["AnomalyDetection".to_string()]),
                value: Some(element.clone()),
                sensitive: false,
            }],
            // `submitted_lists` offers nothing for this list.
            HashMap::new,
        )
        .expect("a stated refusal");
        let expected = format!(
            "1 validation error detected: Value '{element}' at 'eventPublishers' failed to \
             satisfy constraint: Member must satisfy constraint: [Member must satisfy enum \
             value set: [AnomalyDetection]]"
        );
        assert_eq!(
            stated.message, expected,
            "case a list with no rendering: expected the framework's own echo, got {:?}",
            stated.message
        );
    }
}

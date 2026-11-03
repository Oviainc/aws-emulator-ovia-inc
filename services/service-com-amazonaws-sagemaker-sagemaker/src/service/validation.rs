// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request that breaks a constraint.
//!
//! The framework finds the violations (the protocol's deserializer for an absent
//! required member, `constraints::validate` for a value outside a declared bound) and
//! words them in its own conventional envelope — `1 validation error detected: Value
//! '<value>' at '<member path>' failed to satisfy constraint: <requirement>`. SageMaker
//! words the same facts differently in four ways, each a property of the shape that was
//! violated rather than of one request, so each belongs here rather than in a handler:
//!
//! 1. **A `@pattern` is stated WITHOUT the `^`/`$` anchors.** Smithy's `@pattern` is an
//!    unanchored find, so a model writes `^…$` to demand a whole-value match; the
//!    requirement a caller is told about is the expression itself. `CreateTrialComponent`
//!    `DisplayName` (`smithy_query(query="shape",shape="ExperimentEntityName")`:
//!    `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`) is reported as
//!    `[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}`, and `S3Uri`
//!    (`smithy_query(query="shape",shape="S3Uri")`: `^(https|s3)://([^/]+)/?(.*)$`) as
//!    `(https|s3)://([^/]+)/?(.*)`. The pair is dropped from the RENDERING only — the
//!    check still runs the model's own expression ([`unanchored`]).
//! 2. **An enum value set is listed in the order the SERVICE declares the enum**, which
//!    is not always the order the public model lists (see [`MODEL_APPROVAL_STATUS`]).
//! 3. **A value outside the TRAINING instance types is told the CONDITION it failed**
//!    rather than handed that value set
//!    ([`SUPPORTED_INSTANCE_TYPE_REQUIREMENT`]).
//! 4. **A violation of a MAP member's values is stated against the map**, echoing the
//!    whole map and listing the value shape's complete constraint set
//!    ([`map_value_message`]); that one is reached from the handler that enforces such a
//!    constraint, because the framework's own walk reports the offending entry instead.
//!
//! Status (400) and error code ([`VALIDATION_EXCEPTION`]) are the framework's own choice
//! for a constraint failure and are left alone; only the constructed sentence changes.

use std::collections::BTreeMap;

use aws_smithy_schema_server::constraints::ViolationKind;
use service_common::validation::unanchored;
use smithy_server_runtime::{Refused, Violation};

/// The error code a constraint failure is named by, unchanged from the one the
/// framework's refusal channel picks for this `awsJson1_1` service: a chosen [`Refused`]
/// replaces the code and the message TOGETHER, so the code is restated here rather than
/// left to a default that a reworded message would otherwise silently drop.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text separating a clause's subject from the requirement it states, as the
/// framework's own [`Violation::message`] writes it.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// What a clause states when the value that broke a constraint is a MAP's value: the
/// whole map is the value, and the requirement is the value shape's whole constraint set
/// rather than the one bound that failed.
const MAP_VALUE_REQUIREMENT: &str = "Map value must satisfy constraint: ";

/// The one enum of this service whose value set the recordings show in an order the
/// public Smithy model does not list it in.
///
/// `smithy_query(query="shape",shape="ModelApprovalStatus")` declares
/// `[Approved, Rejected, PendingManualApproval]`; a recorded `UpdateModelPackage`
/// refusal of an invalid `ModelApprovalStatus` lists the set as below. The order is
/// therefore a property of the ENUM SHAPE — the order the service's own model declares
/// its values in — and not of any one request: it is the same set, listed as the service
/// lists it, for every operation that carries the member.
///
/// FRAGILITY, stated rather than hidden: a runtime [`Violation`] carries the value set
/// but not the shape it came from, so [`declared_order`] can only recognise this enum by
/// its MEMBERS. Two consequences follow, and both are asserted in this module's tests: a
/// different enum of this service declaring exactly these three values would be reordered
/// too (none does — the unit rows check the model's other enums pass through), and a
/// `ModelApprovalStatus` that gained or lost a member would stop being recognised, so
/// this constant has to be updated with the model. Key the reorder on the shape id
/// instead the moment the framework carries one on `Violation`.
const MODEL_APPROVAL_STATUS: [&str; 3] = ["PendingManualApproval", "Approved", "Rejected"];

/// What a member whose value must be one of the TRAINING instance types is told it failed:
/// the CONDITION the value did not meet, in place of the value set that would meet it.
///
/// A property of that value SET — not of one member, one operation or one request — so
/// every member targeting it states this, wherever it sits in a request:
/// `ResourceConfig$InstanceType` and its `InstanceGroups` entries, the tuning resource
/// configuration's instance configurations, and an algorithm's supported training instance
/// types. The listing of [`as_rendered`] stands for every OTHER enum of this service,
/// the hosting instance types (`ProductionVariantInstanceType`) among them, whose recorded
/// refusal does list its set.
///
/// The value set is recognised by its MEMBERS, from the generated
/// `TRAINING_INSTANCE_TYPE_VALUES` — the model's own values, so the recognition follows the
/// model rather than a list written out here — on the same terms, and with the same
/// fragility, as [`MODEL_APPROVAL_STATUS`]: a runtime [`Violation`] carries the set and not
/// the shape it came from.
const SUPPORTED_INSTANCE_TYPE_REQUIREMENT: &str = "Member must be a supported instance type";

/// This service's wording for the violations the framework found on one request, or
/// `None` when there are none to word.
pub(super) fn refusal(violations: &[Violation]) -> Option<Refused> {
    let clauses: Vec<String> = violations.iter().map(stated_clause).collect();
    let message = stated_message(&clauses)?;
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message,
    })
}

/// One violation as this service states it.
///
/// The whole sentence is the FRAMEWORK's — the echoed value, the member path, the frame
/// around them and the requirement itself — reached through the same [`Violation`] the
/// walk produced, so a constraint class this service does not reword cannot drift from the
/// framework's own clause. Where the service states the CONDITION a value failed instead
/// of the framework's rendering of the constraint ([`restated_requirement`]), only the
/// requirement half is replaced, so the clause's subject stays the framework's.
fn stated_clause(violation: &Violation) -> String {
    let clause = as_stated(violation).message();
    let Some(requirement) = restated_requirement(&violation.kind) else {
        return clause;
    };
    match clause.rsplit_once(CONSTRAINT_SEPARATOR) {
        Some((subject, _)) => format!("{subject}{CONSTRAINT_SEPARATOR}{requirement}"),
        None => clause,
    }
}

/// The requirement this service words ITSELF, for a constraint whose failure it states as
/// a condition rather than as the framework's rendering of the constraint — or `None`,
/// which is every other class, where the framework's requirement stands.
///
/// [`ViolationKind`] carries the constraint that was broken, so a sentence that is not a
/// rendering OF one cannot travel on it: it is chosen here, from the violated constraint,
/// and nowhere else.
fn restated_requirement(kind: &ViolationKind) -> Option<&'static str> {
    match kind {
        ViolationKind::EnumValue(values)
            if is_set_of(values, crate::ops::types::TRAINING_INSTANCE_TYPE_VALUES) =>
        {
            Some(SUPPORTED_INSTANCE_TYPE_REQUIREMENT)
        }
        _ => None,
    }
}

/// The `ValidationException` message around the clauses this service stated.
///
/// Spelled exactly as [`aws_smithy_schema_server::envelope`] spells it, and not reached
/// through it, because a clause this module states is not always one the framework can
/// build from a [`Violation`] (see [`restated_requirement`] and [`map_value_message`]).
fn stated_message(clauses: &[String]) -> Option<String> {
    match clauses {
        [] => None,
        [one] => Some(envelope_of(one)),
        many => Some(format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        )),
    }
}

/// One violation as this service states it: the framework's path and echoed value, with
/// the violated constraint rendered the way this service renders it.
fn as_stated(violation: &Violation) -> Violation {
    Violation {
        path: violation.path.clone(),
        kind: as_rendered(&violation.kind),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// A violated constraint as this service renders it: a `@pattern` without its
/// whole-value anchors, an enum value set in the service's declared order, and every
/// other class exactly as the framework renders it.
fn as_rendered(kind: &ViolationKind) -> ViolationKind {
    match kind {
        ViolationKind::Pattern(expression) => ViolationKind::Pattern(unanchored(expression)),
        ViolationKind::EnumValue(values) => ViolationKind::EnumValue(declared_order(values)),
        other => other.clone(),
    }
}

/// The requirement half of a clause: what the value had to satisfy, with none of the
/// clause's subject.
///
/// Read off the FRAMEWORK's own rendering of `kind` — with an empty path and no value, so
/// every word of the sentence is the framework's or the model's — and then cut at the
/// clause separator, at its LAST occurrence: the requirement is the tail, and cutting
/// there cannot be fooled by a separator-shaped run inside the rendering. A constraint
/// class the framework learns to check therefore cannot drift from this wording, and
/// [`ViolationKind`] being `#[non_exhaustive]` costs nothing here.
///
/// MIGRATION NOTE: `services/common/src/validation.rs` reserves this "requirement half"
/// rule for the shared module (its header lists it among the parts still to land there).
/// It has no shared helper yet, so this is the local reading until that lands — and is to
/// be deleted in favour of it rather than copied into a third service.
fn requirement_text(kind: &ViolationKind) -> String {
    if let Some(requirement) = restated_requirement(kind) {
        return requirement.to_string();
    }
    let rendered = Violation {
        path: String::new(),
        kind: as_rendered(kind),
        value: None,
        sensitive: false,
    }
    .message();
    match rendered.rsplit_once(CONSTRAINT_SEPARATOR) {
        Some((_, requirement)) => requirement.to_string(),
        None => rendered,
    }
}

/// The largest number of map ENTRIES a clause renders, and the largest number of bytes
/// the rendered map may carry.
///
/// The clause echoes a value the CALLER chose, so its size has to be bounded here rather
/// than inherited from the member's modelled bounds — `ArtifactProperties` admits 30
/// entries of up to 4096 bytes each, which is a ~120 KB response built out of one
/// request. Both bounds are this module's own choice, in the spirit of the framework's
/// own echo cap (`MAX_ECHOED_VALUE`, 256 bytes per clause) and of
/// `services/common/src/validation.rs`'s restored-echo ceilings: generous enough that
/// every refusal a modelled map produces today renders in full (30 entries of a name and
/// a short value), small enough that one refusal cannot carry request-sized content.
/// Past either bound the rendering is cut and the elision is MARKED, so a reader can tell
/// a cut map from a short one.
const MAX_RENDERED_ENTRIES: usize = 30;

/// See [`MAX_RENDERED_ENTRIES`].
const MAX_RENDERED_BYTES: usize = 4096;

/// What a cut rendering ends with, in place of the entries it does not carry — after the
/// entries it does carry ([`ELISION_AFTER_ENTRIES`]), or on its own when not even the
/// first entry fits ([`ELISION_ALONE`]).
const ELISION_AFTER_ENTRIES: &str = ", ...";

/// See [`ELISION_AFTER_ENTRIES`].
const ELISION_ALONE: &str = "...";

/// The whole `ValidationException` message for a MAP member whose VALUES must satisfy
/// `requirements`: the map itself is the value the clause echoes, `path` is the map
/// member, and the requirement is the value shape's COMPLETE constraint set.
///
/// SageMaker reports a map-value constraint against the map, not against the entry that
/// broke it: a caller is told which map it sent and what every value in it has to
/// satisfy, so the whole map is rendered (`{key=value, …}`) and `requirements` is listed
/// in full. Entries are listed in key order, so one request renders one way however the
/// map arrived, and the rendering is bounded by [`MAX_RENDERED_ENTRIES`] and
/// [`MAX_RENDERED_BYTES`] so a refusal is never sized by the request.
pub(crate) fn map_value_message<'entries>(
    path: &str,
    entries: impl IntoIterator<Item = (&'entries str, &'entries str)>,
    requirements: &[ViolationKind],
) -> String {
    let sorted: BTreeMap<&str, &str> = entries.into_iter().collect();
    let mut rendered = String::new();
    let mut cut = sorted.len() > MAX_RENDERED_ENTRIES;
    for (key, value) in sorted.iter().take(MAX_RENDERED_ENTRIES) {
        let entry = format!("{key}={value}");
        let separator = usize::from(!rendered.is_empty()) * 2;
        if rendered.len() + separator + entry.len() > MAX_RENDERED_BYTES {
            cut = true;
            break;
        }
        if separator != 0 {
            rendered.push_str(", ");
        }
        rendered.push_str(&entry);
    }
    if cut {
        rendered.push_str(if rendered.is_empty() {
            ELISION_ALONE
        } else {
            ELISION_AFTER_ENTRIES
        });
    }
    let stated: Vec<String> = requirements.iter().map(requirement_text).collect();
    envelope_of(&format!(
        "Value '{{{rendered}}}' at '{path}'{CONSTRAINT_SEPARATOR}{MAP_VALUE_REQUIREMENT}[{}]",
        stated.join(", ")
    ))
}

/// The validation path of the `Properties` member, as the framework's own path spelling
/// (lowerCamel) writes it — the map a property-value refusal is stated against.
const PROPERTIES_PATH: &str = "properties";

/// The lower bound a value of the `ArtifactProperties` map must satisfy. The shape its
/// values target (`smithy_query(query="shape",shape="ArtifactPropertyValue")`) states
/// `@length 0..4096`, so an empty value is inside the modelled bound and outside the one
/// the service enforces; this is that enforced minimum.
const PROPERTY_VALUE_MIN_LENGTH: i64 = 1;

/// The whole constraint set a value of the `ArtifactProperties` map must satisfy, in the
/// order the service lists it in a refusal: the modelled maximum, the enforced minimum,
/// then the modelled expression. Source for the two modelled entries:
/// `smithy_query(query="shape",shape="ArtifactPropertyValue")` — `@length 0..4096`,
/// `@pattern .*`.
fn property_value_constraints() -> [ViolationKind; 3] {
    [
        ViolationKind::MaxLength(4096),
        ViolationKind::MinLength(PROPERTY_VALUE_MIN_LENGTH),
        ViolationKind::Pattern(".*".to_string()),
    ]
}

/// How this service states an `ArtifactProperties` map a request may not send, or `None`
/// when every value in it is acceptable.
///
/// The enforced minimum is a property of the VALUE SHAPE, not of one operation, so every
/// operation carrying a map of `ArtifactPropertyValue`s — `CreateArtifact` and
/// `UpdateArtifact` — refuses the same map here rather than wording it twice. The clause
/// is stated against the MAP ([`map_value_message`]), echoing every entry of it and
/// listing the whole constraint set a property value has to satisfy.
pub(crate) fn artifact_properties_refusal(
    properties: &std::collections::HashMap<String, String>,
) -> Option<String> {
    properties.values().any(String::is_empty).then(|| {
        map_value_message(
            PROPERTIES_PATH,
            properties
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
            &property_value_constraints(),
        )
    })
}

/// The `ValidationException` envelope around one clause this service stated itself.
///
/// Spelled exactly as [`aws_smithy_schema_server::envelope`] spells a single-violation
/// message, so a stated clause is framed the way a walked one is; it cannot go
/// through that function because the clause this module states is not always one the
/// framework can build from a [`Violation`] (a map-value clause reports the map, not the
/// entry the walk found, and a restated requirement is no rendering of a
/// [`ViolationKind`]).
fn envelope_of(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// `values` in the order the service declares the enum they came from.
///
/// (The unanchored `@pattern` rendering this module used to carry three local helpers for
/// is GONE: that rule is AWS's rather than this service's, and
/// [`service_common::validation::unanchored`] is its one home.)
///
/// A value set is identified by the values it holds (a runtime [`Violation`] carries the
/// set, not the shape it came from), so the one enum this service declares in an order
/// the public model does not list is recognised by its own value set. Every other value
/// set is already in its declared order and is passed through.
fn declared_order(values: &[String]) -> Vec<String> {
    if is_set_of(values, &MODEL_APPROVAL_STATUS) {
        return MODEL_APPROVAL_STATUS
            .iter()
            .map(|value| (*value).to_string())
            .collect();
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
    //! The renderings this module owns, each over a SET of inputs rather than one
    //! recorded case. The end-to-end wording is pinned on the wire in
    //! `tests/all/rejection_fidelity.rs`.

    use super::*;

    /// A `@pattern` loses the whole-value anchor PAIR and nothing else.
    #[test]
    fn a_pattern_is_rendered_without_its_whole_value_anchors() {
        // (case, modelled expression, rendered expression)
        let rows: [(&str, &str, &str); 12] = [
            // Source: `smithy_query(query="shape",shape="ExperimentEntityName")`, the
            // target of `CreateTrialComponent$DisplayName`.
            (
                "the modelled experiment entity name",
                r"^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$",
                r"[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}",
            ),
            // Source: `smithy_query(query="shape",shape="S3Uri")`.
            (
                "the modelled S3 URI",
                r"^(https|s3)://([^/]+)/?(.*)$",
                r"(https|s3)://([^/]+)/?(.*)",
            ),
            // Source: `smithy_query(query="shape",shape="ImageUri")`, the target of
            // `ContainerDefinition$Image`.
            ("a one-class expression", r"^[\S]+$", r"[\S]+"),
            ("anchored at the start only", "^arn:aws:", "^arn:aws:"),
            ("anchored at the end only", "[0-9]+$", "[0-9]+$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            ("an escaped trailing dollar", r"^[a-z]+\$$", r"[a-z]+\$"),
            // The pair is dropped POSITIONALLY — see
            // `service_common::validation::unanchored`, which this module reads the rule
            // from: the published expression is the service's own with that pair written
            // around it, so an alternation does not keep it (a lambdacore recording shows
            // AWS dropping it from a top-level alternation).
            ("a fully anchored alternation", "^a$|^b$", "a$|^b"),
            ("a half-anchored alternation", "^a|b$", "a|b"),
            ("an alternation inside a group", "^(a|b)$", "(a|b)"),
            ("a negated character class", r"^[^/]+$", r"[^/]+"),
            ("a class holding a literal dollar", "^[a-z$]+$", "[a-z$]+"),
        ];

        for (case, modelled, expected) in rows {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?} ({modelled}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A value set is listed in the service's declared order: the recorded order for
    /// `ModelApprovalStatus`, the model's own order for every other enum.
    ///
    /// The second half of the test pins the ASSUMPTION the set-membership recognition
    /// rests on (see [`MODEL_APPROVAL_STATUS`]): no other enum of this service's model
    /// declares exactly those three values, so no other value set is reordered.
    #[test]
    fn an_enum_value_set_is_listed_in_the_services_declared_order() {
        // (case, the set as the model declares it, the set as the service lists it)
        let rows: [(&str, &[&str], &[&str]); 7] = [
            (
                "ModelApprovalStatus, in the model's declaration order",
                &["Approved", "Rejected", "PendingManualApproval"],
                &MODEL_APPROVAL_STATUS,
            ),
            (
                "ModelApprovalStatus, already in the service's order",
                &MODEL_APPROVAL_STATUS,
                &MODEL_APPROVAL_STATUS,
            ),
            // Source: `smithy_query(query="shape",shape="SkipModelValidation")`.
            ("SkipModelValidation", &["All", "None"], &["All", "None"]),
            // Source: `smithy_query(query="shape",shape="PipelineStatus")`.
            ("PipelineStatus", &["Active", "Deleting"], &["Active", "Deleting"]),
            // Source: `smithy_query(query="shape",shape="ModelPackageStatus")` — three
            // of its five members, so a three-value set of this service is not enough to
            // be mistaken for `ModelApprovalStatus`.
            (
                "a three-value set of another enum",
                &["Completed", "Failed", "Pending"],
                &["Completed", "Failed", "Pending"],
            ),
            // A set that merely overlaps `ModelApprovalStatus` keeps its own order.
            (
                "a value set holding some of ModelApprovalStatus's values",
                &["Approved", "Rejected"],
                &["Approved", "Rejected"],
            ),
            // One more value than the recognised set: the rule stops applying, which is
            // the signal `MODEL_APPROVAL_STATUS` has to be updated with the model.
            (
                "the recognised set plus a member",
                &["Approved", "Rejected", "PendingManualApproval", "Withdrawn"],
                &["Approved", "Rejected", "PendingManualApproval", "Withdrawn"],
            ),
        ];

        for (case, declared, expected) in rows {
            let given: Vec<String> = declared.iter().map(|value| (*value).to_string()).collect();
            let actual = declared_order(&given);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A value outside the TRAINING instance types is told the condition it failed, and no
    /// other value set is: the generic listing stands for the rest, the hosting instance
    /// types included.
    #[test]
    fn only_the_training_instance_types_are_stated_as_a_condition() {
        // (case, the value set the walk reported, the requirement this service states)
        let listed = |values: &[&str]| {
            format!(
                "Member must satisfy enum value set: [{}]",
                values.join(", ")
            )
        };
        let training: Vec<&str> = crate::ops::types::TRAINING_INSTANCE_TYPE_VALUES.to_vec();
        // Source: `smithy_query(query="shape",shape="ProductionVariantInstanceType")` and
        // the other value sets of this model, through their generated constants.
        let hosting: Vec<&str> = crate::ops::types::PRODUCTION_VARIANT_INSTANCE_TYPE_VALUES.to_vec();
        let transform: Vec<&str> = crate::ops::types::TRANSFORM_INSTANCE_TYPE_VALUES.to_vec();
        let processing: Vec<&str> = crate::ops::types::PROCESSING_INSTANCE_TYPE_VALUES.to_vec();
        // The recognition is by MEMBERS, so a set that merely overlaps the training one —
        // here every training type but the first — is not it.
        let short_of_the_set: Vec<&str> = training.iter().skip(1).copied().collect();

        let rows: [(&str, &[&str], String); 6] = [
            (
                "the training instance types",
                &training,
                SUPPORTED_INSTANCE_TYPE_REQUIREMENT.to_string(),
            ),
            ("the hosting instance types", &hosting, listed(&hosting)),
            (
                "the transform instance types",
                &transform,
                listed(&transform),
            ),
            (
                "the processing instance types",
                &processing,
                listed(&processing),
            ),
            (
                "one short of the training set",
                &short_of_the_set,
                listed(&short_of_the_set),
            ),
            (
                "a two-value set of another enum",
                &["All", "None"],
                listed(&["All", "None"]),
            ),
        ];

        for (case, values, expected) in rows {
            let found = Violation {
                path: "resourceConfig.instanceType".to_string(),
                kind: ViolationKind::EnumValue(
                    values.iter().map(|value| (*value).to_string()).collect(),
                ),
                value: Some("ml.totally.bogus".to_string()),
                sensitive: false,
            };
            let stated = refusal(std::slice::from_ref(&found))
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            let whole = format!(
                "1 validation error detected: Value 'ml.totally.bogus' at \
                 'resourceConfig.instanceType'{CONSTRAINT_SEPARATOR}{expected}"
            );
            assert_eq!(
                stated.message, whole,
                "case {case:?}: expected {whole:?}, got {:?}",
                stated.message
            );
        }
    }

    /// The requirement is replaced for EVERY member carrying the set, whatever the clause's
    /// subject, and the envelope still counts and joins the clauses the framework's way.
    #[test]
    fn a_restated_requirement_leaves_the_clauses_subject_and_the_envelope_alone() {
        let instance_type = |path: &str, value: &str| Violation {
            path: path.to_string(),
            kind: ViolationKind::EnumValue(
                crate::ops::types::TRAINING_INSTANCE_TYPE_VALUES
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
            ),
            value: Some(value.to_string()),
            sensitive: false,
        };

        // Rows differ along the member's depth in the request, the echoed value's shape
        // (including one that spells the clause separator itself) and the clause count.
        let rows: [(&str, Vec<Violation>, String); 4] = [
            (
                "a top-level resource configuration",
                vec![instance_type("resourceConfig.instanceType", "ml.t2.medium")],
                format!(
                    "1 validation error detected: Value 'ml.t2.medium' at \
                     'resourceConfig.instanceType'{CONSTRAINT_SEPARATOR}\
                     {SUPPORTED_INSTANCE_TYPE_REQUIREMENT}"
                ),
            ),
            (
                "an instance group inside a list",
                vec![instance_type(
                    "resourceConfig.instanceGroups.1.member.instanceType",
                    "",
                )],
                format!(
                    "1 validation error detected: Value '' at \
                     'resourceConfig.instanceGroups.1.member.instanceType'\
                     {CONSTRAINT_SEPARATOR}{SUPPORTED_INSTANCE_TYPE_REQUIREMENT}"
                ),
            ),
            (
                "a value spelling the clause separator",
                vec![instance_type(
                    "resourceConfig.instanceType",
                    "x failed to satisfy constraint: y",
                )],
                format!(
                    "1 validation error detected: Value 'x failed to satisfy constraint: y' \
                     at 'resourceConfig.instanceType'{CONSTRAINT_SEPARATOR}\
                     {SUPPORTED_INSTANCE_TYPE_REQUIREMENT}"
                ),
            ),
            (
                "two members of one request",
                vec![
                    instance_type("resourceConfig.instanceType", "ml.bogus.one"),
                    instance_type(
                        "resourceConfig.instanceGroups.1.member.instanceType",
                        "ml.bogus.two",
                    ),
                ],
                format!(
                    "2 validation errors detected: Value 'ml.bogus.one' at \
                     'resourceConfig.instanceType'{CONSTRAINT_SEPARATOR}\
                     {SUPPORTED_INSTANCE_TYPE_REQUIREMENT}; Value 'ml.bogus.two' at \
                     'resourceConfig.instanceGroups.1.member.instanceType'\
                     {CONSTRAINT_SEPARATOR}{SUPPORTED_INSTANCE_TYPE_REQUIREMENT}"
                ),
            ),
        ];

        for (case, found, expected) in rows {
            let stated = refusal(&found)
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// Every constraint class the framework can report keeps the framework's own clause,
    /// and only the two renderings above change.
    #[test]
    fn every_constraint_class_keeps_the_frameworks_clause() {
        // (case, the violation the framework found, the message this service states)
        let rows: [(&str, Violation, &str); 4] = [
            (
                "an enum value set",
                Violation {
                    path: "modelApprovalStatus".to_string(),
                    kind: ViolationKind::EnumValue(
                        ["Approved", "Rejected", "PendingManualApproval"]
                            .iter()
                            .map(|value| (*value).to_string())
                            .collect(),
                    ),
                    value: Some("Maybe".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value 'Maybe' at 'modelApprovalStatus' \
                 failed to satisfy constraint: Member must satisfy enum value set: \
                 [PendingManualApproval, Approved, Rejected]",
            ),
            (
                "a pattern",
                Violation {
                    path: "transformInput.dataSource.s3DataSource.s3Uri".to_string(),
                    kind: ViolationKind::Pattern(r"^(https|s3)://([^/]+)/?(.*)$".to_string()),
                    value: Some("not-a-valid-s3-uri".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value 'not-a-valid-s3-uri' at \
                 'transformInput.dataSource.s3DataSource.s3Uri' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 (https|s3)://([^/]+)/?(.*)",
            ),
            (
                "an absent required member",
                Violation {
                    path: "modelPackageArn".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                "1 validation error detected: Value null at 'modelPackageArn' failed to \
                 satisfy constraint: Member must not be null",
            ),
            (
                "a length bound",
                Violation {
                    path: "displayName".to_string(),
                    kind: ViolationKind::MaxLength(120),
                    value: Some("x".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value 'x' at 'displayName' failed to \
                 satisfy constraint: Member must have length less than or equal to 120",
            ),
        ];

        for (case, found, expected) in rows {
            let stated = refusal(std::slice::from_ref(&found))
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
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

    /// Nothing to word is no refusal: a request that breaks no constraint is not answered
    /// here.
    #[test]
    fn no_violations_is_no_refusal() {
        let stated = refusal(&[]);
        assert!(
            stated.is_none(),
            "case no violations: expected None, got {stated:?}"
        );
    }

    /// A map-value clause echoes the WHOLE map, in key order, and lists every
    /// requirement it was given.
    #[test]
    fn a_map_value_clause_echoes_the_whole_map_and_every_requirement() {
        let requirements = [
            ViolationKind::MaxLength(4096),
            ViolationKind::MinLength(1),
            ViolationKind::Pattern(".*".to_string()),
        ];
        // Rows differ along entry count, the order the entries arrive in, and which of
        // the values is the empty one.
        type Row = (
            &'static str,
            &'static [(&'static str, &'static str)],
            &'static str,
        );
        let rows: [Row; 5] = [
            ("one entry", &[("only", "")], "{only=}"),
            (
                "two entries, the empty one first",
                &[("empty_prop", ""), ("present_prop", "some-value")],
                "{empty_prop=, present_prop=some-value}",
            ),
            (
                "the same two entries, sent in the other order",
                &[("present_prop", "some-value"), ("empty_prop", "")],
                "{empty_prop=, present_prop=some-value}",
            ),
            (
                "three entries",
                &[("a", "1"), ("c", ""), ("b", "2")],
                "{a=1, b=2, c=}",
            ),
            ("no entries at all", &[], "{}"),
        ];

        for (case, entries, rendered) in rows {
            let expected = format!(
                "1 validation error detected: Value '{rendered}' at 'properties' failed \
                 to satisfy constraint: Map value must satisfy constraint: [Member must \
                 have length less than or equal to 4096, Member must have length greater \
                 than or equal to 1, Member must satisfy regular expression pattern: .*]"
            );
            let actual = map_value_message("properties", entries.iter().copied(), &requirements);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The rendered map is BOUNDED: a map at either cap renders in full, and one past a
    /// cap is cut with the elision marked, so a refusal is never sized by the request.
    #[test]
    fn a_map_value_clause_is_bounded_by_its_own_caps() {
        let requirements = [ViolationKind::MinLength(1)];
        // Keys and values sized so the byte cap is reached by entry COUNT in one row and
        // by entry SIZE in another: "k<nn>=<v…>" entries of a known width.
        let entry_text = |index: usize, value_len: usize| {
            (format!("k{index:03}"), "v".repeat(value_len))
        };

        // (case, entries, whether the rendering must be cut)
        let at_the_entry_cap: Vec<(String, String)> =
            (0..MAX_RENDERED_ENTRIES).map(|i| entry_text(i, 1)).collect();
        let past_the_entry_cap: Vec<(String, String)> = (0..MAX_RENDERED_ENTRIES + 1)
            .map(|i| entry_text(i, 1))
            .collect();
        // Two entries whose renderings together exceed the byte cap, the first alone
        // fitting: the rendering carries the first and marks the elision after it.
        let past_the_byte_cap: Vec<(String, String)> = (0..2)
            .map(|i| entry_text(i, MAX_RENDERED_BYTES / 2))
            .collect();
        // One entry that alone exceeds the byte cap: nothing fits, so the rendering is
        // the elision on its own rather than a leading separator.
        let first_entry_past_the_cap: Vec<(String, String)> =
            vec![entry_text(0, MAX_RENDERED_BYTES + 1)];

        type BoundRow<'r> = (&'r str, &'r Vec<(String, String)>, bool, &'r str);
        let rows: [BoundRow<'_>; 4] = [
            ("at the entry cap", &at_the_entry_cap, false, ""),
            (
                "one entry past the cap",
                &past_the_entry_cap,
                true,
                ELISION_AFTER_ENTRIES,
            ),
            (
                "past the byte cap",
                &past_the_byte_cap,
                true,
                ELISION_AFTER_ENTRIES,
            ),
            (
                "the first entry alone past the byte cap",
                &first_entry_past_the_cap,
                true,
                ELISION_ALONE,
            ),
        ];

        for (case, entries, must_be_cut, marker) in rows {
            let borrowed: Vec<(&str, &str)> = entries
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            let actual = map_value_message("properties", borrowed, &requirements);
            assert_eq!(
                actual.contains(ELISION_ALONE),
                must_be_cut,
                "case {case:?}: expected cut={must_be_cut}, got {actual:?}"
            );
            if must_be_cut {
                assert!(
                    actual.contains(&format!("{marker}}}")),
                    "case {case:?}: expected the rendering to end in {marker:?}, \
                     got {actual:?}"
                );
            }
            // Whatever the request, the clause stays within the byte cap plus the frame
            // this module writes round it.
            assert!(
                actual.len() < MAX_RENDERED_BYTES * 2,
                "case {case:?}: the clause must stay bounded, got {} bytes",
                actual.len()
            );
        }
    }
}

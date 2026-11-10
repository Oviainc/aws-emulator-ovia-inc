// SPDX-License-Identifier: CC0-1.0

//! The WORDS of the refusal the framework's boundary walk produces for this service:
//! how a set of constraint violations becomes the `message` a 400
//! `ValidationException` carries.
//!
//! [`crate::service`] owns the SEAM — which requests are checked, and how the
//! violations and the request's own values are collected — and this module owns the
//! sentence, so the clause rules live in one place instead of in each operation that
//! trips them. What is ENFORCED is always the model's: every violation handed to
//! [`refusal`] comes from the framework's own walk, and nothing here accepts or
//! refuses anything.
//!
//! The rules, each holding for every member of every operation rather than for the
//! members one report happened to cite:
//!
//! 1. the envelope is the AWS one, `<N> validation error(s) detected: ` over one clause
//!    per failing (member, requirement), joined with `; ` ([`envelope`]);
//! 2. a clause states the value as SUBMITTED, the member's dotted path and the
//!    requirement it failed, in the framework's own words ([`framework_requirement`]);
//! 3. a reported `@pattern` is the expression without the `^`/`$` anchors a model writes
//!    to demand a whole-value match (`service_common::validation::unanchored`);
//! 4. a violation on the ELEMENTS of a list is stated ONCE, at the LIST member, with the
//!    value that member holds — the whole list the caller sent — and with the
//!    requirements nested as `Member must satisfy constraint: [ … ]`: EVERY constraint
//!    the element shape declares, not only the one this request broke, read off the same
//!    model-derived schemas the walk checks against ([`element_constraints`]);
//! 5. a clause about a COLLECTION's own `@length` states no value, because a list and a
//!    map have no scalar rendering to quote (`service_common::validation_message`'s
//!    readers of the framework's count rendering);
//! 6. an enumerated type's value SET is listed in the order the SERVICE states that enum
//!    in, which is not the order the model declares its members in
//!    ([`crate::state::stated_enum_value_set`]); applied inside
//!    [`framework_requirement`], so one enum reads alike whether its clause is stated at
//!    the member itself or nested inside one about the list that holds it;
//! 7. a bound the SERVICE enforces for itself — one the model does not declare, so the
//!    constraint walk never sees it — is stated in the same envelope and the same
//!    requirement sentence, and quotes NO value ([`above_ceiling`]);
//! 8. a MODELLED constraint a handler re-checks for itself, so its refusal stays this
//!    operation's 400 whatever the boundary does with the value, is stated through the
//!    very clause writer the walk's own refusal goes through
//!    ([`outside_enum`], [`outside_element_enum`]) — so no second rendering of a value
//!    set, of a member path or of a requirement exists in this crate to drift from the
//!    one above.
//!
//! Rules 1-3 and 5 are `services/common`'s already
//! (`service_common::validation_message::aws_validation_refusal`), which cannot be
//! CALLED here: it renders every clause from a [`Violation`], and rule 4's nested
//! requirement is not a [`ViolationKind`] any violation can carry. Rule 4 is the one
//! this module exists for, and the parts it does not own — the path spelling, the list
//! rendering, the `@pattern` rendering, the requirement sentences and the readers of a
//! collection's count rendering — are all taken from the shared module and the
//! framework rather than restated, so this cannot drift from them.
//!
//! The status (400) and the error code ([`VALIDATION_EXCEPTION`]) are what the framework
//! already answers a constraint failure with on this awsJson service and are restated,
//! not changed: a chosen [`Refused`] replaces the code and the message together.

use std::collections::{HashMap, HashSet};

use ::aws_smithy_schema::{Schema, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{
    path, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait, Violation,
    ViolationKind,
};
use ::service_common::validation::{list_of_element, quoted_echo, unanchored};
use ::service_common::validation_message::{list_item_count, map_entry_count};
use ::smithy_server_runtime::Refused;

/// The error code a constraint failure is named by, unchanged from the one the
/// framework's refusal channel picks for this service. Restated here because a chosen
/// [`Refused`] carries both halves: a message without its matching code is worse than
/// leaving both alone.
pub(super) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text that separates a clause's subject from its requirement, in the framework's
/// own wording ([`Violation::message`]). Split on, never written: see
/// [`framework_requirement`].
const CLAUSE_MARKER: &str = "failed to satisfy constraint: ";

/// How a clause nests the requirements of a LIST's elements (rule 4).
const NESTED_REQUIREMENT: &str = "Member must satisfy constraint";

/// This service's wording for the violations the framework found, or `None` when there
/// is nothing to word (no violations) or a violation class this module has no wording
/// for ([`ViolationKind`] is `#[non_exhaustive]`), which keeps the framework's own.
///
/// `lists` yields every list of scalar elements the request carried, keyed by the member
/// path the walk reports that list at and already rendered
/// (`service_common::validation::submitted_lists`). It is a closure because it is only
/// needed for rule 4 — a clause stated at a list member states the whole list — so a
/// refusal that has no element violation never walks the request again.
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
/// (member, requirement), in the order the walk found them.
///
/// A violation found on a list element is stated at the LIST member with the element
/// requirements nested, which is why two bad elements of one list are ONE clause rather
/// than two, while a member that broke two different rules still states both.
fn message(violations: &[Violation], lists: &HashMap<String, String>) -> Option<String> {
    let mut stated: HashSet<(String, String)> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let element = list_of_element(&violation.path);
        let path = element.unwrap_or(violation.path.as_str()).to_string();
        let requirement = match element {
            // Every requirement the element shape declares, so the caller reads the
            // whole rule its value has to satisfy rather than the part of it this
            // request happened to break. A member whose element constraints cannot be
            // resolved from the schemas states the one that WAS broken, nested the same
            // way, rather than nothing.
            Some(_) => {
                let declared = match element_constraints(&path) {
                    Some(declared) => declared,
                    None => vec![framework_requirement(&violation.kind)?],
                };
                format!("{NESTED_REQUIREMENT}: [{}]", declared.join(", "))
            }
            None => framework_requirement(&violation.kind)?,
        };
        if !stated.insert((path.clone(), requirement.clone())) {
            continue;
        }
        let subject = subject(violation, element.and_then(|list| lists.get(list)));
        clauses.push(format!("{subject} at '{path}' {CLAUSE_MARKER}{requirement}"));
    }
    envelope(&clauses)
}

/// The envelope over the clauses of one refusal: `1 validation error detected: <clause>`
/// for one, `<N> validation errors detected: <clause>; <clause>` for several.
///
/// The wording is `aws_smithy_schema_server::envelope`'s, which cannot be CALLED here
/// (see the module header), so the two are pinned identical against that function itself
/// in [`tests::the_envelope_counts_and_joins_the_way_the_framework_does`].
fn envelope(clauses: &[String]) -> Option<String> {
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses.join("; "))),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The value half of a clause.
///
/// * an ABSENT member has no value at all and is stated as the null it was;
/// * a clause stated at a LIST member because one of its ELEMENTS failed states the
///   list the caller sent, when `service_common::validation::submitted_lists` could
///   render that list in full;
/// * a clause about a COLLECTION's own `@length` states no value: the only rendering the
///   walk has for a list or a map is its element count, which is not a value a caller
///   sent (rule 5);
/// * everything else echoes the value carried on the violation, which the seam has
///   already restored to the bytes the caller submitted.
fn subject(violation: &Violation, rendered_list: Option<&String>) -> String {
    match (&violation.value, rendered_list) {
        (None, _) => "Value null".to_string(),
        (Some(_), Some(rendered)) => format!("Value '{rendered}'"),
        (Some(_), None) if states_a_count(violation) => "Value".to_string(),
        (Some(shown), None) => format!("Value '{shown}'"),
    }
}

/// Whether the value stored on `violation` is the framework's COUNT rendering of a
/// collection rather than something the caller sent — a list's `<n> items` or a map's
/// `<n> entries` on a clause about that collection's own `@length`.
///
/// Read through the shared module's readers of that rendering, which are public for
/// exactly this: two services must not disagree about what the walk wrote.
fn states_a_count(violation: &Violation) -> bool {
    if !matches!(
        violation.kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    ) {
        return false;
    }
    violation.value.as_deref().is_some_and(|stored| {
        list_item_count(stored).is_some() || map_entry_count(stored).is_some()
    })
}

/// The framework's own sentence for `kind`, with the `@pattern` rendering of rule 3
/// applied.
///
/// Recovered from a violation with NO value and an EMPTY path, so that neither a
/// rejected value nor a caller-chosen path segment can reach the requirement half, and
/// read off [`Violation::message`] rather than re-spelled here, so a constraint class
/// the framework words differently (or one it gains) cannot drift out of step with it.
/// `None` when the marker is absent, which leaves the framework's whole message
/// standing.
fn framework_requirement(kind: &ViolationKind) -> Option<String> {
    let reported = match kind {
        ViolationKind::Pattern(pattern) => ViolationKind::Pattern(unanchored(pattern)),
        // Rule 6. Applied here, where every requirement sentence is built, so a value set
        // reads the same way whether the clause is stated at the enumerated member itself
        // or nested inside one about the list whose elements target that enum.
        ViolationKind::EnumValue(values) => {
            ViolationKind::EnumValue(crate::state::stated_enum_value_set(values))
        }
        other => other.clone(),
    };
    let stated = Violation {
        path: String::new(),
        kind: reported,
        value: None,
        sensitive: false,
    }
    .message();
    stated
        .rsplit_once(CLAUSE_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// This service's refusal of a value above a ceiling the SERVICE enforces on the member
/// at `path`, as rule 7 states it: the envelope of rule 1 over one clause naming the
/// member and the requirement, and NO value.
///
/// A bound the model declares is found by the constraint walk, which carries the bytes
/// the caller submitted, so its clause quotes them (rule 2). A bound the service enforces
/// on top of the model's is checked after that walk has already accepted the value, and
/// the clause the live service answers it with quotes nothing — which is also the one
/// thing that tells the two bounds apart in the response. The requirement itself is the
/// framework's own sentence for the class, read off [`ViolationKind`] exactly as every
/// other requirement here is, so a service-enforced bound cannot be worded differently
/// from a modelled one.
pub(crate) fn above_ceiling(path: &str, ceiling: i32) -> String {
    let kind = ViolationKind::MaxValue(ceiling.to_string());
    // A class the framework words without the clause marker keeps its whole message,
    // which still states the bound — the same fallback [`framework_requirement`]'s callers
    // make, written out because there is no framework wording to fall back TO here.
    let requirement = framework_requirement(&kind).unwrap_or_else(|| {
        Violation {
            path: String::new(),
            kind,
            value: None,
            sensitive: false,
        }
        .message()
    });
    let clause = format!("Value at '{path}' {CLAUSE_MARKER}{requirement}");
    envelope(std::slice::from_ref(&clause)).unwrap_or(clause)
}

/// The whole `message` of this service's refusal of a value outside the enumerated type
/// the member at `member` targets, as rule 8 states it: the clause this module writes for
/// exactly that violation, which is the one the boundary walk's refusal carries.
///
/// `value` is the value as the caller sent it and `declared` the values the model
/// declares for the enum, handed over in the order the model declares them in — rule 6
/// restates the set in the order this SERVICE lists it, here as everywhere else, because
/// that order is a property of the enum and not of the layer that found the violation.
pub(crate) fn outside_enum(member: &str, value: &str, declared: &[&str]) -> String {
    let violation = enum_violation(member.to_string(), value, declared);
    one_clause(&violation, &HashMap::new())
}

/// The same refusal for an ELEMENT of a list whose element shape targets the enum, which
/// rule 4 states at the LIST member: `member` is that list member, `position` the
/// element's 1-based position (the spelling the walk's own paths use), and `rendered` the
/// whole list as the caller sent it (`service_common::validation::rendered_list`, the
/// rendering a clause restored from the walk quotes).
pub(crate) fn outside_element_enum(
    member: &str,
    position: usize,
    value: &str,
    declared: &[&str],
    rendered: &str,
) -> String {
    let element = path::join(&[member.to_string(), path::element(position)]);
    let lists = HashMap::from([(member.to_string(), rendered.to_string())]);
    let violation = enum_violation(element, value, declared);
    one_clause(&violation, &lists)
}

/// One violation of an enumerated type's value set, recorded the way the constraint walk
/// records one so the clause writer cannot tell the two apart.
///
/// The value is quoted under the SAME ceiling as a clause restored from that walk
/// (`service_common::validation::quoted_echo`), so neither refusal path of this service
/// states a message sized by the request. The membership check itself is made against the
/// value the caller sent, never against this rendering.
fn enum_violation(path: String, value: &str, declared: &[&str]) -> Violation {
    Violation {
        path,
        kind: ViolationKind::EnumValue(
            declared.iter().map(|value| (*value).to_string()).collect(),
        ),
        value: Some(quoted_echo(value).into_owned()),
        sensitive: false,
    }
}

/// The whole `message` of a refusal over the single violation `violation`, through
/// [`message`] so its envelope, its clause and its requirement are the ones every other
/// refusal of this service carries. A violation class [`message`] has no wording for
/// keeps the framework's own sentence rather than losing the clause — the same fallback
/// [`above_ceiling`] makes.
fn one_clause(violation: &Violation, lists: &HashMap<String, String>) -> String {
    message(std::slice::from_ref(violation), lists).unwrap_or_else(|| violation.message())
}

/// Every constraint the ELEMENT shape of the list member at `list_path` declares, as the
/// requirement sentences a nested clause lists, or `None` when this service's schemas
/// describe no one set for it.
///
/// Read off the model-derived schemas: codegen attaches a collection member's element
/// constraints to that member as an `ElementConstraintsTrait`, which is where the
/// boundary walk reads them too, so this cannot state a bound the walk does not enforce.
/// The order is the one AWS reports: the length maximum, the length minimum, the numeric
/// bounds, the pattern, then the value set.
///
/// Resolved by the member NAME the path ends in, across every shape a request of this
/// service carries ([`request_shapes`]), because this is reached from the violations and
/// not from the shape they were found in — a [`Violation`] carries a dotted path and no
/// schema, and a member schema carries its own name and nothing of the shape it targets,
/// so the path cannot be walked back through the model at runtime. A name two shapes
/// declare with DIFFERENT element constraints is therefore refused here rather than
/// guessed at: the clause then states the constraint that was broken, which is never
/// wrong, instead of a set another shape declares.
fn element_constraints(list_path: &str) -> Option<Vec<String>> {
    let name = list_path.rsplit('.').next()?;
    let mut resolved: Option<Vec<String>> = None;
    for shape in request_shapes() {
        let Some(traits) = shape
            .members()
            .iter()
            .find(|member| {
                matches!(member.shape_type(), ShapeType::List)
                    && member
                        .member_name()
                        .is_some_and(|declared| path::member(declared) == name)
            })
            .and_then(|member| member.traits())
            .and_then(|traits| traits.get(&ElementConstraintsTrait::TRAIT_ID))
            .and_then(|wrapper| wrapper.as_any().downcast_ref::<ElementConstraintsTrait>())
            .map(ElementConstraintsTrait::inner)
        else {
            continue;
        };
        let declared = declared_constraints(traits);
        if declared.is_empty() {
            continue;
        }
        match &resolved {
            Some(first) if *first != declared => return None,
            Some(_) => {}
            None => resolved = Some(declared),
        }
    }
    resolved
}

/// The requirement sentences for the constraints `traits` holds, in the order a nested
/// clause lists them.
fn declared_constraints(traits: &TraitMap) -> Vec<String> {
    let mut declared: Vec<ViolationKind> = Vec::new();
    if let Some(length) = traits
        .get(&LengthTrait::TRAIT_ID)
        .and_then(|found| found.as_any().downcast_ref::<LengthTrait>())
    {
        declared.extend(length.max.map(ViolationKind::MaxLength));
        declared.extend(length.min.map(ViolationKind::MinLength));
    }
    if let Some(range) = traits
        .get(&RangeTrait::TRAIT_ID)
        .and_then(|found| found.as_any().downcast_ref::<RangeTrait>())
    {
        declared.extend(range.max.map(|max| ViolationKind::MaxValue(bound(max))));
        declared.extend(range.min.map(|min| ViolationKind::MinValue(bound(min))));
    }
    if let Some(pattern) = traits
        .get(&PatternTrait::TRAIT_ID)
        .and_then(|found| found.as_any().downcast_ref::<PatternTrait>())
    {
        declared.push(ViolationKind::Pattern(pattern.regex.to_string()));
    }
    if let Some(values) = traits
        .get(&EnumTrait::TRAIT_ID)
        .and_then(|found| found.as_any().downcast_ref::<EnumTrait>())
    {
        declared.push(ViolationKind::EnumValue(
            values.values.iter().map(|value| (*value).to_string()).collect(),
        ));
    }
    declared
        .iter()
        .filter_map(framework_requirement)
        .collect()
}

/// A numeric `@range` bound as a requirement states it: an integral bound without a
/// trailing `.0`, a fractional one with its decimals — the rendering the framework uses
/// for the same bound when it reports the violation itself.
fn bound(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Every shape a REQUEST of this service carries, as the schemas codegen derives from
/// the Smithy model — the only runtime source for what a member declares: the five
/// operation inputs plus the two shapes nested under `GetFreeTierUsage`'s `filter`.
///
/// A hand-maintained restatement of a model fact drifts, and silently: a shape left out
/// of it loses rule 4 for its list members. So it is not left to review —
/// [`tests::every_constrained_list_member_of_the_model_resolves_its_element_constraints`]
/// derives the set from the model itself and fails when this does not cover it.
fn request_shapes() -> [&'static Schema; 7] {
    use crate::ops::types;

    [
        types::GET_ACCOUNT_ACTIVITY_INPUT_SCHEMA,
        types::GET_ACCOUNT_PLAN_STATE_INPUT_SCHEMA,
        types::GET_FREE_TIER_USAGE_INPUT_SCHEMA,
        types::LIST_ACCOUNT_ACTIVITIES_INPUT_SCHEMA,
        types::UPGRADE_ACCOUNT_PLAN_INPUT_SCHEMA,
        types::EXPRESSION_SCHEMA,
        types::DIMENSION_VALUES_SCHEMA,
    ]
}

#[cfg(test)]
mod tests {
    //! The clause rules this module owns, each over a SET of inputs rather than the one
    //! recorded case. The same rules are pinned on the wire, through real requests, in
    //! `tests/all/validation_message_rules.rs`.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: no non-test build needs it.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-freetier-awsfreetierservice.json"
    );

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: &str) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: Some(value.to_string()),
            sensitive: false,
        }
    }

    /// The model, parsed.
    fn model() -> serde_json::Value {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        serde_json::from_str(&model).expect("the model that generated this crate is JSON")
    }

    /// RULE: the envelope this module writes is the framework's own, for one clause and
    /// for several.
    ///
    /// Pinned against `aws_smithy_schema_server::envelope` itself — the rows differ in
    /// the number of violations, which is the only thing that envelope varies with — so
    /// the restatement cannot drift from it.
    #[test]
    fn the_envelope_counts_and_joins_the_way_the_framework_does() {
        for count in 1..=4_usize {
            let violations: Vec<Violation> = (0..count)
                .map(|index| {
                    violation(
                        &format!("member{index}"),
                        ViolationKind::MinLength(1),
                        "",
                    )
                })
                .collect();
            let clauses: Vec<String> = violations
                .iter()
                .map(aws_smithy_schema_server::constraints::Violation::message)
                .collect();
            let expected = aws_smithy_schema_server::envelope(&violations);
            let actual = envelope(&clauses);
            assert_eq!(
                actual, expected,
                "case {count} violations: expected the framework's envelope {expected:?}, \
                 got {actual:?}"
            );
        }
    }

    /// One row of the table below: what distinguishes the case, the violations the walk
    /// found, the rendered lists in hand, and the message stated.
    type ElementRow = (
        &'static str,
        Vec<Violation>,
        HashMap<String, String>,
        String,
    );

    /// RULE: a violation on a list's ELEMENTS is stated once, at the list member, with
    /// the whole list as the value and every constraint the element shape declares
    /// nested.
    ///
    /// The rows differ along the list member, the constraint class broken, the number of
    /// failing elements and whether the request's rendered list is in hand, so nothing of
    /// one caller's shape decides the clause.
    #[test]
    fn a_list_elements_failure_is_stated_at_the_list_with_every_element_constraint() {
        let values_constraints = "Member must satisfy constraint: [Member must have length \
             less than or equal to 20, Member must have length greater than or equal to 1, \
             Member must satisfy regular expression pattern: \
             [ a-zA-Z0-9\\-\\:\\.\\_\\/\\,\\$\\(\\)]*]";

        let rows: Vec<ElementRow> = vec![
            (
                "one element outside the element pattern",
                vec![violation(
                    "filter.dimensions.values.1.member",
                    ViolationKind::Pattern("^[ a-zA-Z0-9\\-\\:\\.\\_\\/\\,\\$\\(\\)]*$".to_string()),
                    "Amazon!EC2",
                )],
                HashMap::from([(
                    "filter.dimensions.values".to_string(),
                    "[Amazon!EC2]".to_string(),
                )]),
                format!(
                    "1 validation error detected: Value '[Amazon!EC2]' at \
                     'filter.dimensions.values' failed to satisfy constraint: \
                     {values_constraints}"
                ),
            ),
            (
                "one bad element among good ones, the whole list stated",
                vec![violation(
                    "filter.dimensions.values.2.member",
                    ViolationKind::MaxLength(20),
                    "an-overlong-service-name",
                )],
                HashMap::from([(
                    "filter.dimensions.values".to_string(),
                    "[AWSLambda, an-overlong-service-name, AmazonEC2]".to_string(),
                )]),
                format!(
                    "1 validation error detected: Value '[AWSLambda, an-overlong-service-name, \
                     AmazonEC2]' at 'filter.dimensions.values' failed to satisfy constraint: \
                     {values_constraints}"
                ),
            ),
            (
                "two elements of one list, each broken differently: ONE clause",
                vec![
                    violation(
                        "filter.dimensions.values.1.member",
                        ViolationKind::Pattern(
                            "^[ a-zA-Z0-9\\-\\:\\.\\_\\/\\,\\$\\(\\)]*$".to_string(),
                        ),
                        "Amazon!EC2",
                    ),
                    violation(
                        "filter.dimensions.values.2.member",
                        ViolationKind::MaxLength(20),
                        "an-overlong-service-name",
                    ),
                ],
                HashMap::from([(
                    "filter.dimensions.values".to_string(),
                    "[Amazon!EC2, an-overlong-service-name]".to_string(),
                )]),
                format!(
                    "1 validation error detected: Value '[Amazon!EC2, an-overlong-service-name]' \
                     at 'filter.dimensions.values' failed to satisfy constraint: \
                     {values_constraints}"
                ),
            ),
            (
                "no rendered list in hand: the element's own value is stated",
                vec![violation(
                    "filter.dimensions.values.1.member",
                    ViolationKind::MaxLength(20),
                    "an-overlong-service-name",
                )],
                HashMap::new(),
                format!(
                    "1 validation error detected: Value 'an-overlong-service-name' at \
                     'filter.dimensions.values' failed to satisfy constraint: \
                     {values_constraints}"
                ),
            ),
        ];

        for (case, violations, lists, expected) in rows {
            let actual = refusal(&violations, || lists.clone());
            let message = actual.as_ref().map(|refused| refused.message.as_str());
            assert_eq!(
                message,
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            let code = actual.as_ref().map(|refused| refused.code.as_str());
            assert_eq!(
                code,
                Some(VALIDATION_EXCEPTION),
                "case {case:?}: a chosen message travels with its code, got {code:?}"
            );
        }
    }

    /// RULE: a clause about a member that is not a list element states the value as
    /// submitted, the member path and the requirement — a `@pattern` without its
    /// whole-value anchors, a collection's own `@length` with no value at all.
    #[test]
    fn a_members_own_clause_states_the_value_the_path_and_the_requirement() {
        let overlong = "x".repeat(33);

        // (case, the violation, the message stated)
        let rows: Vec<(&str, Violation, String)> = vec![
            (
                "a @pattern on a scalar member loses the anchors",
                violation(
                    "activityId",
                    ViolationKind::Pattern("^[a-zA-Z0-9]+$".to_string()),
                    "not-an-id",
                ),
                "1 validation error detected: Value 'not-an-id' at 'activityId' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 [a-zA-Z0-9]+"
                    .to_string(),
            ),
            (
                "a @length maximum on a scalar member",
                violation("activityId", ViolationKind::MaxLength(32), &overlong),
                format!(
                    "1 validation error detected: Value '{overlong}' at 'activityId' failed to \
                     satisfy constraint: Member must have length less than or equal to 32"
                ),
            ),
            (
                "a @range minimum on a scalar member",
                violation("maxResults", ViolationKind::MinValue("1".to_string()), "0"),
                "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1"
                    .to_string(),
            ),
            (
                "an absent @required member",
                Violation {
                    path: "accountPlanType".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                "1 validation error detected: Value null at 'accountPlanType' failed to \
                 satisfy constraint: Member must not be null"
                    .to_string(),
            ),
            (
                "a LIST's own @length states no value",
                violation(
                    "filter.dimensions.values",
                    ViolationKind::MinLength(1),
                    "0 items",
                ),
                "1 validation error detected: Value at 'filter.dimensions.values' failed to \
                 satisfy constraint: Member must have length greater than or equal to 1"
                    .to_string(),
            ),
        ];

        for (case, broken, expected) in rows {
            let actual = refusal(std::slice::from_ref(&broken), HashMap::new);
            let message = actual.as_ref().map(|refused| refused.message.as_str());
            assert_eq!(
                message,
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// Nothing to word is `None`, so the framework's own answer stands.
    #[test]
    fn no_violation_is_no_wording() {
        let actual = refusal(&[], HashMap::new);
        assert!(
            actual.is_none(),
            "case no violations: expected no wording, got {actual:?}"
        );
    }

    /// GUARD on [`request_shapes`]: every list member the MODEL declares with
    /// constrained elements, anywhere a request can carry it, resolves its element
    /// constraint set — and resolves it to exactly the constraints the model declares.
    ///
    /// The members come from the model rather than from a second hand list, so a shape
    /// added to the model and not to [`request_shapes`] fails here instead of silently
    /// falling back to the one constraint a request happened to break.
    #[test]
    fn every_constrained_list_member_of_the_model_resolves_its_element_constraints() {
        let model = model();
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");

        // Every shape a request can carry: the operation inputs and, transitively, the
        // shapes their members target.
        let mut reachable: Vec<String> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(_, shape)| shape["input"]["target"].as_str().map(str::to_string))
            .collect();
        let mut seen: HashSet<String> = reachable.iter().cloned().collect();
        let mut index = 0;
        while index < reachable.len() {
            let id = reachable[index].clone();
            index += 1;
            let Some(shape) = shapes.get(id.as_str()) else {
                continue;
            };
            let targets = shape["members"]
                .as_object()
                .map(|members| {
                    members
                        .values()
                        .filter_map(|member| member["target"].as_str().map(str::to_string))
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default();
            let collection: Vec<String> = ["member", "value"]
                .iter()
                .filter_map(|part| shape[*part]["target"].as_str().map(str::to_string))
                .collect();
            for target in targets.into_iter().chain(collection) {
                if shapes.contains_key(&target) && seen.insert(target.clone()) {
                    reachable.push(target);
                }
            }
        }

        // The list members those shapes declare whose ELEMENT shape carries constraints,
        // as (the member path segment the walk reports, the requirement sentences).
        let mut constrained: Vec<(String, Vec<String>)> = Vec::new();
        for id in &reachable {
            let Some(shape) = shapes.get(id.as_str()) else {
                continue;
            };
            let Some(members) = shape["members"].as_object() else {
                continue;
            };
            for (name, member) in members {
                let target = member["target"].as_str().unwrap_or_default();
                let Some(list) = shapes.get(target) else {
                    continue;
                };
                if list["type"] != "list" {
                    continue;
                }
                let element = list["member"]["target"].as_str().unwrap_or_default();
                let declared = modelled_constraints(shapes, element);
                if declared.is_empty() {
                    continue;
                }
                constrained.push((path::member(name), declared));
            }
        }
        assert!(
            constrained.len() >= 3,
            "case the model's constrained list members: expected the ones this service \
             declares (DimensionValues$Values, DimensionValues$MatchOptions, \
             ListAccountActivitiesRequest$filterActivityStatuses), got {constrained:?}"
        );

        for (member, declared) in constrained {
            let actual = element_constraints(&format!("some.path.{member}")).map(|sentences| {
                sentences
                    .iter()
                    .map(|sentence| comparable(sentence))
                    .collect::<Vec<String>>()
            });
            let expected: Vec<String> = declared
                .iter()
                .map(|sentence| comparable(sentence))
                .collect();
            assert_eq!(
                actual.as_ref(),
                Some(&expected),
                "case the list member {member:?}: expected the element constraints the model \
                 declares, {expected:?}, got {actual:?}"
            );
        }
    }

    /// One requirement sentence, with an enumerated VALUE SET sorted so two readings of
    /// one model can be compared.
    ///
    /// The ORDER a value set is listed in is the model's declaration order, which is
    /// what the schemas carry and what a clause states; this test reads the model through
    /// `serde_json`, whose object is sorted by key, so it can compare WHICH values the
    /// set holds and not the order. The order a clause states is pinned where it is
    /// observable — on the wire, in `tests/all/validation_message_rules.rs`.
    fn comparable(sentence: &str) -> String {
        /// The opening of the requirement that reports an enumerated type's value set.
        const VALUE_SET: &str = "Member must satisfy enum value set: [";

        match sentence
            .strip_prefix(VALUE_SET)
            .and_then(|rest| rest.strip_suffix(']'))
        {
            Some(values) => {
                let mut values: Vec<&str> = values.split(", ").collect();
                values.sort_unstable();
                format!("{VALUE_SET}{}]", values.join(", "))
            }
            None => sentence.to_owned(),
        }
    }

    /// The requirement sentences for the constraints the model declares on `shape`, in
    /// the order a nested clause lists them. The expectation side of the guard above:
    /// read from the model's own traits rather than from the schemas the code reads.
    fn modelled_constraints(
        shapes: &serde_json::Map<String, serde_json::Value>,
        shape_id: &str,
    ) -> Vec<String> {
        let Some(shape) = shapes.get(shape_id) else {
            return Vec::new();
        };
        let traits = &shape["traits"];
        let mut declared: Vec<ViolationKind> = Vec::new();
        let length = &traits["smithy.api#length"];
        if let Some(max) = length["max"].as_i64() {
            declared.push(ViolationKind::MaxLength(max));
        }
        if let Some(min) = length["min"].as_i64() {
            declared.push(ViolationKind::MinLength(min));
        }
        let range = &traits["smithy.api#range"];
        if let Some(max) = range["max"].as_f64() {
            declared.push(ViolationKind::MaxValue(bound(max)));
        }
        if let Some(min) = range["min"].as_f64() {
            declared.push(ViolationKind::MinValue(bound(min)));
        }
        if let Some(pattern) = traits["smithy.api#pattern"].as_str() {
            declared.push(ViolationKind::Pattern(pattern.to_string()));
        }
        // Both spellings of an enumerated type: the Smithy 1.0 `@enum` trait, and the
        // `enum` shape whose MEMBERS carry the values — each member's `@enumValue`, or
        // its name where it declares none, which is how codegen reads the same shape.
        let mut values: Vec<String> = traits["smithy.api#enum"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry["value"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if values.is_empty() && shape["type"] == "enum" {
            values = shape["members"]
                .as_object()
                .map(|members| {
                    members
                        .iter()
                        .map(|(name, member)| {
                            member["traits"]["smithy.api#enumValue"]
                                .as_str()
                                .unwrap_or(name)
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        if !values.is_empty() {
            declared.push(ViolationKind::EnumValue(values));
        }
        declared.iter().filter_map(framework_requirement).collect()
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-constraint refusal: how the structured [`Violation`]s
//! the framework's boundary walk produces become the `message` the modelled
//! `ValidationException` (HTTP 400) carries.
//!
//! [`crate::service`] owns the SEAM — which request is validated, and how the violations
//! and the submitted values are collected — and this module owns the sentence, so the
//! clause rules live in one place instead of in each operation that trips them. WHAT is
//! enforced is always the model's: every violation handed to [`refusal`] comes from
//! `aws_smithy_schema_server::constraints`, which reads the `@length`, `@range`,
//! `@pattern`, `@enum` and `@uniqueItems` traits codegen captured off the model, and
//! nothing here accepts or refuses anything.
//!
//! The rules, each holding for every constrained member of every operation rather than for
//! the members one report happened to cite:
//!
//! 1. the envelope is AWS's `N validation error(s) detected: ` over one clause per failing
//!    (member, requirement), joined with `; `, in the order the walk found them
//!    ([`envelope`]);
//! 2. a clause names the member's dotted path and the requirement it failed. Whether it
//!    also quotes the VALUE the member held depends on which of this endpoint's backends
//!    serves the operation — rule 5;
//! 3. a reported `@pattern` is the expression WITHOUT the `^` / `$` anchors the model
//!    spells it with (`service_common::validation::unanchored`): the advertised constraint
//!    is the character grammar a value must satisfy, and a caller that lifts the expression
//!    out of the message to re-validate locally must get the expression the service
//!    publishes, not the model's way of making the match total. The check itself still uses
//!    the model's pattern as written: only the RENDERING drops them;
//! 4. a violation on the ELEMENTS of a collection is reported against the COLLECTION MEMBER
//!    — no element position and no `member` step — with the ELEMENT SHAPE'S WHOLE
//!    CONSTRAINT SET nested inside `Member must satisfy constraint: [ … ]`
//!    ([`modelled_element_requirements`]). The set is the element's, not just the
//!    constraint that happened to fail: the clause tells a caller what every element of
//!    that member has to satisfy. That is why two bad elements of one list are ONE clause,
//!    while a member that broke two different rules still states both. A violation on the
//!    collection ITSELF (its own `@length`, its `@uniqueItems`) is not nested;
//! 5. the VALUE: a clause quotes the value the member held — except for an operation served
//!    by one of the backends listed in [`VALUE_LESS_FAMILIES`], whose clause states the
//!    member and the requirement alone. See that constant for the evidence and for why the
//!    family, not the member, decides.
//!
//! Rule 4's constraint set is read off the MODEL, not off the refusal: the structured
//! refusal carries only the constraints that failed, so the ones a value satisfied would
//! otherwise be missing from the clause. Codegen attaches a collection member's element
//! constraints to that member's schema as an
//! [`ElementConstraintsTrait`](aws_smithy_schema_server::constraints::ElementConstraintsTrait),
//! which is why [`refusal`] takes the operation's input schema.
//!
//! The status (400) and the error code ([`VALIDATION_EXCEPTION`]) are what the framework
//! already answers a constraint failure with on this `restJson1` service, and what every
//! operation of this service models for one; they are restated, not changed, because a
//! chosen [`Refused`] replaces the code and the message together and a client branches on
//! the code.

use std::collections::{HashMap, HashSet};

use aws_smithy_schema::{Schema, ShapeId, TraitMap};
use aws_smithy_schema_server::constraints::{
    self, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait, Violation,
    ViolationKind,
};
use service_common::validation::{list_of_element, unanchored};
use smithy_server_runtime::Refused;

/// The error code a constraint failure is named by: the shape
/// `com.amazonaws.bedrockagentcorecontrol#ValidationException`, which every operation of
/// this service declares and whose `@httpError` is 400 — and the code the framework's own
/// refusal channel picks for this `restJson1` service. Restated here because a chosen
/// [`Refused`] carries both halves.
pub(super) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The first URI segment of every resource family whose backend states a constraint
/// refusal WITHOUT echoing the value the request carried.
///
/// WHY A FAMILY AND NOT A MEMBER: `bedrock-agentcore-control` is one endpoint in front of
/// several AgentCore services, one per resource family, and they do not word a refusal
/// alike. The recordings show the SAME violation — member path `name`, `@pattern`
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` — quoted with the value under `/browser-profiles` and
/// without it under `/memories`, so no rule over the member, its target shape or the
/// constraint it broke can produce both: what differs is which backend answered. The
/// family is read off the first segment of the `@http` URI the operation's own model
/// declares ([`echo_of`]), so every operation of a listed family is worded alike and an
/// operation added to one later needs no change here.
///
/// Attested, family by family, by the recorded refusals this wording was derived from:
/// `/memories` (`CreateMemory`), `/configuration-bundles`
/// (`CreateConfigurationBundle`) and `/identities` (`CreateConsentPortal`,
/// `CreateWorkloadIdentity`, `UpdateWorkloadIdentity`). Every other family echoes the
/// value, which is equally attested: `/runtimes` (`CreateAgentRuntime`),
/// `/browser-profiles` (`ListBrowserProfiles`), `/payments` (`CreatePaymentManager`), and
/// `/harnesses` + `/gateways` through this crate's own suite.
const VALUE_LESS_FAMILIES: &[&str] = &["memories", "configuration-bundles", "identities"];

/// The clause a collection states around its element shape's constraint set (rule 4).
const ELEMENT_SET_OPEN: &str = "Member must satisfy constraint: [";

/// The text that separates a clause's subject from its requirement.
const CLAUSE_MARKER: &str = "failed to satisfy constraint: ";

/// Whether the backend serving an operation quotes the value a refused member held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Echo {
    /// The clause quotes the value as the caller submitted it.
    Submitted,
    /// The clause states the member and the requirement alone.
    Suppressed,
}

/// The echo policy for the operation whose input is `input_schema` (rule 5): decided by
/// the resource family the FIRST segment of that operation's modelled `@http` URI names.
///
/// An operation whose input schema carries no HTTP binding, or whose URI has no first
/// segment, is not in a listed family and so keeps the echo every other family shows.
pub(super) fn echo_of(input_schema: &Schema) -> Echo {
    let family = input_schema
        .http()
        .map(|http| http.uri())
        .and_then(|uri| service_common::nth_path_segment(uri, 0));
    match family {
        Some(family) if VALUE_LESS_FAMILIES.contains(&family) => Echo::Suppressed,
        _ => Echo::Submitted,
    }
}

/// This service's wording for the violations the framework found, or `None` when there is
/// nothing to word (no violations) or one of them is a class this module has no wording for
/// ([`ViolationKind`] is `#[non_exhaustive]`), which keeps the framework's own whole
/// message rather than inventing a sentence for it.
///
/// `input_schema` is the refused operation's own input shape, which rule 4 reads the
/// element constraint sets off; `None` (a refusal that reached no operation) falls back to
/// the constraints that actually failed. `lists` carries every list of scalar elements the
/// request held, keyed by the member path the walk reports that list at and already
/// rendered (`service_common::validation::refusing_violations_with_lists`) — the value a
/// clause states AT a list member, which no violation carries.
pub(super) fn refusal(
    violations: &[Violation],
    input_schema: Option<&Schema>,
    echo: Echo,
    lists: &HashMap<String, String>,
) -> Option<Refused> {
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: message(violations, input_schema, echo, lists)?,
    })
}

/// The `message` for a set of violations: the count, then one clause per failing
/// (member, requirement) in the order the walk found them.
fn message(
    violations: &[Violation],
    input_schema: Option<&Schema>,
    echo: Echo,
    lists: &HashMap<String, String>,
) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    // Resolved once per collection member, before the clauses are built: the set a clause
    // states is the element shape's whole set, and reading it per violation would resolve
    // it once per failing ELEMENT — a count the caller chooses.
    let element_sets = element_sets(violations, input_schema)?;
    let mut stated: HashSet<(String, String)> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let element = list_of_element(&violation.path);
        let path = element.unwrap_or(violation.path.as_str());
        let requirement = match element {
            // Built from these same violations, so a collection named by one of them is
            // always in the map.
            Some(_) => nested(element_sets.get(path)?),
            None => requirement_of(&violation.kind)?,
        };
        if !stated.insert((path.to_string(), requirement.clone())) {
            continue;
        }
        let subject = subject(echo, violation, element.and_then(|list| lists.get(list)));
        clauses.push(format!("{subject} at '{path}' {CLAUSE_MARKER}{requirement}"));
    }
    envelope(&clauses)
}

/// AWS's envelope over the clauses of one refusal: `1 validation error detected: <clause>`
/// for one, `N validation errors detected: <clause>; <clause>` for several.
///
/// The wording is `aws_smithy_schema_server::constraints::envelope`'s, which cannot be
/// CALLED here: it renders each clause from a [`Violation`], and rule 4's nested
/// requirement is not a [`ViolationKind`] any violation can carry. The two must stay
/// identical, which [`tests::the_envelope_is_the_frameworks_own`] pins against that
/// function itself.
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

/// The element constraint set each collection member named by a violation states (rule 4),
/// keyed by that member's path.
///
/// The MODEL's set when the member declares one, and otherwise the constraints these
/// violations show its elements actually broke — which is what is known to be true of the
/// request, where a guessed set would not be. `None` when one of those constraints is a
/// class this module has no wording for, which declines the whole refusal.
fn element_sets(
    violations: &[Violation],
    input_schema: Option<&Schema>,
) -> Option<HashMap<String, Vec<String>>> {
    let mut broken: HashMap<String, Vec<String>> = HashMap::new();
    for violation in violations {
        let Some(list) = list_of_element(&violation.path) else {
            continue;
        };
        let requirement = requirement_of(&violation.kind)?;
        let failing = broken.entry(list.to_string()).or_default();
        if !failing.contains(&requirement) {
            failing.push(requirement);
        }
    }
    Some(
        broken
            .into_iter()
            .map(|(list, failing)| {
                let modelled = input_schema
                    .and_then(|schema| modelled_element_requirements(schema, &list))
                    .unwrap_or(failing);
                (list, modelled)
            })
            .collect(),
    )
}

/// `requirements` as a collection clause states them: the set, comma-separated, inside
/// rule 4's nesting.
fn nested(requirements: &[String]) -> String {
    format!("{ELEMENT_SET_OPEN}{}]", requirements.join(", "))
}

/// Every requirement the ELEMENT shape of `input_schema`'s `path` member declares, in the
/// order a clause states them, or `None` when that member is not a collection with
/// constrained elements.
///
/// `path` is matched against the schema's own TOP-LEVEL members, which is where this
/// model's constrained collections sit; a collection nested inside a structure member
/// resolves to `None` and its clause states the constraints that failed instead
/// ([`element_sets`]), which is a smaller set, never a wrong one.
fn modelled_element_requirements(input_schema: &Schema, path: &str) -> Option<Vec<String>> {
    let member = input_schema.members().iter().find(|member| {
        member
            .member_name()
            .is_some_and(|name| constraints::path::member(name) == path)
    })?;
    let wrapper = constraint_trait::<ElementConstraintsTrait>(
        member.traits()?,
        &ElementConstraintsTrait::TRAIT_ID,
    )?;
    let requirements = element_requirements(wrapper.inner());
    (!requirements.is_empty()).then_some(requirements)
}

/// One constraint trait off a trait map, downcast to the type that carries its bounds.
fn constraint_trait<'map, T: 'static>(traits: &'map TraitMap, id: &ShapeId) -> Option<&'map T> {
    traits.get(id)?.as_any().downcast_ref::<T>()
}

/// Every requirement an element shape declares, in the order a clause states them.
///
/// The ORDER is the one this service's recorded element-constraint clause shows
/// (`UpdateWorkloadIdentity`'s `allowedResourceOauth2ReturnUrls`, whose element shape
/// carries `@length(1, 2048)` and a `@pattern`, is stated as
/// `[… length less than or equal to 2048, … length greater than or equal to 1, … pattern: …]`):
/// the `@length` maximum, then its minimum, then the numeric bounds the same way, then the
/// `@pattern`, then an enumerated value set. A `@length` with no declared minimum states
/// `0`, the smallest length a Smithy `@length` can bound.
///
/// Empty when the element shape declares no constraint this module words.
fn element_requirements(traits: &TraitMap) -> Vec<String> {
    let mut requirements: Vec<String> = Vec::new();
    if let Some(length) = constraint_trait::<LengthTrait>(traits, &LengthTrait::TRAIT_ID) {
        if let Some(max) = length.max {
            requirements.push(max_length(max));
        }
        requirements.push(min_length(length.min.unwrap_or(0)));
    }
    if let Some(range) = constraint_trait::<RangeTrait>(traits, &RangeTrait::TRAIT_ID) {
        if let Some(max) = range.max {
            requirements.push(max_value(&render_bound(max)));
        }
        if let Some(min) = range.min {
            requirements.push(min_value(&render_bound(min)));
        }
    }
    if let Some(pattern) = constraint_trait::<PatternTrait>(traits, &PatternTrait::TRAIT_ID) {
        requirements.push(pattern_requirement(pattern.regex));
    }
    if let Some(allowed) = constraint_trait::<EnumTrait>(traits, &EnumTrait::TRAIT_ID) {
        requirements.push(enum_requirement(allowed.values));
    }
    requirements
}

/// The requirement one violation states, or `None` for a class this module has no wording
/// for.
///
/// Every sentence is the framework's own
/// ([`tests::a_requirement_is_the_frameworks_own_sentence`] pins each against
/// [`Violation::message`]) except rule 3's `@pattern` rendering. They are spelled here
/// rather than read off a violation because rule 4's set is read off the MODEL, where
/// there is no violation to read a sentence from.
fn requirement_of(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => min_length(*min),
        ViolationKind::MaxLength(max) => max_length(*max),
        ViolationKind::MinValue(min) => min_value(min),
        ViolationKind::MaxValue(max) => max_value(max),
        ViolationKind::Pattern(pattern) => pattern_requirement(pattern),
        ViolationKind::EnumValue(allowed) => enum_requirement(allowed),
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// The `@length` maximum, as a requirement reads.
fn max_length(max: i64) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The `@length` minimum, as a requirement reads.
fn min_length(min: i64) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The `@range` maximum, as a requirement reads.
fn max_value(max: &str) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The `@range` minimum, as a requirement reads.
fn min_value(min: &str) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The `@pattern`, as a requirement reads: the expression UNANCHORED (rule 3), through the
/// tree's one implementation of that rendering.
fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(pattern)
    )
}

/// An enumerated value set, as a requirement reads.
fn enum_requirement<S: AsRef<str>>(values: &[S]) -> String {
    let values: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}

/// A `@range` bound as it displays: an integral value without a trailing `.0`, a
/// fractional value with its decimals — the same rendering the framework's own clause uses,
/// so a bound read off the model here and one read off a violation agree.
///
/// `{bound:.0}` rather than a cast through `i64`: a narrowing cast SATURATES, so a bound
/// whose magnitude is past `i64`'s would advertise `i64::MAX` instead of what the model
/// declares.
fn render_bound(bound: f64) -> String {
    if bound.fract() == 0.0 && bound.is_finite() {
        format!("{bound:.0}")
    } else {
        format!("{bound}")
    }
}

/// The value half of a clause (rule 5).
///
/// * an ABSENT member has no value at all — there is nothing of the caller's to echo or to
///   suppress — and is reported as the null it was, in either policy;
/// * under [`Echo::Suppressed`] the subject is the bare `Value`: the member and the
///   requirement are the whole of what the backends of [`VALUE_LESS_FAMILIES`] state;
/// * a clause stated at a COLLECTION member because one of its ELEMENTS failed echoes the
///   collection the caller sent, when the seam could render it in full;
/// * everything else echoes the value carried on the violation, which the seam has already
///   restored to the bytes the caller submitted.
fn subject(echo: Echo, violation: &Violation, rendered_list: Option<&String>) -> String {
    if violation.value.is_none() {
        return "Value null".to_string();
    }
    match (echo, rendered_list) {
        (Echo::Suppressed, _) => "Value".to_string(),
        (Echo::Submitted, Some(rendered)) => format!("Value '{rendered}'"),
        (Echo::Submitted, None) => {
            format!("Value '{}'", violation.value.as_deref().unwrap_or_default())
        }
    }
}

#[cfg(test)]
mod tests {
    //! The clause rules, read off violations built here rather than off a request: the
    //! end-to-end half (which operation is worded which way, and the sets this model
    //! declares) is `tests/all/validation_message_rendering.rs`.

    use super::*;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: value.map(str::to_owned),
            sensitive: false,
        }
    }

    /// The message for `violations` under `echo`, with no schema and no lists.
    fn stated(violations: &[Violation], echo: Echo) -> String {
        message(violations, None, echo, &HashMap::new()).expect("a violation is worded")
    }

    /// One violation and the clause each policy states for it.
    struct Row {
        /// What this row is a case of.
        case: &'static str,
        /// The member the violation names.
        path: &'static str,
        /// What the value failed.
        kind: ViolationKind,
        /// What the request put there, or `None` for a member it omitted.
        value: Option<&'static str>,
        /// The clause [`Echo::Submitted`] states.
        echoed: &'static str,
        /// The clause [`Echo::Suppressed`] states.
        suppressed: &'static str,
    }

    /// RULE 5 over the two policies, on the SAME violation: the member and the requirement
    /// are stated either way, and only the value differs.
    #[test]
    fn only_the_value_differs_between_the_two_policies() {
        let rows = [
            Row {
                case: "a @pattern on a name",
                path: "name",
                kind: ViolationKind::Pattern("^[a-zA-Z][a-zA-Z0-9_]{0,47}$".to_owned()),
                value: Some("1_starts_with_digit"),
                echoed: "Value '1_starts_with_digit' at 'name' failed to satisfy constraint: \
                         Member must satisfy regular expression pattern: \
                         [a-zA-Z][a-zA-Z0-9_]{0,47}",
                suppressed: "Value at 'name' failed to satisfy constraint: Member must \
                             satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9_]{0,47}",
            },
            Row {
                case: "a @length minimum",
                path: "clientToken",
                kind: ViolationKind::MinLength(33),
                value: Some("too-short"),
                echoed: "Value 'too-short' at 'clientToken' failed to satisfy constraint: \
                         Member must have length greater than or equal to 33",
                suppressed: "Value at 'clientToken' failed to satisfy constraint: Member must \
                             have length greater than or equal to 33",
            },
            Row {
                case: "a @range maximum",
                path: "eventExpiryDuration",
                kind: ViolationKind::MaxValue("365".to_owned()),
                value: Some("400"),
                echoed: "Value '400' at 'eventExpiryDuration' failed to satisfy constraint: \
                         Member must have value less than or equal to 365",
                suppressed: "Value at 'eventExpiryDuration' failed to satisfy constraint: \
                             Member must have value less than or equal to 365",
            },
            Row {
                case: "an enum value set",
                path: "view",
                kind: ViolationKind::EnumValue(vec!["full".to_owned(), "summary".to_owned()]),
                value: Some("partial"),
                echoed: "Value 'partial' at 'view' failed to satisfy constraint: Member must \
                         satisfy enum value set: [full, summary]",
                suppressed: "Value at 'view' failed to satisfy constraint: Member must \
                             satisfy enum value set: [full, summary]",
            },
            Row {
                // No value either way: an absent member has nothing of the caller's in it.
                case: "an absent @required member",
                path: "name",
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                echoed: "Value null at 'name' failed to satisfy constraint: Member must not \
                         be null",
                suppressed: "Value null at 'name' failed to satisfy constraint: Member must \
                             not be null",
            },
        ];

        for row in rows {
            let case = row.case;
            let one = [violation(row.path, row.kind, row.value)];
            for (policy, expected) in [
                (Echo::Submitted, row.echoed),
                (Echo::Suppressed, row.suppressed),
            ] {
                let actual = stated(&one, policy);
                let expected = format!("1 validation error detected: {expected}");
                assert_eq!(
                    actual, expected,
                    "case {case:?} under {policy:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }

    /// RULE 1: two different requirements on one member are two clauses, counted and joined
    /// the way AWS counts and joins them.
    #[test]
    fn two_requirements_on_one_member_are_two_clauses() {
        let violations = [
            violation("name", ViolationKind::MinLength(1), Some("")),
            violation(
                "name",
                ViolationKind::Pattern("^[a-zA-Z][a-zA-Z0-9]{0,47}$".to_owned()),
                Some(""),
            ),
        ];
        let actual = stated(&violations, Echo::Submitted);
        let expected = "2 validation errors detected: Value '' at 'name' failed to satisfy \
                        constraint: Member must have length greater than or equal to 1; Value \
                        '' at 'name' failed to satisfy constraint: Member must satisfy regular \
                        expression pattern: [a-zA-Z][a-zA-Z0-9]{0,47}";
        assert_eq!(
            actual, expected,
            "case \"a length and a pattern on one member\": expected {expected:?}, got \
             {actual:?}"
        );
    }

    /// RULE 4 without a schema: several failing elements of ONE collection are a SINGLE
    /// clause, stated at the collection member with the constraints they broke — never at
    /// an element position.
    #[test]
    fn failing_elements_fold_into_one_clause_at_their_collection() {
        let pattern = ViolationKind::Pattern(r"^\w+:(\/?\/?)[^\s]+$".to_owned());
        let violations = [
            violation("urls.1.member", pattern.clone(), Some("not a url")),
            violation("urls.3.member", pattern, Some("nor this")),
            violation("urls.3.member", ViolationKind::MinLength(1), Some("")),
        ];
        let actual = stated(&violations, Echo::Suppressed);
        let expected = "1 validation error detected: Value at 'urls' failed to satisfy \
                        constraint: Member must satisfy constraint: [Member must satisfy \
                        regular expression pattern: \\w+:(\\/?\\/?)[^\\s]+, Member must have \
                        length greater than or equal to 1]";
        assert_eq!(
            actual, expected,
            "case \"three element violations of one list\": expected {expected:?}, got \
             {actual:?}"
        );
        assert!(
            !actual.contains(".member"),
            "case \"three element violations of one list\": a clause names the collection, \
             never an element position, got {actual:?}"
        );
    }

    /// RULE 1's envelope is the framework's own, pinned against the function that writes it
    /// — which this module cannot call for a nested requirement.
    #[test]
    fn the_envelope_is_the_frameworks_own() {
        for count in 1_usize..4 {
            let violations: Vec<Violation> = (0..count)
                .map(|index| {
                    violation(
                        &format!("member{index}"),
                        ViolationKind::MinLength(1),
                        Some(""),
                    )
                })
                .collect();
            let framework = constraints::envelope(&violations).expect("violations are worded");
            let clauses: Vec<String> = violations.iter().map(Violation::message).collect();
            let actual = envelope(&clauses).expect("clauses are worded");
            assert_eq!(
                actual, framework,
                "case {count} clause(s): expected the framework's envelope {framework:?}, got \
                 {actual:?}"
            );
        }
    }

    /// RULE 2/3: every requirement this module spells is the framework's own sentence,
    /// except that a `@pattern` is quoted unanchored — so a constraint class the framework
    /// words differently cannot drift out of step with it.
    #[test]
    fn a_requirement_is_the_frameworks_own_sentence() {
        let kinds = [
            ("an absent member", ViolationKind::MissingRequiredMember),
            ("a @length minimum", ViolationKind::MinLength(3)),
            ("a @length maximum", ViolationKind::MaxLength(2048)),
            ("a @range minimum", ViolationKind::MinValue("1".to_owned())),
            ("a @range maximum", ViolationKind::MaxValue("100".to_owned())),
            // Already unanchored, so rule 3 is the identity here and the two must agree.
            (
                "an unanchored @pattern",
                ViolationKind::Pattern(r"\S*".to_owned()),
            ),
            (
                "an enum value set",
                ViolationKind::EnumValue(vec!["A".to_owned(), "B".to_owned()]),
            ),
            ("@uniqueItems", ViolationKind::UniqueItems),
        ];
        for (case, kind) in kinds {
            let framework = violation("member", kind.clone(), None)
                .message()
                .rsplit_once(CLAUSE_MARKER)
                .map(|(_, requirement)| requirement.to_string())
                .expect("the framework states a requirement");
            let actual = requirement_of(&kind).expect("this module words the class");
            assert_eq!(
                actual, framework,
                "case {case:?}: expected the framework's sentence {framework:?}, got {actual:?}"
            );
        }
    }

    /// RULE 3 goes through the tree's one `unanchored`: the model's outer `^`/`$` pair is
    /// dropped positionally, and an expression without one is quoted as the model spells it.
    #[test]
    fn a_pattern_is_quoted_without_its_whole_value_anchors() {
        // (case, the model's expression, what the clause quotes). The rows span this
        // model's own pattern shapes and the shapes the rule must leave alone.
        let rows: [(&str, &str, &str); 7] = [
            (
                "a name's character class",
                "^[a-zA-Z][a-zA-Z0-9_]{0,47}$",
                "[a-zA-Z][a-zA-Z0-9_]{0,47}",
            ),
            (
                "a RoleArn, whose class negation is not an anchor",
                "^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$",
                "arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+",
            ),
            (
                "a version's anchored group",
                "^([1-9][0-9]{0,4})$",
                "([1-9][0-9]{0,4})",
            ),
            ("anchored at the start only", "^arn:aws:", "^arn:aws:"),
            ("anchored at the end only", "[0-9]{12}$", "[0-9]{12}$"),
            // The outer pair is dropped POSITIONALLY — see
            // `service_common::validation::unanchored`, the tree's one implementation of
            // this rule: the published expression is the service's own with that pair
            // written around it, so a top-level alternation does not keep it (a lambdacore
            // recording shows AWS dropping it from one). An expression that anchors inside
            // its branches has no outer pair and is quoted verbatim.
            ("a fully anchored alternation", "^a$|^b$", "a$|^b"),
            ("anchors inside the branches", "(^a$)|(^b$)", "(^a$)|(^b$)"),
        ];
        for (case, modelled, expected) in rows {
            let stated = requirement_of(&ViolationKind::Pattern(modelled.to_owned()))
                .expect("a pattern is worded");
            let actual = stated
                .rsplit_once("pattern: ")
                .map(|(_, quoted)| quoted.to_string())
                .expect("a pattern clause quotes an expression");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {modelled:?} to be quoted as {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    /// Nothing to word is `None`, so the framework's own answer stands.
    #[test]
    fn no_violation_is_worded_at_all() {
        assert!(
            refusal(&[], None, Echo::Submitted, &HashMap::new()).is_none(),
            "expected no violations to state nothing"
        );
    }
}

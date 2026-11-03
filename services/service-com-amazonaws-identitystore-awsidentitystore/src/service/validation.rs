// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request the boundary validator refused.
//!
//! The validator checks a request against the model's own constraints BEFORE any handler
//! runs and hands the service the violations it found, each as a member path, the
//! constraint class it failed and the value it held. [`message`] turns those into the
//! `ValidationException` message this service states; it is reached from
//! [`crate::service`]'s `override_refusal`, the one seam that sees a pre-handler refusal.
//!
//! Three rules distinguish it from the framework's default wording, and each holds for
//! every member of every operation, not for the ones a report happened to cite:
//!
//! 1. a reported `@pattern` is the expression the service APPLIES, not the model's trait
//!    text: Smithy pattern semantics are a find, so the `^`/`$` anchors are not part of
//!    it, and an ARN pattern carries the partition grammar rather than the model's loose
//!    wildcard ([`runtime_pattern`]);
//! 2. the offending value is NOT echoed for a member the model marks `@sensitive`
//!    ([`SENSITIVE_MEMBERS`]) — the member path and the constraint still are;
//! 3. violations on the scalar ELEMENTS of a list member are reported once, at the list
//!    member, with the full set of constraints its element shape declares
//!    ([`element_constraint_clauses`]).
//!
//! NOTE on where this belongs: the three rules are AWS's wording, not this service's,
//! and sibling services carry their own copies of parts of them (an `unanchored` helper,
//! the same composite list-member clause, the envelope itself — `grep -rln 'fn
//! unanchored' services/*/src`). `services/common/src/validation.rs` is the staged home
//! for exactly the three clause rules this module applies, and the envelope's
//! counting/joining is already rendered by the framework
//! (`aws_smithy_schema_server::constraints::envelope`, which cannot be reused here
//! because it words each clause through `Violation::message` with no hook for the
//! redaction and list folding above); `bound` likewise re-derives the framework's private
//! `render_bound`. As that staged module's own header asks, this is the note rather than
//! a silent fourth copy: migrating them is one change of its own — it re-words services
//! whose recorded behaviour their own suites pin, and the framework half is in the
//! read-only vendored tree — so it is not made here under one service's issue. All this
//! module holds that is service-specific is [`SENSITIVE_MEMBERS`] and
//! [`RUNTIME_PARTITION_FORM`], which is what such a move would need from a service.

use std::collections::HashMap;

use ::aws_smithy_schema::{Schema, ShapeType};
use ::aws_smithy_schema_server::constraints::{
    path, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait, Violation,
    ViolationKind,
};

/// The path segment the validating walk writes for the step from a collection into one
/// of its elements: a list element's path is `<member>.<1-based index>.member`.
const COLLECTION_STEP: &str = "member";

/// The clause separator inside the AWS validation envelope.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// The opening of the requirement that reports a `@pattern`; the rest of that clause is
/// the expression itself.
const PATTERN_REQUIREMENT: &str = "Member must satisfy regular expression pattern: ";

/// The ARN partition as the model's patterns spell it: a wildcard that admits any
/// string of lower-case letters and hyphens after `aws`.
const MODEL_PARTITION_WILDCARD: &str = "aws[a-z-]*";

/// The ARN partition as the service reports it: the partition GRAMMAR — `aws` followed
/// by up to three `-<segment>` parts, which is every published partition name
/// (`aws`, `aws-cn`, `aws-us-gov`, `aws-iso`, `aws-iso-b`, …) and nothing else.
const RUNTIME_PARTITION_FORM: &str = "aws(-[a-z]{1,5}){0,3}";

/// The member names this service's model marks `@sensitive`.
///
/// `@sensitive` is a MODEL fact that the generated runtime `Schema` does not carry —
/// codegen captures the constraint traits (`@length`, `@range`, `@pattern`, `@enum`,
/// `@uniqueItems`) and nothing else — so the set is restated here from the model: every
/// member whose target is one of its five sensitive string shapes, `UserName`,
/// `GroupDisplayName`, `SensitiveStringType`, `ExternalIdIssuer` and
/// `ExternalIdIdentifier`
/// (`smithy_query(query="closure",operation="CreateUser")` plus
/// `smithy_query(query="shape",shape="CreateGroupRequest,Filter,ExternalId")`).
///
/// By member NAME, because in this model a name that targets a sensitive shape does so
/// everywhere it appears: the only name shared with a non-sensitive member is
/// `attributeValue` (sensitive on `Filter`, an unconstrained `document` on
/// `UniqueAttribute` and `AttributeOperation`, whose only possible violation is an
/// absent required member — which echoes no value either way).
///
/// A hand-maintained restatement of a model fact drifts when the model changes, so it is
/// not left to review: `tests/all/error_message_rules.rs` derives the same set from the
/// model's own `@sensitive` traits and fails when the two disagree, which is why this is
/// `pub`.
pub const SENSITIVE_MEMBERS: &[&str] = &[
    "attributeValue",
    "birthdate",
    "country",
    "description",
    "display",
    "displayName",
    "familyName",
    "formatted",
    "givenName",
    "honorificPrefix",
    "honorificSuffix",
    "id",
    "issuer",
    "locale",
    "locality",
    "middleName",
    "nickName",
    "postalCode",
    "preferredLanguage",
    "profileUrl",
    "region",
    "streetAddress",
    "timezone",
    "title",
    "type",
    "userName",
    "userType",
    "value",
    "website",
];

/// Every operation input shape of this service, as the schemas codegen derives from the
/// Smithy model — the only runtime source for what a member declares. One entry per
/// operation file in `src/ops/`.
///
/// A hand-maintained restatement of a model fact drifts, and silently: an operation left
/// out of it loses rule 3 for its list members. So it is not left to review either —
/// `tests/all/error_message_rules.rs` counts the operations the model declares and fails
/// when this does not cover them all, which is why it is `pub`.
pub fn input_schemas() -> [&'static Schema; 22] {
    use crate::ops::types;

    [
        types::CREATE_GROUP_INPUT_SCHEMA,
        types::CREATE_GROUP_MEMBERSHIP_INPUT_SCHEMA,
        types::CREATE_USER_INPUT_SCHEMA,
        types::DELETE_GROUP_INPUT_SCHEMA,
        types::DELETE_GROUP_MEMBERSHIP_INPUT_SCHEMA,
        types::DELETE_USER_INPUT_SCHEMA,
        types::DESCRIBE_GROUP_INPUT_SCHEMA,
        types::DESCRIBE_GROUP_MEMBERSHIP_INPUT_SCHEMA,
        types::DESCRIBE_IDENTITY_STORE_INPUT_SCHEMA,
        types::DESCRIBE_USER_INPUT_SCHEMA,
        types::GET_GROUP_ID_INPUT_SCHEMA,
        types::GET_GROUP_MEMBERSHIP_ID_INPUT_SCHEMA,
        types::GET_USER_ID_INPUT_SCHEMA,
        types::IS_MEMBER_IN_GROUPS_INPUT_SCHEMA,
        types::LIST_GROUPS_INPUT_SCHEMA,
        types::LIST_GROUP_MEMBERSHIPS_FOR_MEMBER_INPUT_SCHEMA,
        types::LIST_GROUP_MEMBERSHIPS_INPUT_SCHEMA,
        types::LIST_IDENTITY_STORES_INPUT_SCHEMA,
        types::LIST_USERS_INPUT_SCHEMA,
        types::UPDATE_GROUP_INPUT_SCHEMA,
        types::UPDATE_IDENTITY_STORE_INPUT_SCHEMA,
        types::UPDATE_USER_INPUT_SCHEMA,
    ]
}

/// One clause of the validation envelope, before it is worded.
#[derive(Debug)]
enum Clause {
    /// One violation, reported where the walk found it.
    Single(Violation),
    /// Every violation found on the scalar elements of ONE list member, reported at
    /// that member: `constraints` is the set its element shape declares and `values`
    /// the offending elements, in the order the walk met them. `withheld` is the
    /// framework's own reading of whether the member's values may be shown at all.
    ListMember {
        member: String,
        constraints: Vec<String>,
        values: Vec<String>,
        withheld: bool,
    },
}

/// This service's `ValidationException` message for the violations the boundary
/// validator found, or `None` when it found none.
///
/// The envelope is AWS's: `1 validation error detected: <clause>`, or
/// `N validation errors detected: <clause>; <clause>` — where N counts the CLAUSES,
/// so a list member whose elements all failed is one error, not one per element.
pub fn message(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = group_violations(violations)
        .iter()
        .map(render_clause)
        .collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// Fold the violations into one clause each, except those on the scalar elements of a
/// list member, which fold into one clause for that member.
///
/// Linear in the violations: each one is placed with a single hashed lookup, never a
/// scan of the clauses already accumulated, so the cost is not the product of two
/// request-derived factors — and the element constraint set, the one step that reads the
/// static schemas, is resolved once per list member rather than once per violation. The
/// validating walk hands over at most `MAX_VIOLATIONS` (100) violations per request (it
/// stops accumulating there), so the whole rendering is bounded by that cap however
/// large the body was.
fn group_violations(violations: &[Violation]) -> Vec<Clause> {
    let mut clauses: Vec<Clause> = Vec::new();
    // Which clause each already-seen list member folded into.
    let mut folded: HashMap<String, usize> = HashMap::new();
    for violation in violations {
        let Some(member) = element_list_member(&violation.path) else {
            clauses.push(Clause::Single(violation.clone()));
            continue;
        };
        if let Some(&index) = folded.get(member) {
            if let Some(Clause::ListMember { values, .. }) = clauses.get_mut(index) {
                // An element failing several element constraints arrives as several
                // violations carrying the same value; the clause reports the ELEMENTS
                // that failed, so one is not listed twice.
                if let Some(value) = violation.value.clone() {
                    if !values.contains(&value) {
                        values.push(value);
                    }
                }
            }
            continue;
        }
        // First violation on this member: whether it aggregates at all depends on the
        // element constraint set the model declares, so that is resolved here, once.
        let Some(constraints) = element_constraint_clauses(member) else {
            clauses.push(Clause::Single(violation.clone()));
            continue;
        };
        folded.insert(member.to_string(), clauses.len());
        clauses.push(Clause::ListMember {
            member: member.to_string(),
            constraints,
            values: violation.value.clone().into_iter().collect(),
            withheld: violation.sensitive,
        });
    }
    clauses
}

/// The clause's wording.
fn render_clause(clause: &Clause) -> String {
    match clause {
        Clause::Single(violation) => format!(
            "{} at '{}'{CONSTRAINT_SEPARATOR}{}",
            value_fragment(&violation.path, violation.value.as_deref(), violation.sensitive),
            violation.path,
            requirement_of(violation)
        ),
        Clause::ListMember {
            member,
            constraints,
            values,
            withheld,
        } => {
            // The value a list member failed on is the collection: the offending
            // elements, bracketed, so the clause names the member the constraint is
            // reported at and still shows what was in it. One element that failed
            // several of the element constraints is still one element.
            let shown = (!values.is_empty()).then(|| format!("[{}]", values.join(", ")));
            format!(
                "{} at '{member}'{CONSTRAINT_SEPARATOR}Member must satisfy constraint: [{}]",
                value_fragment(member, shown.as_deref(), *withheld),
                constraints.join(", ")
            )
        }
    }
}

/// How the clause opens: `Value '<v>'` for a value it may show, `Value` for one the
/// model marks sensitive, and `Value null` when the member was absent — absence IS
/// what that clause states, so it keeps saying so.
///
/// `withheld` is the violation's own answer to "was this value withheld": the validating
/// walk drops a `@sensitive` member's value as it records the violation, so an empty
/// value half is not by itself evidence that the member was absent. [`is_sensitive`] is
/// the same reading off the PATH, for a violation this service built itself and the
/// framework never walked.
fn value_fragment(path: &str, value: Option<&str>, withheld: bool) -> String {
    match value {
        // A sensitive member's value reaches here as `None`, so the two are told apart by
        // the flag rather than by the absence: only an absent member states `null`.
        None if withheld => "Value".to_string(),
        None => "Value null".to_string(),
        Some(_) if is_sensitive(path) => "Value".to_string(),
        Some(shown) => format!("Value '{shown}'"),
    }
}

/// Whether the member at `path` is one the model marks `@sensitive`.
///
/// The member a clause is about is the last segment of the path that is neither the
/// collection step nor a list index, so an element of a sensitive list member counts as
/// that member and a value inside a struct element counts as its own member.
fn is_sensitive(path: &str) -> bool {
    let member = path
        .rsplit('.')
        .find(|segment| *segment != COLLECTION_STEP && segment.parse::<usize>().is_err())
        .unwrap_or(path);
    SENSITIVE_MEMBERS.contains(&member)
}

/// The requirement half of a clause: what the value had to satisfy.
///
/// Rendered from the violation's KIND alone — through the framework's own wording, so a
/// constraint class it learns to check cannot drift from this one — with the reported
/// pattern put into the form the service applies ([`runtime_pattern`]).
fn requirement_of(violation: &Violation) -> String {
    requirement_text(violation.kind.clone())
}

/// The requirement a constraint class states.
///
/// The framework renders a clause, not a requirement, so the requirement is taken from
/// a rendering of `kind` with NO value and an EMPTY path: every word of that sentence is
/// then the framework's own or the model's, and none of it is anything a caller sent.
/// Rendering the violation as it arrived and cutting the sentence up would be a defect:
/// a value carrying the separator would splice itself into the requirement — and on a
/// `@sensitive` member that is exactly the value this renderer withholds.
fn requirement_text(kind: ViolationKind) -> String {
    let rendered = Violation {
        path: String::new(),
        kind,
        value: None,
        sensitive: false,
    }
    .message();
    let requirement = rendered
        .split_once(CONSTRAINT_SEPARATOR)
        .map_or(rendered.as_str(), |(_, requirement)| requirement);
    match requirement.strip_prefix(PATTERN_REQUIREMENT) {
        Some(pattern) => format!("{PATTERN_REQUIREMENT}{}", runtime_pattern(pattern)),
        None => requirement.to_string(),
    }
}

/// A `@pattern` as the service reports it: the expression it APPLIES.
///
/// Smithy pattern semantics are a find, so the model's `^`/`$` anchors are not part of
/// the expression being applied and are not reported; an ARN pattern reports the
/// partition grammar rather than the model's loose wildcard, so a caller that validates
/// input locally with the reported expression admits exactly the partitions the service
/// does.
///
/// REPORTED, not ENFORCED: the validating walk keeps applying the model's own trait, so
/// the partition form reported here is the narrower of the two — the emulator still
/// accepts a partition string the model's wildcard admits and this expression does not.
/// The reported form is what a caller is told; do not read it as what was checked.
fn runtime_pattern(pattern: &str) -> String {
    let unanchored = pattern.strip_prefix('^').unwrap_or(pattern);
    // Only an ANCHOR is dropped: a trailing `\$` is an escaped dollar sign the
    // expression matches literally, so that `$` stays. The run of backslashes before it
    // decides — an odd number escapes it, an even number (including none) leaves it the
    // anchor.
    let unanchored = match unanchored.strip_suffix('$') {
        Some(body) if body.chars().rev().take_while(|c| *c == '\\').count() % 2 == 0 => body,
        _ => unanchored,
    };
    unanchored.replace(MODEL_PARTITION_WILDCARD, RUNTIME_PARTITION_FORM)
}

/// The top-level list member an element-level violation belongs to, or `None` when the
/// violation is not one on a scalar element of such a member.
///
/// A violation inside a STRUCT element keeps its own deeper path
/// (`<list>.<n>.member.<member>`) and so is not matched here: the constraint it failed is
/// that member's, not the list's. Only a TOP-LEVEL list member can be resolved against
/// an operation's input schema either, because a member schema carries its own traits and
/// not the members of the shape it targets, so a list nested inside a structure keeps its
/// per-element clause.
fn element_list_member(path: &str) -> Option<&str> {
    let element = path.strip_suffix(COLLECTION_STEP)?.strip_suffix('.')?;
    let (member, index) = element.rsplit_once('.')?;
    index.parse::<usize>().ok()?;
    (!member.contains('.')).then_some(member)
}

/// Every constraint the element shape of the top-level list member `name` declares,
/// worded as the clauses the composite requirement lists, or `None` when this service's
/// model does not describe one set for it.
///
/// Read off the model-derived schema: codegen attaches a collection member's element
/// constraints to that member as an `ElementConstraintsTrait`, which is where the
/// boundary validator reads them too, so this cannot state a bound the validator does
/// not enforce. The order is the one the service reports: the length maximum, the length
/// minimum, the numeric bounds, the pattern, the value set, and last that an element
/// must not be null — which every element shape requires and no trait records.
///
/// Resolved by member NAME across every operation input, because `override_refusal`
/// sees the violations and not the operation they came from (`Refusal::Invalid` carries
/// no schema). So a name that two inputs declare with DIFFERENT element constraints is
/// refused here rather than guessed at: the violation then keeps its own per-element
/// clause, which is never wrong, instead of a composite that could state a bound this
/// operation does not enforce. Today no name in this model is declared twice with
/// different element constraints, and nothing silently relies on that.
fn element_constraint_clauses(name: &str) -> Option<Vec<String>> {
    let mut resolved: Option<Vec<String>> = None;
    for input in input_schemas() {
        let member = input.members().iter().find(|member| {
            matches!(member.shape_type(), ShapeType::List)
                && member
                    .member_name()
                    .is_some_and(|member_name| path::member(member_name) == name)
        });
        let Some(traits) = member
            .and_then(|member| member.traits())
            .and_then(|traits| traits.get(&ElementConstraintsTrait::TRAIT_ID))
            .and_then(|wrapper| wrapper.as_any().downcast_ref::<ElementConstraintsTrait>())
            .map(ElementConstraintsTrait::inner)
        else {
            continue;
        };

        let mut clauses = Vec::new();
        if let Some(length) = traits
            .get(&LengthTrait::TRAIT_ID)
            .and_then(|found| found.as_any().downcast_ref::<LengthTrait>())
        {
            if let Some(max) = length.max {
                clauses.push(requirement_text(ViolationKind::MaxLength(max)));
            }
            if let Some(min) = length.min {
                clauses.push(requirement_text(ViolationKind::MinLength(min)));
            }
        }
        if let Some(range) = traits
            .get(&RangeTrait::TRAIT_ID)
            .and_then(|found| found.as_any().downcast_ref::<RangeTrait>())
        {
            if let Some(max) = range.max {
                clauses.push(requirement_text(ViolationKind::MaxValue(bound(max))));
            }
            if let Some(min) = range.min {
                clauses.push(requirement_text(ViolationKind::MinValue(bound(min))));
            }
        }
        if let Some(pattern) = traits
            .get(&PatternTrait::TRAIT_ID)
            .and_then(|found| found.as_any().downcast_ref::<PatternTrait>())
        {
            clauses.push(requirement_text(ViolationKind::Pattern(
                pattern.regex.to_string(),
            )));
        }
        if let Some(values) = traits
            .get(&EnumTrait::TRAIT_ID)
            .and_then(|found| found.as_any().downcast_ref::<EnumTrait>())
        {
            clauses.push(requirement_text(ViolationKind::EnumValue(
                values
                    .values
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
            )));
        }
        // An element shape whose only constraint is one no trait here describes
        // (`@uniqueItems`, say, which bounds the LIST and not its elements) has no
        // element constraint set to compose, so its violations keep their own clause.
        if clauses.is_empty() {
            continue;
        }
        clauses.push(requirement_text(ViolationKind::MissingRequiredMember));
        match &resolved {
            // Two inputs declaring this name with different element constraints: which
            // one is being validated is not knowable here, so neither is reported.
            Some(first) if *first != clauses => return None,
            Some(_) => {}
            None => resolved = Some(clauses),
        }
    }
    resolved
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

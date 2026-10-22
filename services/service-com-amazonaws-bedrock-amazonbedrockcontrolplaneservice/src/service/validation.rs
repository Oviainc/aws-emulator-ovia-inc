// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-constraint refusal: how the structured
//! [`Violation`]s the framework's boundary walk produces become the `message` the
//! modelled `ValidationException` (HTTP 400) carries.
//!
//! [`crate::service`] owns the SEAM (`Service::override_refusal`) and this module owns the
//! sentence, so the clause rules live in one place rather than in the seam that applies
//! them. WHAT is enforced is always the model's: every violation handed to [`envelope`]
//! comes from `aws_smithy_schema_server::constraints`, which reads the `@length`,
//! `@range`, `@pattern`, `@enum` and `@uniqueItems` traits codegen captured off the model.
//! Only the WORDS are chosen here — and only the ones the seam does not word itself: an
//! ARN this service checks ITSELF keeps that service-own sentence (`crate::service`), which
//! is chosen ahead of this envelope.
//!
//! The rules, each holding for every constrained member of every operation rather than for
//! the members one report happened to cite:
//!
//! 1. the envelope is AWS's `N validation error(s) detected: ` over one clause per
//!    offending member, joined with `; `, in the order the walk found them
//!    ([`envelope`]);
//! 2. a clause states the value the member held, its dotted path and the requirement it
//!    failed;
//! 3. a reported `@pattern` is the expression WITHOUT the `^` / `$` anchors the model
//!    spells it with ([`unanchored`]): the advertised constraint is the character grammar
//!    a value must satisfy, and a caller that lifts the expression out of the message to
//!    re-validate locally must get the expression the service publishes, not the model's
//!    way of making the match total;
//! 4. a violation on an ELEMENT of a collection is reported against the COLLECTION MEMBER
//!    — no element index and no `member` step — with the ELEMENT SHAPE'S WHOLE CONSTRAINT
//!    SET nested inside `Member must satisfy constraint: [ … ]`, and the collection
//!    rendered as the value ([`collapsed`], [`element_constraint_set`]). The set is the
//!    element's, not just the constraint that happened to fail: the clause tells a caller
//!    what every element of that member has to satisfy. A violation on the collection
//!    ITSELF (its own `@length`, its `@uniqueItems`) is not nested, and neither is one on
//!    a member INSIDE a structure element, which keeps its own full path.
//!
//! Rule 4's constraint set is read off the MODEL, not off the refusal: the structured
//! refusal carries only the constraints that failed, so the ones a value satisfied would
//! otherwise be missing from the clause. Codegen attaches a collection member's element
//! constraints to that member's schema as an
//! [`ElementConstraintsTrait`](aws_smithy_schema_server::constraints::ElementConstraintsTrait),
//! and [`ELEMENT_CONSTRAINTS`] indexes them for every input shape of this service.
//!
//! What the refusal does NOT carry is the value a collection member held — only the
//! elements that offended, each already elided to the framework's echo cap — so the VALUE a
//! collection clause states is the one formed from those elements. That divergence, and why
//! closing it belongs elsewhere, is recorded on [`Clause::render`].
//!
//! A violation class this module has no wording for falls back to the FRAMEWORK's own
//! clause for that one violation ([`ViolationKind`] is `#[non_exhaustive]`, so a class can
//! be added upstream), which keeps the rest of the refusal worded here.
//!
//! MIGRATION NOTE (`services/common/src/validation.rs`): rules 1-3 are AWS's wording
//! rather than this service's, and ~24 sibling crates carry their own copy of parts of
//! them (`grep -rln 'fn unanchored' services/*/src`). That module is the staged home for
//! exactly those clause rules; as its header asks, this is the note rather than a silent
//! further copy — migrating them re-words services whose recorded behaviour their own
//! suites pin, so it is one change of its own. What would stay here is rule 4's
//! model-derived element constraint set.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use aws_smithy_schema::{Schema, ShapeId, TraitMap};
use aws_smithy_schema_server::constraints::{
    self, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait,
};
use smithy_server_runtime::{Violation, ViolationKind};

/// The segment the framework's validation path carries for the step from a collection into
/// one of its elements: `constraints::path::element` spells an element as its 1-based
/// position followed by this segment (`jobIdentifiers.1.member`).
const COLLECTION_STEP: &str = ".member";

/// The clause a collection states around its element's constraint set.
const ELEMENT_SET_OPEN: &str = "Member must satisfy constraint: [";

/// The largest number of clauses one refusal RENDERS.
///
/// The violation count is REQUEST-SHAPED — a caller chooses how many members and how many
/// elements of a collection to make invalid — so the size of the refusal is bounded here
/// rather than by the request. Past this many distinct clauses the rest are not spelled
/// out (a caller fixing a request works from the first ones anyway), but they are still
/// COUNTED: the `N` the envelope states is the real number of distinct clauses.
const MAX_CLAUSES: usize = 20;

/// The largest number of element values one collection clause renders, for the same
/// reason: a collection of N invalid elements must not turn into an N-element string in
/// the response.
const MAX_RENDERED_ELEMENTS: usize = 10;

/// The deepest collection nesting one clause states, for the same reason again.
///
/// A step out of a collection is spelled `<position-or-key>.member`, and a MAP entry's
/// step carries the key the CALLER chose, so a key spelling `.<digits>.member` is
/// indistinguishable from a list index by the path alone: without a ceiling the nesting a
/// request can ask for would grow with the length of that key. The inputs of this model
/// nest one collection deep (`jobIdentifiers`, `tags`, `tagKeys`, the identifier lists),
/// so the ceiling is one past that — enough to state a list of lists, were the model to
/// grow one, and no more.
const MAX_DEPTH: usize = 2;

// =============================================================================
// Requirement wording
// =============================================================================

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

/// The `@pattern`, as a requirement reads: the expression UNANCHORED (rule 3).
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
/// fractional value with its decimals — the same rendering the framework's own clause
/// uses, so a bound read off the model here and one read off a violation agree.
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

/// The requirement a violation states, or `None` for a class this module has no wording
/// for.
fn requirement(kind: &ViolationKind) -> Option<String> {
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

/// A `@pattern` expression without the anchors a model may spell it with.
///
/// A `$` preceded by an ODD number of backslashes is an escaped dollar sign — a literal
/// character of the expression, not the end anchor — so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let escapes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if escapes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

// =============================================================================
// The element constraint sets this model declares
// =============================================================================

/// One constraint trait off a trait map, downcast to the type that carries its bounds.
fn constraint_trait<'map, T: 'static>(traits: &'map TraitMap, id: &ShapeId) -> Option<&'map T> {
    traits.get(id)?.as_any().downcast_ref::<T>()
}

/// Every requirement an element shape declares, in the order a clause states them.
///
/// The ORDER is the one the service's recorded refusals show, in both fixtures that carry
/// an element-constraint clause (`bedrock-advanced-prompt-optimization-jobs` and
/// `bedrock-evaluation-job-lifecycle` both state
/// `[… length less than or equal to 1011, … length greater than or equal to 0, … pattern: …]`):
/// the `@length` maximum, then its minimum, then the numeric bounds the same way, then the
/// `@pattern`, then an enumerated value set. A `@length` with no declared minimum states
/// `0`, which is the smallest length a Smithy `@length` can bound and what those
/// recordings show for a `@length(max:)`-only element shape.
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

/// The element constraint sets this service's input shapes declare, keyed by the member
/// path the validation walk reports that collection member under.
///
/// Built once, from the generated schemas — so what a clause states is the model's, and a
/// member whose element constraints change in the model changes here with it. A path can
/// hold SEVERAL sets, because two operations may name a member the same way over different
/// element shapes (`jobIdentifiers` is an evaluation-job identifier list in one operation
/// and an advanced-prompt-optimization-job identifier list in another); which one a clause
/// states is decided by the constraint that actually failed (see
/// [`element_constraint_set`]).
static ELEMENT_CONSTRAINTS: LazyLock<HashMap<String, Vec<Vec<String>>>> = LazyLock::new(|| {
    let mut catalog: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for schema in input_schemas() {
        for member in schema.members() {
            let Some(name) = member.member_name() else {
                continue;
            };
            let Some(traits) = member.traits() else {
                continue;
            };
            let Some(wrapper) = constraint_trait::<ElementConstraintsTrait>(
                traits,
                &ElementConstraintsTrait::TRAIT_ID,
            ) else {
                continue;
            };
            let requirements = element_requirements(wrapper.inner());
            if requirements.is_empty() {
                continue;
            }
            let sets = catalog.entry(constraints::path::member(name)).or_default();
            if !sets.contains(&requirements) {
                sets.push(requirements);
            }
        }
    }
    catalog
});

/// The element constraint set a clause at `path` states, given every requirement the
/// walk reported `path`'s elements as having broken.
///
/// Those failures are what identify the element shape when a path is declared by several
/// operations: a set that does not contain all of them is not the set this request broke.
/// They are taken TOGETHER — one bound a request broke may be declared by several element
/// shapes while the combination is not — which is why the set is resolved once per member
/// rather than once per violation. When no set is found, or several candidate sets contain
/// them all, the clause states the broken requirements alone: those are the thing known to
/// be true of the request, where a guessed set would not be.
fn element_constraint_set(path: &str, failing: &[String]) -> Vec<String> {
    let alone = || failing.to_vec();
    let Some(sets) = ELEMENT_CONSTRAINTS.get(path) else {
        return alone();
    };
    let mut matching = sets
        .iter()
        .filter(|set| failing.iter().all(|broken| set.contains(broken)));
    let Some(found) = matching.next() else {
        return alone();
    };
    if matching.any(|other| other != found) {
        return alone();
    }
    found.clone()
}

// =============================================================================
// Clause assembly
// =============================================================================

/// The collection a violation's path points INTO, when the violation is on one of that
/// collection's elements.
///
/// An element path ends with the collection step preceded by the element's 1-based position
/// (`jobIdentifiers.1.member` -> `jobIdentifiers`). The position is what identifies the
/// step, so a member that happens to be NAMED `member` (`filters.1.member.member`) is not
/// mistaken for one.
///
/// A MAP entry is spelled the same way over the key the CALLER chose
/// (`constraints::path::entry`), so an all-digit key is indistinguishable from a list index
/// by the path alone and an entry under such a key is worded as a list element would be.
/// The path and the constraint set stay the member's either way; only the brackets around
/// the echoed value read as a list's. Telling the two apart needs the member's shape, which
/// a path does not carry.
fn enclosing_collection(path: &str) -> Option<&str> {
    let (head, position) = path.strip_suffix(COLLECTION_STEP)?.rsplit_once('.')?;
    if position.is_empty() || !position.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(head)
}

/// `path` with its collection steps walked out of: the member a clause is reported at, and
/// how deep the nesting is.
///
/// At most [`MAX_DEPTH`] steps are walked, and the path and the depth are clamped TOGETHER
/// so the member named and the nesting stated always agree. The ceiling is there because a
/// path's entry steps may carry a caller-chosen map key, so without one a key spelling
/// `.<digits>.member` would let the request choose how much nesting the clause renders.
fn collapsed(path: &str) -> (&str, usize) {
    let mut path = path;
    let mut depth = 0;
    while depth < MAX_DEPTH {
        match enclosing_collection(path) {
            Some(collection) => {
                path = collection;
                depth += 1;
            }
            None => break,
        }
    }
    (path, depth)
}

/// `requirement` wrapped in the collection-constraint clause once per collection step,
/// built in ONE allocation rather than re-formatting the string per step.
fn nested(requirement: &str, depth: usize) -> String {
    if depth == 0 {
        return requirement.to_string();
    }
    let mut nested =
        String::with_capacity(requirement.len() + depth * (ELEMENT_SET_OPEN.len() + 1));
    for _ in 0..depth {
        nested.push_str(ELEMENT_SET_OPEN);
    }
    nested.push_str(requirement);
    for _ in 0..depth {
        nested.push(']');
    }
    nested
}

/// One clause of the envelope, accumulated as the violations are walked.
///
/// A clause is keyed by the member path it is reported at and the requirement it states,
/// so every element of ONE collection accumulates into a single clause — and so do two
/// constraints of one element shape, because both state that shape's whole constraint set.
#[derive(Debug)]
struct Clause {
    /// The member path the clause is reported at: the collection, for an element violation.
    path: String,
    /// The requirement, already nested once per collection step walked out of.
    requirement: String,
    /// How many collection steps were walked out of: 0 for a member reported on its own.
    depth: usize,
    /// The offending values in walk order, each with the violation PATH it was found at,
    /// capped at [`MAX_RENDERED_ELEMENTS`]. The path is what keeps ONE element that broke
    /// two of its shape's constraints from being rendered twice, while two different
    /// elements — which have different paths, at any nesting depth — are both rendered.
    values: Vec<(String, String)>,
    /// The framework's own clause, kept verbatim for a violation class this module has no
    /// wording for, so one such class does not cost the refusal its other clauses.
    verbatim: Option<String>,
}

impl Clause {
    /// The clause as the envelope states it.
    ///
    /// KNOWN DIVERGENCE, recorded rather than papered over, in the VALUE a collection
    /// clause echoes. Two limits come from what the structured refusal carries:
    ///
    /// * it carries the OFFENDING elements, not the collection they came from, so a
    ///   collection whose elements are only partly invalid is echoed as the collection
    ///   formed from the invalid ones (`["<valid>", "bad"]` -> `Value '[bad]'`) where AWS
    ///   echoes the whole list. The member path and the constraint set — what tells a
    ///   caller WHICH member is wrong and what it must satisfy — are exact either way;
    /// * `Violation::value` is the framework's ELIDED echo, so a value past its cap carries
    ///   the `… [N bytes]` marker.
    ///
    /// Both are fixed in the same place, and it is not this seam: the refusal would have to
    /// carry the value the caller submitted (`services/common/src/validation.rs`
    /// `with_submitted_values` restores it from the deserialized input, which only an
    /// operation-level seam holds). Inventing the missing elements here would echo bytes the
    /// caller never sent, which is worse than echoing fewer of them.
    fn render(&self) -> String {
        if let Some(verbatim) = &self.verbatim {
            return verbatim.clone();
        }
        // No value at all: an absent member, reported as the `null` it was. Read off what
        // the clause COLLECTED rather than off the violation that opened it, so a clause
        // that did collect a value always echoes it.
        let subject = if self.values.is_empty() {
            "Value null".to_string()
        } else if self.depth == 0 {
            format!(
                "Value '{}'",
                self.values
                    .first()
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_default()
            )
        } else {
            // One bracket per collection step, over the elements the refusal named (see the
            // divergence noted above).
            let values: Vec<&str> = self
                .values
                .iter()
                .map(|(_, value)| value.as_str())
                .collect();
            format!(
                "Value '{}{}{}'",
                "[".repeat(self.depth),
                values.join(", "),
                "]".repeat(self.depth)
            )
        };
        format!(
            "{subject} at '{}' failed to satisfy constraint: {}",
            self.path, self.requirement
        )
    }
}

/// The `ValidationException` message for a refused request: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per offending member, in the order
/// the walk found them.
///
/// TWO passes over the violations, so the cost stays linear in their number: the first
/// resolves each collection member's element constraint set, and the second groups every
/// violation into the clause it belongs to (its member path and the requirement it states),
/// which is what collapses the elements of one collection onto one clause. The COUNT the
/// envelope states is the real number of distinct clauses the request produced; what is
/// bounded is how much of it is RENDERED ([`MAX_CLAUSES`] clauses,
/// [`MAX_RENDERED_ELEMENTS`] element values each), because the violation count is chosen by
/// the request.
///
/// `None` only when there is nothing to state.
pub(crate) fn envelope(violations: &[Violation]) -> Option<String> {
    // The element constraint set a collection member's clause states, resolved ONCE per
    // member from every requirement its elements broke (see [`element_constraint_set`]):
    // every element violation of that member then states the same requirement, which is
    // what folds them onto one clause.
    let mut broken: HashMap<&str, Vec<String>> = HashMap::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        if depth == 0 {
            continue;
        }
        if let Some(failing) = requirement(&violation.kind) {
            let failures = broken.entry(path).or_default();
            if !failures.contains(&failing) {
                failures.push(failing);
            }
        }
    }
    let element_sets: HashMap<&str, String> = broken
        .iter()
        .map(|(path, failures)| (*path, element_constraint_set(path, failures).join(", ")))
        .collect();

    let mut order: Vec<(String, String)> = Vec::new();
    let mut clauses: HashMap<(String, String), Clause> = HashMap::new();
    // Every distinct clause the request produced, including the ones past the render cap:
    // the count AWS states is a fact about the request, so it is not truncated.
    let mut distinct: HashSet<(String, String)> = HashSet::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        let (requirement, verbatim) = match requirement(&violation.kind) {
            // An element violation states the element shape's WHOLE constraint set, nested
            // once per collection step; a violation on the member itself states what it
            // failed.
            Some(failing) if depth > 0 => (
                nested(element_sets.get(path).unwrap_or(&failing), depth),
                None,
            ),
            Some(failing) => (failing, None),
            // Nothing to word this class with: keep the framework's own sentence for THIS
            // clause, and leave the others as this module words them.
            None => (violation.message(), Some(violation.message())),
        };
        let key = (path.to_string(), requirement.clone());
        distinct.insert(key.clone());
        let clause = match clauses.get_mut(&key) {
            Some(clause) => clause,
            None => {
                if order.len() >= MAX_CLAUSES {
                    continue;
                }
                order.push(key.clone());
                clauses.entry(key).or_insert(Clause {
                    path: path.to_string(),
                    requirement,
                    depth,
                    values: Vec::new(),
                    verbatim,
                })
            }
        };
        if let Some(value) = violation.value.as_deref() {
            // One rendered value per violation PATH: the same element breaking two of its
            // shape's constraints is one value, two elements are two.
            let held = clause
                .values
                .iter()
                .any(|(seen, _)| *seen == violation.path);
            if !held && clause.values.len() < MAX_RENDERED_ELEMENTS {
                clause
                    .values
                    .push((violation.path.clone(), value.to_string()));
            }
        }
    }
    let rendered: Vec<String> = order
        .iter()
        .filter_map(|key| clauses.get(key))
        .map(Clause::render)
        .collect();
    match (distinct.len(), rendered.is_empty()) {
        (_, true) => None,
        (1, _) => Some(format!("1 validation error detected: {}", rendered[0])),
        (n, _) => Some(format!(
            "{n} validation errors detected: {}",
            rendered.join("; ")
        )),
    }
}

/// Every INPUT shape of this service, as the generated schema statics.
///
/// All of them, not the ones a report happened to cite: a constraint is a property of an
/// input shape, so leaving an operation out would silently word its collection clauses with
/// less than the model declares. Regenerate with
/// `grep -h 'const INPUT_SCHEMA' src/ops/*.rs`, which names exactly these statics; an input
/// shape missing from this list is not a wrong clause, only a clause that states the
/// constraint that failed rather than the element shape's whole set.
fn input_schemas() -> Vec<&'static Schema> {
    use crate::ops::types as t;
    vec![
        t::BATCH_DELETE_ADVANCED_PROMPT_OPTIMIZATION_JOB_INPUT_SCHEMA,
        t::BATCH_DELETE_EVALUATION_JOB_INPUT_SCHEMA,
        t::CANCEL_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_INPUT_SCHEMA,
        t::CREATE_ADVANCED_PROMPT_OPTIMIZATION_JOB_INPUT_SCHEMA,
        t::CREATE_AUTOMATED_REASONING_POLICY_INPUT_SCHEMA,
        t::CREATE_AUTOMATED_REASONING_POLICY_TEST_CASE_INPUT_SCHEMA,
        t::CREATE_AUTOMATED_REASONING_POLICY_VERSION_INPUT_SCHEMA,
        t::CREATE_CUSTOM_MODEL_DEPLOYMENT_INPUT_SCHEMA,
        t::CREATE_CUSTOM_MODEL_INPUT_SCHEMA,
        t::CREATE_EVALUATION_JOB_INPUT_SCHEMA,
        t::CREATE_FOUNDATION_MODEL_AGREEMENT_INPUT_SCHEMA,
        t::CREATE_GUARDRAIL_INPUT_SCHEMA,
        t::CREATE_GUARDRAIL_VERSION_INPUT_SCHEMA,
        t::CREATE_INFERENCE_PROFILE_INPUT_SCHEMA,
        t::CREATE_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA,
        t::CREATE_MODEL_COPY_JOB_INPUT_SCHEMA,
        t::CREATE_MODEL_CUSTOMIZATION_JOB_INPUT_SCHEMA,
        t::CREATE_MODEL_IMPORT_JOB_INPUT_SCHEMA,
        t::CREATE_MODEL_INVOCATION_JOB_INPUT_SCHEMA,
        t::CREATE_PROMPT_ROUTER_INPUT_SCHEMA,
        t::CREATE_PROVISIONED_MODEL_THROUGHPUT_INPUT_SCHEMA,
        t::DELETE_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_INPUT_SCHEMA,
        t::DELETE_AUTOMATED_REASONING_POLICY_INPUT_SCHEMA,
        t::DELETE_AUTOMATED_REASONING_POLICY_TEST_CASE_INPUT_SCHEMA,
        t::DELETE_CUSTOM_MODEL_DEPLOYMENT_INPUT_SCHEMA,
        t::DELETE_CUSTOM_MODEL_INPUT_SCHEMA,
        t::DELETE_ENFORCED_GUARDRAIL_CONFIGURATION_INPUT_SCHEMA,
        t::DELETE_FOUNDATION_MODEL_AGREEMENT_INPUT_SCHEMA,
        t::DELETE_GUARDRAIL_INPUT_SCHEMA,
        t::DELETE_IMPORTED_MODEL_INPUT_SCHEMA,
        t::DELETE_INFERENCE_PROFILE_INPUT_SCHEMA,
        t::DELETE_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA,
        t::DELETE_MODEL_INVOCATION_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        t::DELETE_PROMPT_ROUTER_INPUT_SCHEMA,
        t::DELETE_PROVISIONED_MODEL_THROUGHPUT_INPUT_SCHEMA,
        t::DELETE_RESOURCE_POLICY_INPUT_SCHEMA,
        t::DEREGISTER_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA,
        t::EXPORT_AUTOMATED_REASONING_POLICY_VERSION_INPUT_SCHEMA,
        t::GET_ACCOUNT_DATA_RETENTION_INPUT_SCHEMA,
        t::GET_ADVANCED_PROMPT_OPTIMIZATION_JOB_INPUT_SCHEMA,
        t::GET_AUTOMATED_REASONING_POLICY_ANNOTATIONS_INPUT_SCHEMA,
        t::GET_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_INPUT_SCHEMA,
        t::GET_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_RESULT_ASSETS_INPUT_SCHEMA,
        t::GET_AUTOMATED_REASONING_POLICY_INPUT_SCHEMA,
        t::GET_AUTOMATED_REASONING_POLICY_NEXT_SCENARIO_INPUT_SCHEMA,
        t::GET_AUTOMATED_REASONING_POLICY_TEST_CASE_INPUT_SCHEMA,
        t::GET_AUTOMATED_REASONING_POLICY_TEST_RESULT_INPUT_SCHEMA,
        t::GET_CUSTOM_MODEL_DEPLOYMENT_INPUT_SCHEMA,
        t::GET_CUSTOM_MODEL_INPUT_SCHEMA,
        t::GET_EVALUATION_JOB_INPUT_SCHEMA,
        t::GET_FOUNDATION_MODEL_AVAILABILITY_INPUT_SCHEMA,
        t::GET_FOUNDATION_MODEL_INPUT_SCHEMA,
        t::GET_GUARDRAIL_INPUT_SCHEMA,
        t::GET_IMPORTED_MODEL_INPUT_SCHEMA,
        t::GET_INFERENCE_PROFILE_INPUT_SCHEMA,
        t::GET_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA,
        t::GET_MODEL_COPY_JOB_INPUT_SCHEMA,
        t::GET_MODEL_CUSTOMIZATION_JOB_INPUT_SCHEMA,
        t::GET_MODEL_IMPORT_JOB_INPUT_SCHEMA,
        t::GET_MODEL_INVOCATION_JOB_INPUT_SCHEMA,
        t::GET_MODEL_INVOCATION_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        t::GET_PROMPT_ROUTER_INPUT_SCHEMA,
        t::GET_PROVISIONED_MODEL_THROUGHPUT_INPUT_SCHEMA,
        t::GET_RESOURCE_POLICY_INPUT_SCHEMA,
        t::GET_USE_CASE_FOR_MODEL_ACCESS_INPUT_SCHEMA,
        t::LIST_ADVANCED_PROMPT_OPTIMIZATION_JOBS_INPUT_SCHEMA,
        t::LIST_AUTOMATED_REASONING_POLICIES_INPUT_SCHEMA,
        t::LIST_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOWS_INPUT_SCHEMA,
        t::LIST_AUTOMATED_REASONING_POLICY_TEST_CASES_INPUT_SCHEMA,
        t::LIST_AUTOMATED_REASONING_POLICY_TEST_RESULTS_INPUT_SCHEMA,
        t::LIST_CUSTOM_MODEL_DEPLOYMENTS_INPUT_SCHEMA,
        t::LIST_CUSTOM_MODELS_INPUT_SCHEMA,
        t::LIST_ENFORCED_GUARDRAILS_CONFIGURATION_INPUT_SCHEMA,
        t::LIST_EVALUATION_JOBS_INPUT_SCHEMA,
        t::LIST_FOUNDATION_MODEL_AGREEMENT_OFFERS_INPUT_SCHEMA,
        t::LIST_FOUNDATION_MODELS_INPUT_SCHEMA,
        t::LIST_GUARDRAILS_INPUT_SCHEMA,
        t::LIST_IMPORTED_MODELS_INPUT_SCHEMA,
        t::LIST_INFERENCE_PROFILES_INPUT_SCHEMA,
        t::LIST_MARKETPLACE_MODEL_ENDPOINTS_INPUT_SCHEMA,
        t::LIST_MODEL_COPY_JOBS_INPUT_SCHEMA,
        t::LIST_MODEL_CUSTOMIZATION_JOBS_INPUT_SCHEMA,
        t::LIST_MODEL_IMPORT_JOBS_INPUT_SCHEMA,
        t::LIST_MODEL_INVOCATION_JOBS_INPUT_SCHEMA,
        t::LIST_PROMPT_ROUTERS_INPUT_SCHEMA,
        t::LIST_PROVISIONED_MODEL_THROUGHPUTS_INPUT_SCHEMA,
        t::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        t::PUT_ACCOUNT_DATA_RETENTION_INPUT_SCHEMA,
        t::PUT_ENFORCED_GUARDRAIL_CONFIGURATION_INPUT_SCHEMA,
        t::PUT_MODEL_INVOCATION_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        t::PUT_RESOURCE_POLICY_INPUT_SCHEMA,
        t::PUT_USE_CASE_FOR_MODEL_ACCESS_INPUT_SCHEMA,
        t::REGISTER_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA,
        t::START_AUTOMATED_REASONING_POLICY_BUILD_WORKFLOW_INPUT_SCHEMA,
        t::START_AUTOMATED_REASONING_POLICY_TEST_WORKFLOW_INPUT_SCHEMA,
        t::STOP_ADVANCED_PROMPT_OPTIMIZATION_JOB_INPUT_SCHEMA,
        t::STOP_EVALUATION_JOB_INPUT_SCHEMA,
        t::STOP_MODEL_CUSTOMIZATION_JOB_INPUT_SCHEMA,
        t::STOP_MODEL_INVOCATION_JOB_INPUT_SCHEMA,
        t::TAG_RESOURCE_INPUT_SCHEMA,
        t::UNTAG_RESOURCE_INPUT_SCHEMA,
        t::UPDATE_AUTOMATED_REASONING_POLICY_ANNOTATIONS_INPUT_SCHEMA,
        t::UPDATE_AUTOMATED_REASONING_POLICY_INPUT_SCHEMA,
        t::UPDATE_AUTOMATED_REASONING_POLICY_TEST_CASE_INPUT_SCHEMA,
        t::UPDATE_CUSTOM_MODEL_DEPLOYMENT_INPUT_SCHEMA,
        t::UPDATE_GUARDRAIL_INPUT_SCHEMA,
        t::UPDATE_MARKETPLACE_MODEL_ENDPOINT_INPUT_SCHEMA,
        t::UPDATE_PROVISIONED_MODEL_THROUGHPUT_INPUT_SCHEMA,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchors are stripped, and only the anchors: a `$` that is part of the
    /// expression stays. The forms this model reaches are covered end-to-end by
    /// `tests/all/validation_message_wording.rs`; these rows pin the branches no input
    /// shape of THIS model produces today, so the rule stays stated for a model that
    /// grows one.
    #[test]
    fn only_the_anchors_are_stripped_from_a_pattern() {
        for (pattern, expected) in [
            (r"^[a-z]+$", "[a-z]+"),
            (r"[a-z]+", "[a-z]+"),
            (r"^[a-z]+", "[a-z]+"),
            (r"[a-z]+$", "[a-z]+"),
            // An escaped dollar is a literal character, not the end anchor.
            (r"^[a-z]+\$", r"[a-z]+\$"),
            // An escaped backslash before the anchor: the anchor is still an anchor.
            (r"^[a-z]+\\$", r"[a-z]+\\"),
            ("^$", ""),
        ] {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// An element violation is reported at the collection member, never at the indexed
    /// element position the walk spells — and the path and the depth are clamped together.
    #[test]
    fn an_element_path_collapses_onto_its_collection() {
        for (path, member, depth) in [
            ("jobIdentifiers.1.member", "jobIdentifiers", 1),
            ("jobIdentifiers.12.member", "jobIdentifiers", 1),
            ("rows.2.member.1.member", "rows", 2),
            // Past the ceiling: the step that would exceed it is not walked, so the member
            // named still matches the nesting stated.
            ("cube.1.member.2.member.3.member", "cube.1.member", MAX_DEPTH),
            // Not an element step: no position before `member`.
            ("filters.member", "filters.member", 0),
            // A member that happens to be named `member`.
            ("filters.1.member.member", "filters.1.member.member", 0),
            ("jobIdentifier", "jobIdentifier", 0),
        ] {
            let actual = collapsed(path);
            assert_eq!(
                actual,
                (member, depth),
                "case {path:?}: expected {:?}, got {actual:?}",
                (member, depth)
            );
        }
    }

    /// Every element constraint set in the catalog is keyed by a member path the walk can
    /// report, and holds the model's whole set rather than one constraint.
    #[test]
    fn the_element_constraint_catalog_is_built_from_the_model() {
        let catalog = &*ELEMENT_CONSTRAINTS;
        assert!(
            !catalog.is_empty(),
            "the model declares collection members with constrained elements, so the \
             catalog must not be empty"
        );
        for (path, sets) in catalog {
            assert!(
                !path.contains(COLLECTION_STEP),
                "case {path:?}: a catalog key is a member path, not an element position"
            );
            for set in sets {
                assert!(
                    !set.is_empty(),
                    "case {path:?}: an empty constraint set must not be recorded"
                );
                for requirement in set {
                    // `^` and `$` INSIDE an expression are ordinary characters (`[^:]+`),
                    // so what is checked is the anchoring: no `^` where the expression
                    // starts and no `$` where it ends.
                    assert!(
                        !requirement.contains("pattern: ^") && !requirement.ends_with('$'),
                        "case {path:?}: a requirement states a pattern unanchored, got \
                         {requirement:?}"
                    );
                }
            }
        }
    }
}

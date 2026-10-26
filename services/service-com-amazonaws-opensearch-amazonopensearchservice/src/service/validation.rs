// SPDX-License-Identifier: CC0-1.0

//! How this service STATES the boundary refusal of a request whose input breaks a
//! constraint the model declares.
//!
//! [`crate::service`] owns the seam (`Service::override_refusal`) and this module owns
//! the sentence. WHAT is enforced is always the model's: every [`Violation`] handed to
//! [`envelope`] comes from `aws_smithy_schema_server::constraints`, which reads the
//! `@length`, `@range`, `@pattern`, `@enum` and `@uniqueItems` traits codegen captured
//! off this service's model. Only how the violations are GROUPED into clauses is
//! decided here, and only for one class of them:
//!
//! * a violation on an ELEMENT of a collection member is stated against the COLLECTION
//!   MEMBER — no element position and no `member` step in the path — with the ELEMENT
//!   SHAPE'S WHOLE constraint set nested inside `Member must satisfy constraint: [ … ]`,
//!   and the collection rendered as the value. The set is the element shape's, not just
//!   the constraint that happened to fail: the clause tells a caller what every element
//!   of that member has to satisfy, so one request that breaks two of an element's
//!   constraints is ONE validation error about one member rather than one error per
//!   constraint at an indexed path. A violation on the collection member ITSELF (its own
//!   `@length`, its `@uniqueItems`) is not nested, and neither is one on a member INSIDE
//!   a structure element, which keeps its own full path;
//! * every OTHER violation keeps the framework's own clause, verbatim — the envelope,
//!   the member paths, the echoed value and every requirement sentence are the
//!   framework's, so nothing about them can drift from the one place that writes them.
//!
//! The element constraint set is read off the MODEL, not off the refusal: the structured
//! refusal carries only the constraints that FAILED, so the ones a value satisfied would
//! otherwise be missing from the clause. Codegen attaches a collection member's element
//! constraints to that member's schema as an
//! [`ElementConstraintsTrait`](aws_smithy_schema_server::constraints::ElementConstraintsTrait),
//! and [`ELEMENT_CONSTRAINTS`] indexes them for every input shape of this service.
//!
//! What the refusal does NOT carry is the value a collection member held — only the
//! elements that offended, each already elided to the framework's echo cap — so the
//! value a collection clause states is the one formed from those elements. That limit,
//! and why closing it does not belong in this seam, is recorded on [`Clause::render`].
//!
//! The SIZE of the message is bounded by the framework's own two ceilings on the walk
//! it comes from — how many violations one request accumulates, and how many bytes of a
//! value one of them echoes — and grouping only ever folds violations together, so a
//! refusal worded here is no larger than the one the framework would have worded.

use std::collections::HashMap;
use std::sync::LazyLock;

use aws_smithy_schema::{Schema, ShapeId, TraitMap};
use aws_smithy_schema_server::constraints::{
    self, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait,
};
use aws_smithy_schema_server::{Violation, ViolationKind};
use smithy_server_runtime::{Refusal, Refused};

/// The error code AWS answers a constraint refusal with on the JSON protocols, and the
/// code the framework's own default picks there — named so a chosen message always
/// travels with its matching code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The segment the framework's validation path carries for the step from a collection
/// into one of its elements: `constraints::path::element` spells an element as its
/// 1-based position followed by this segment (`packageUserList.1.member`).
const COLLECTION_STEP: &str = ".member";

/// The clause a collection member states around its element shape's constraint set.
const ELEMENT_SET_OPEN: &str = "Member must satisfy constraint: [";

/// The text that separates a clause's subject from the requirement it states, in the
/// framework's own sentence — the one place that sentence is written.
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint: ";

/// The deepest collection nesting one clause states.
///
/// A step out of a collection is spelled `<position-or-key>.member`, and a MAP entry's
/// step carries the key the CALLER chose, so a key spelling `.<digits>.member` is
/// indistinguishable from a list position by the path alone: without a ceiling, the
/// nesting a request could ask for would grow with the length of that key. The inputs of
/// this model nest one collection deep (`packageUserList`, `vpcEndpointIds`, the tag and
/// id lists), so the ceiling is one past that — enough to state a list of lists, were
/// the model to grow one, and no more.
const MAX_DEPTH: usize = 2;

// =============================================================================
// Requirement wording
// =============================================================================

/// The requirement sentence of one violation class, as the FRAMEWORK words it.
///
/// Read back out of [`Violation::message`] — the ONE place that sentence is written —
/// rather than restated here, so a requirement this module nests into a collection
/// clause can never be worded differently from the clause a non-collection violation
/// keeps verbatim. `None` for a class whose sentence does not follow that shape, which
/// leaves that violation with the framework's own clause.
fn requirement(kind: &ViolationKind) -> Option<String> {
    let probe = Violation {
        path: String::new(),
        kind: kind.clone(),
        value: None,
    };
    probe
        .message()
        .split_once(CONSTRAINT_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// A `@range` bound as it displays: an integral value without a trailing `.0`, a
/// fractional value with its decimals — the rendering the framework's own clause uses,
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

// =============================================================================
// The element constraint sets this model declares
// =============================================================================

/// One constraint trait off a trait map, downcast to the type that carries its bounds.
fn constraint_trait<'map, T: 'static>(traits: &'map TraitMap, id: &ShapeId) -> Option<&'map T> {
    traits.get(id)?.as_any().downcast_ref::<T>()
}

/// Every requirement an element shape declares, in the order a clause states them.
///
/// The ORDER is the one the service's recorded refusals show: the `@length` maximum,
/// then its minimum, then the numeric bounds the same way, then the `@pattern`, then an
/// enumerated value set. A `@length` with no declared minimum states `0`, the smallest
/// length a Smithy `@length` can bound.
///
/// Each sentence comes from [`requirement`], so it is the framework's own wording for
/// that bound rather than a second spelling of it.
///
/// Empty when the element shape declares no constraint at all.
fn element_requirements(traits: &TraitMap) -> Vec<String> {
    let mut kinds: Vec<ViolationKind> = Vec::new();
    if let Some(length) = constraint_trait::<LengthTrait>(traits, &LengthTrait::TRAIT_ID) {
        if let Some(max) = length.max {
            kinds.push(ViolationKind::MaxLength(max));
        }
        kinds.push(ViolationKind::MinLength(length.min.unwrap_or(0)));
    }
    if let Some(range) = constraint_trait::<RangeTrait>(traits, &RangeTrait::TRAIT_ID) {
        if let Some(max) = range.max {
            kinds.push(ViolationKind::MaxValue(render_bound(max)));
        }
        if let Some(min) = range.min {
            kinds.push(ViolationKind::MinValue(render_bound(min)));
        }
    }
    if let Some(pattern) = constraint_trait::<PatternTrait>(traits, &PatternTrait::TRAIT_ID) {
        kinds.push(ViolationKind::Pattern(pattern.regex.to_string()));
    }
    if let Some(allowed) = constraint_trait::<EnumTrait>(traits, &EnumTrait::TRAIT_ID) {
        kinds.push(ViolationKind::EnumValue(
            allowed.values.iter().map(|value| (*value).to_string()).collect(),
        ));
    }
    kinds.iter().filter_map(requirement).collect()
}

/// The element constraint sets this service's input shapes declare, keyed by the member
/// path the validation walk reports that collection member under.
///
/// Built once, from the generated schemas — so what a clause states is the model's, and a
/// member whose element constraints change in the model changes here with it. A path can
/// hold SEVERAL sets, because two operations may name a member the same way over
/// different element shapes; which one a clause states is decided by the constraints that
/// actually failed (see [`element_constraint_set`]).
///
/// Only the members of the INPUT shapes themselves are indexed, so a collection nested
/// inside a structure member is reported against its collection member with the
/// constraints the request broke rather than with the element shape's whole set.
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

/// The element constraint set a clause at `path` states, given every requirement the walk
/// reported `path`'s elements as having broken.
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
/// An element path ends with the collection step preceded by the element's 1-based
/// position (`packageUserList.1.member` -> `packageUserList`). The position is what
/// identifies the step, so a member that happens to be NAMED `member`
/// (`filters.1.member.member`) is not mistaken for one.
///
/// A MAP entry is spelled the same way over the key the CALLER chose
/// (`constraints::path::entry`), so an all-digit key is indistinguishable from a list
/// position by the path alone, and an entry under such a key is worded as a list element
/// would be. The member named and the constraint set stated stay the member's either way.
fn enclosing_collection(path: &str) -> Option<&str> {
    let (head, position) = path.strip_suffix(COLLECTION_STEP)?.rsplit_once('.')?;
    if position.is_empty() || !position.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(head)
}

/// `path` with its collection steps walked out of: the member a clause is reported at,
/// and how deep the nesting is.
///
/// At most [`MAX_DEPTH`] steps are walked, and the path and the depth are clamped
/// TOGETHER so the member named and the nesting stated always agree.
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
#[derive(Debug)]
enum Clause {
    /// The framework's own clause for one violation, kept verbatim: everything that is
    /// not a violation on a collection's element.
    Verbatim(String),
    /// One collection member's clause: the member it is reported at, the element shape's
    /// constraint set nested once per collection step, and the offending elements in walk
    /// order, each with the violation PATH it was found at — the path is what keeps ONE
    /// element that broke two of its shape's constraints from being rendered twice, while
    /// two different elements are both rendered.
    Collection {
        path: String,
        requirement: String,
        depth: usize,
        values: Vec<(String, String)>,
    },
}

impl Clause {
    /// The clause as the envelope states it.
    ///
    /// KNOWN LIMIT, recorded rather than papered over, in the VALUE a collection clause
    /// echoes: the structured refusal carries the OFFENDING elements, not the collection
    /// they came from, so a collection whose elements are only partly invalid is echoed
    /// as the collection formed from the invalid ones. The member path and the constraint
    /// set — what tells a caller WHICH member is wrong and what it must satisfy — are
    /// exact either way. Closing it is not this seam's to do: the refusal would have to
    /// carry the value the caller submitted, which only an operation-level seam holds.
    /// Inventing the missing elements here would echo bytes the caller never sent.
    fn render(&self) -> String {
        match self {
            Self::Verbatim(clause) => clause.clone(),
            Self::Collection {
                path,
                requirement,
                depth,
                values,
            } => {
                let values: Vec<&str> = values.iter().map(|(_, value)| value.as_str()).collect();
                let subject = if values.is_empty() {
                    "Value null".to_string()
                } else {
                    // One bracket per collection step, over the elements the refusal
                    // named (see the limit noted above).
                    format!(
                        "Value '{}{}{}'",
                        "[".repeat(*depth),
                        values.join(", "),
                        "]".repeat(*depth)
                    )
                };
                format!("{subject} at '{path}' {CONSTRAINT_MARKER}{requirement}")
            }
        }
    }
}

/// The `ValidationException` message for a refused request: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per offending member, in the
/// order the walk found them.
///
/// TWO passes over the violations, so the cost stays linear in their number: the first
/// resolves each collection member's element constraint set, and the second groups every
/// violation into the clause it belongs to, which is what collapses the elements of one
/// collection onto one clause. The count the envelope states is the number of clauses,
/// which for everything but a collection is the number of violations — exactly what the
/// framework's own envelope states.
///
/// `None` only when there is nothing to state.
fn envelope(violations: &[Violation]) -> Option<String> {
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

    let mut order: Vec<String> = Vec::new();
    let mut clauses: HashMap<String, Clause> = HashMap::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        let collection = (depth > 0)
            .then(|| requirement(&violation.kind))
            .flatten()
            .map(|failing| nested(element_sets.get(path).unwrap_or(&failing), depth));
        let Some(requirement) = collection else {
            // Not a violation on a collection's element: the framework's own clause,
            // kept exactly as it words it.
            let clause = violation.message();
            if clauses
                .insert(clause.clone(), Clause::Verbatim(clause.clone()))
                .is_none()
            {
                order.push(clause);
            }
            continue;
        };
        // A clause is keyed by the member it is reported at and the requirement it
        // states, so every element of ONE collection accumulates into a single clause —
        // and so do two constraints of one element shape, because both state that
        // shape's whole constraint set.
        let key = format!("{path}\u{1f}{requirement}");
        let clause = match clauses.get_mut(&key) {
            Some(clause) => clause,
            None => {
                order.push(key.clone());
                clauses.entry(key).or_insert(Clause::Collection {
                    path: path.to_string(),
                    requirement,
                    depth,
                    values: Vec::new(),
                })
            }
        };
        if let (Clause::Collection { values, .. }, Some(value)) =
            (clause, violation.value.as_deref())
        {
            // One rendered value per violation PATH: the same element breaking two of
            // its shape's constraints is one value, two elements are two.
            if !values.iter().any(|(seen, _)| *seen == violation.path) {
                values.push((violation.path.clone(), value.to_string()));
            }
        }
    }

    let rendered: Vec<String> = order
        .iter()
        .filter_map(|key| clauses.get(key))
        .map(Clause::render)
        .collect();
    match rendered.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", rendered[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            rendered.join("; ")
        )),
    }
}

/// State `refusal` the way this service states it, or `None` to keep the framework's own
/// wording.
///
/// This is the whole `Service::override_refusal` body: a constraint refusal is answered
/// as `400 ValidationException` — the status and code the framework's own default picks
/// on this service's `restJson1` protocol — carrying [`envelope`]'s message, so the code
/// a client branches on travels with the message it is given.
///
/// `None` for anything that is not a constraint refusal (an unauthenticated caller keeps
/// the framework's 403 code and wording), and `None` when there is nothing to state.
pub(super) fn aws_validation_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: envelope(violations)?,
    })
}

/// Every INPUT shape of this service, as the generated schema statics.
///
/// All of them, not the ones one report happened to cite: an element constraint is a
/// property of an input shape, so leaving an operation out would silently word its
/// collection clauses with less than the model declares. Regenerate with
/// `grep -h 'const INPUT_SCHEMA' src/ops/*.rs`, which names exactly these statics; an
/// input shape missing from this list is not a wrong clause, only a clause that states
/// the constraints that failed rather than the element shape's whole set.
fn input_schemas() -> Vec<&'static Schema> {
    use crate::ops::types as t;
    vec![
        t::ACCEPT_INBOUND_CONNECTION_INPUT_SCHEMA,
        t::ADD_DATA_SOURCE_INPUT_SCHEMA,
        t::ADD_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        t::ADD_TAGS_INPUT_SCHEMA,
        t::ASSOCIATE_PACKAGE_INPUT_SCHEMA,
        t::ASSOCIATE_PACKAGES_INPUT_SCHEMA,
        t::ATTACH_DATA_SOURCE_INPUT_SCHEMA,
        t::AUTHORIZE_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::CANCEL_DOMAIN_CONFIG_CHANGE_INPUT_SCHEMA,
        t::CANCEL_SERVICE_SOFTWARE_UPDATE_INPUT_SCHEMA,
        t::CREATE_APPLICATION_INPUT_SCHEMA,
        t::CREATE_DOMAIN_INPUT_SCHEMA,
        t::CREATE_INDEX_INPUT_SCHEMA,
        t::CREATE_OUTBOUND_CONNECTION_INPUT_SCHEMA,
        t::CREATE_PACKAGE_INPUT_SCHEMA,
        t::CREATE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::DELETE_APPLICATION_INPUT_SCHEMA,
        t::DELETE_DATA_SOURCE_INPUT_SCHEMA,
        t::DELETE_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        t::DELETE_DOMAIN_INPUT_SCHEMA,
        t::DELETE_INBOUND_CONNECTION_INPUT_SCHEMA,
        t::DELETE_INDEX_INPUT_SCHEMA,
        t::DELETE_OUTBOUND_CONNECTION_INPUT_SCHEMA,
        t::DELETE_PACKAGE_INPUT_SCHEMA,
        t::DELETE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::DEREGISTER_CAPABILITY_INPUT_SCHEMA,
        t::DESCRIBE_DATA_SOURCE_ATTACHMENT_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_AUTO_TUNES_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_CHANGE_PROGRESS_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_CONFIG_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_HEALTH_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_NODES_INPUT_SCHEMA,
        t::DESCRIBE_DOMAINS_INPUT_SCHEMA,
        t::DESCRIBE_DRY_RUN_PROGRESS_INPUT_SCHEMA,
        t::DESCRIBE_INBOUND_CONNECTIONS_INPUT_SCHEMA,
        t::DESCRIBE_INSIGHT_DETAILS_INPUT_SCHEMA,
        t::DESCRIBE_INSTANCE_TYPE_LIMITS_INPUT_SCHEMA,
        t::DESCRIBE_OUTBOUND_CONNECTIONS_INPUT_SCHEMA,
        t::DESCRIBE_PACKAGES_INPUT_SCHEMA,
        t::DESCRIBE_RESERVED_INSTANCE_OFFERINGS_INPUT_SCHEMA,
        t::DESCRIBE_RESERVED_INSTANCES_INPUT_SCHEMA,
        t::DESCRIBE_VPC_ENDPOINTS_INPUT_SCHEMA,
        t::DETACH_DATA_SOURCE_INPUT_SCHEMA,
        t::DISSOCIATE_PACKAGE_INPUT_SCHEMA,
        t::DISSOCIATE_PACKAGES_INPUT_SCHEMA,
        t::GET_APPLICATION_INPUT_SCHEMA,
        t::GET_CAPABILITY_INPUT_SCHEMA,
        t::GET_COMPATIBLE_VERSIONS_INPUT_SCHEMA,
        t::GET_DATA_SOURCE_INPUT_SCHEMA,
        t::GET_DEFAULT_APPLICATION_SETTING_INPUT_SCHEMA,
        t::GET_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        t::GET_DOMAIN_MAINTENANCE_STATUS_INPUT_SCHEMA,
        t::GET_INDEX_INPUT_SCHEMA,
        t::GET_MIGRATION_INPUT_SCHEMA,
        t::GET_PACKAGE_VERSION_HISTORY_INPUT_SCHEMA,
        t::GET_UPGRADE_HISTORY_INPUT_SCHEMA,
        t::GET_UPGRADE_STATUS_INPUT_SCHEMA,
        t::INSIGHT_FEEDBACK_INPUT_SCHEMA,
        t::LIST_APPLICATIONS_INPUT_SCHEMA,
        t::LIST_DATA_SOURCE_ATTACHMENTS_INPUT_SCHEMA,
        t::LIST_DATA_SOURCES_INPUT_SCHEMA,
        t::LIST_DIRECT_QUERY_DATA_SOURCES_INPUT_SCHEMA,
        t::LIST_DOMAIN_MAINTENANCES_INPUT_SCHEMA,
        t::LIST_DOMAIN_NAMES_INPUT_SCHEMA,
        t::LIST_DOMAINS_FOR_PACKAGE_INPUT_SCHEMA,
        t::LIST_INSIGHTS_INPUT_SCHEMA,
        t::LIST_INSTANCE_TYPE_DETAILS_INPUT_SCHEMA,
        t::LIST_MIGRATIONS_INPUT_SCHEMA,
        t::LIST_PACKAGES_FOR_DOMAIN_INPUT_SCHEMA,
        t::LIST_SCHEDULED_ACTIONS_INPUT_SCHEMA,
        t::LIST_TAGS_INPUT_SCHEMA,
        t::LIST_VERSIONS_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINTS_FOR_DOMAIN_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINTS_INPUT_SCHEMA,
        t::PURCHASE_RESERVED_INSTANCE_OFFERING_INPUT_SCHEMA,
        t::PUT_DEFAULT_APPLICATION_SETTING_INPUT_SCHEMA,
        t::REGISTER_CAPABILITY_INPUT_SCHEMA,
        t::REJECT_INBOUND_CONNECTION_INPUT_SCHEMA,
        t::REMOVE_TAGS_INPUT_SCHEMA,
        t::REVOKE_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::ROLLBACK_SERVICE_SOFTWARE_UPDATE_INPUT_SCHEMA,
        t::START_DOMAIN_MAINTENANCE_INPUT_SCHEMA,
        t::START_MIGRATION_INPUT_SCHEMA,
        t::START_SERVICE_SOFTWARE_UPDATE_INPUT_SCHEMA,
        t::UPDATE_APPLICATION_INPUT_SCHEMA,
        t::UPDATE_DATA_SOURCE_INPUT_SCHEMA,
        t::UPDATE_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        t::UPDATE_DOMAIN_CONFIG_INPUT_SCHEMA,
        t::UPDATE_INDEX_INPUT_SCHEMA,
        t::UPDATE_PACKAGE_INPUT_SCHEMA,
        t::UPDATE_PACKAGE_SCOPE_INPUT_SCHEMA,
        t::UPDATE_SCHEDULED_ACTION_INPUT_SCHEMA,
        t::UPDATE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::UPGRADE_DOMAIN_INPUT_SCHEMA,
    ]
}

#[cfg(test)]
mod tests {
    use super::{aws_validation_refusal, collapsed, ELEMENT_CONSTRAINTS};
    use aws_smithy_schema_server::{Violation, ViolationKind};
    use smithy_server_runtime::Refusal;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: &str) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some(value.to_owned()),
        }
    }

    /// RULE: an element path collapses onto the collection it points into, and nothing
    /// else does.
    #[test]
    fn an_element_path_collapses_onto_its_collection() {
        // (case, the path the walk reported, the member a clause names, the nesting)
        let rows: [(&str, &str, &str, usize); 6] = [
            ("a scalar member", "domainName", "domainName", 0),
            (
                "one list element",
                "packageUserList.1.member",
                "packageUserList",
                1,
            ),
            (
                "an element far down the list",
                "vpcEndpointIds.17.member",
                "vpcEndpointIds",
                1,
            ),
            (
                "a member inside a structure element",
                "packageList.2.member.packageID",
                "packageList.2.member.packageID",
                0,
            ),
            ("a list of lists", "outer.1.member.2.member", "outer", 2),
            (
                "a member merely named member",
                "filters.member",
                "filters.member",
                0,
            ),
        ];
        for (case, path, expected, depth) in rows {
            let actual = collapsed(path);
            assert_eq!(
                actual,
                (expected, depth),
                "case {case:?}: expected {expected:?} at depth {depth}, got {actual:?}"
            );
        }
    }

    /// RULE: a violation on a collection's element is ONE validation error, reported at
    /// the collection member, with the element shape's whole constraint set.
    #[test]
    fn element_violations_of_one_member_collapse_onto_one_clause() {
        // Both constraints of `PackageUser` that one malformed account id breaks, as the
        // walk reports them: at the element's indexed path, one violation per constraint.
        let refusal = Refusal::Invalid(vec![
            violation(
                "packageUserList.1.member",
                ViolationKind::MaxLength(12),
                "not-an-account-id",
            ),
            violation(
                "packageUserList.1.member",
                ViolationKind::Pattern(r"^[0-9]{12}$|^GLOBAL$".to_owned()),
                "not-an-account-id",
            ),
        ]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.code, "ValidationException",
            "expected the JSON protocols' own refusal code, got {:?}",
            stated.code
        );
        assert_eq!(
            stated.message,
            "1 validation error detected: Value '[not-an-account-id]' at 'packageUserList' \
             failed to satisfy constraint: Member must satisfy constraint: [Member must have \
             length less than or equal to 12, Member must have length greater than or equal to \
             6, Member must satisfy regular expression pattern: ^[0-9]{12}$|^GLOBAL$]",
            "expected one clause at the list member carrying the element shape's whole \
             constraint set, got {:?}",
            stated.message
        );
    }

    /// RULE: a violation that is not on a collection's element keeps the framework's own
    /// clause, and each one is counted on its own.
    #[test]
    fn a_scalar_violation_keeps_the_frameworks_clause() {
        let refusal = Refusal::Invalid(vec![
            violation("domainName", ViolationKind::MinLength(3), "ab"),
            Violation {
                path: "packageID".to_owned(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
            },
        ]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.message,
            "2 validation errors detected: Value 'ab' at 'domainName' failed to satisfy \
             constraint: Member must have length greater than or equal to 3; Value null at \
             'packageID' failed to satisfy constraint: Member must not be null",
            "expected the framework's own clauses, got {:?}",
            stated.message
        );
    }

    /// RULE: the element constraint set a clause states is read off the MODEL, so it
    /// carries a member's constraints whether or not the request broke them.
    #[test]
    fn the_element_constraint_catalog_is_built_from_the_model() {
        let sets = ELEMENT_CONSTRAINTS
            .get("packageUserList")
            .expect("the model declares element constraints for packageUserList");
        assert!(
            sets.iter().any(|set| set
                == &vec![
                    "Member must have length less than or equal to 12".to_owned(),
                    "Member must have length greater than or equal to 6".to_owned(),
                    "Member must satisfy regular expression pattern: ^[0-9]{12}$|^GLOBAL$"
                        .to_owned(),
                ]),
            "expected PackageUser's whole constraint set in model order, got {sets:?}"
        );
    }

    /// RULE: a refusal that is no constraint failure at all keeps the framework's wording.
    #[test]
    fn a_refusal_that_is_not_a_constraint_keeps_the_frameworks_wording() {
        assert!(
            aws_validation_refusal(&Refusal::Unauthenticated).is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
        assert!(
            aws_validation_refusal(&Refusal::Invalid(Vec::new())).is_none(),
            "expected no violations to state nothing"
        );
    }
}

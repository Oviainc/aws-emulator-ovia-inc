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
//! 2. a clause states its member's dotted path and the requirement it failed, and whether
//!    it also QUOTES the value the member held is decided by the SHAPE reached at that
//!    member and then by the member's HTTP binding ([`quotes_the_value`]): a shape whose
//!    refusals this service is recorded stating without a value ([`VALUE_FREE_SHAPES`]) is
//!    reported by its position alone, and every other member is quoted exactly where the
//!    model binds it to the request URI — a `@httpLabel` path segment or a `@httpQuery`
//!    parameter, the tokens the service reads off the request line
//!    ([`URI_BOUND_MEMBERS`]). A member the service reads out of the request BODY is
//!    reported by its position alone either way
//!    (`Value at '<path>' failed to satisfy constraint: …`), so no body content a caller
//!    submitted reaches the sentence. Neither decision is the violation's CLASS, so one
//!    rule covers a `@length`, a `@range`, a `@pattern` and an enum alike; a member the
//!    request OMITTED held no value at all and is stated as `Value null` whatever its
//!    binding.
//!
//!    What the SHAPE half rests on is the shape being KNOWN, which a violation's path alone
//!    is not enough for: it is known from the expression a `@pattern` violation carries, and
//!    otherwise only where every shape the model declares at that path is the same one
//!    ([`DECLARED_PATTERNS`]). So a member whose name the model declares over several shapes
//!    — `jobIdentifier` is an evaluation-job, a model-invocation-job and an
//!    advanced-prompt-optimization-job identifier — keeps the binding rule for a violation
//!    that names no expression, a `@length` on it say, which is the one case where two
//!    clauses about one such member can state their subject differently. Closing it needs
//!    the violation to carry the shape it came from, which is the framework's
//!    [`Violation`] rather than this seam;
//! 3. a reported `@pattern` is the expression the WHOLE value must match
//!    ([`whole_value_expression`]), with the two ends decided independently: an end the
//!    model ANCHORS carries its `^` / `$` away with it, which is the model's way of making
//!    the match total rather than part of the character grammar, and an end the model leaves
//!    OPEN — where Smithy matches the expression anywhere in the value — is spelled with the
//!    unconstrained run `.*` that end admits. Either way a caller that lifts the expression
//!    out of the message to re-validate locally gets one its own whole value has to satisfy;
//! 4. a violation on an ELEMENT of a collection is reported against the COLLECTION MEMBER
//!    — no element index and no `member` step — with the ELEMENT SHAPE'S WHOLE CONSTRAINT
//!    SET nested inside `Member must satisfy constraint: [ … ]` ([`collapsed`],
//!    [`element_constraint_set`]). The set is the
//!    element's, not just the constraint that happened to fail: the clause tells a caller
//!    what every element of that member has to satisfy. A violation on the collection
//!    ITSELF (its own `@length`, its `@uniqueItems`) is not nested, and neither is one on
//!    a member INSIDE a structure element, which keeps its own full path;
//! 5. an enum clause states the admitted VALUE SET —
//!    `Member must satisfy enum value set: [ … ]`. The set's MEMBERSHIP is the
//!    model's; the ORDER it is listed in is the service's own listing of that enum's values
//!    ([`VALUE_LISTINGS`], [`listed`]), which no order the model carries reproduces. The
//!    order is a property of the ENUM, not of the member or the operation reaching it, so
//!    one enum states one ordered set wherever it is reached — in the body, in the query
//!    string, in a URI label, inside a nested structure, as a list element, and beside other
//!    clauses of the same refusal.
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
//! URI-bound collection's clause states is the one formed from those elements. That
//! divergence, and why closing it belongs elsewhere, is recorded on [`Clause::render`]; a
//! collection the service reads out of the BODY states no value at all (rule 2), so it
//! cannot reach it.
//!
//! A violation class this module has no wording for falls back to the FRAMEWORK's own
//! clause for that one violation ([`ViolationKind`] is `#[non_exhaustive]`, so a class can
//! be added upstream), which keeps the rest of the refusal worded here.
//!
//! MIGRATION NOTE (`services/common/src/validation.rs`): rules 1-3 are AWS's wording
//! rather than this service's, and ~24 sibling crates carry their own copy of parts of
//! them (`grep -rln 'fn unanchored' services/*/src`, and `grep -rn '\.\*{pattern}'` for the
//! open-end half rule 3 adds, which two more crates spell inline). That module is the staged
//! home for exactly those clause rules; as its header asks, this is the note rather than a
//! silent further copy — migrating them re-words services whose recorded behaviour their own
//! suites pin, so it is one change of its own, and that module now records this service's
//! whole-value rendering beside the pair-positional one the crates calling it state. What
//! would stay here is rule 4's model-derived element constraint set.

use std::borrow::Cow;
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

/// The `@pattern`, as a requirement reads: the expression the WHOLE value must match
/// (rule 3).
fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        whole_value_expression(pattern)
    )
}

/// An enumerated value set, as a requirement reads: the MEMBERSHIP the model declares, in
/// the order this service LISTS that enum's values ([`listed`]).
fn enum_requirement<S: AsRef<str>>(values: &[S]) -> String {
    let values: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
    format!(
        "Member must satisfy enum value set: [{}]",
        listed(&values).join(", ")
    )
}

/// The order this service states an enum's admitted values in, for the enums it is observed
/// to publish a listing for.
///
/// An enum clause states a MEMBERSHIP and an ORDER. The membership is the model's, always.
/// The order is not: this service lists `DataRetentionMode` as
/// `default, provider_data_share, …` while the model declares `default, none, …`, and no
/// order derivable from the model — declaration order, sorted, reversed, by length, by the
/// bucket order of a hashed set — produces it. It is service metadata the public model does
/// not carry, the way rule 4's requirement order is: the only source for either is what the
/// service's refusals are recorded stating. So a listing is recorded per enum HERE, beside
/// the one renderer every enum-constrained member of every operation goes through, rather
/// than in an operation handler.
///
/// A listing is identified by the SET it orders — which is what a violation carries, the
/// clause having no shape id — and is applied ONLY to an enum whose admitted set it is a
/// permutation of. A listing can therefore neither add a value this model does not declare
/// nor drop one it does, and an enum whose membership changes in the model stops matching a
/// stale listing and states the model's order rather than an outdated set. An enum with no
/// recorded listing keeps the order the member schema carries, which is the model's.
///
/// What a membership CANNOT identify is one enum among several declared over the same
/// values: this model declares `Active, Creating, Failed` as both `ModelStatus`
/// (`ListCustomModels.modelStatus`) and `CustomModelDeploymentStatus`
/// (`ListCustomModelDeployments.statusEquals`), and `CreationTime` as three distinct sort
/// enums. No listing below orders such a membership — the one recorded listing's five values
/// are declared by one enum only, so it reaches exactly that enum — and a listing recorded
/// for one of those enums would need narrower identification than its set before it could be
/// applied to either. Both orders those two state today are their own model order, pinned by
/// `tests/all/enum_value_set_listing.rs`.
const VALUE_LISTINGS: &[&[&str]] = &[
    // `DataRetentionMode` (`PutAccountDataRetention.mode`), which the model declares
    // `default, none, aws_review, provider_data_share, inherit`. The listing is the one
    // both recorded refusals of that surface state
    // (`bedrock.put-account-data-retention-invalid-period-out-of-range-rejected` and
    // `bedrock.account-data-retention-ttl-enforced-on-logs`).
    &[
        "default",
        "provider_data_share",
        "none",
        "aws_review",
        "inherit",
    ],
];

/// `values` in the order this service lists them, or in the order they arrived — the
/// model's, off the member schema — when this service publishes no listing for that enum.
fn listed<'v>(values: &[&'v str]) -> Vec<&'v str> {
    match VALUE_LISTINGS
        .iter()
        .find(|listing| orders_the_same_set(listing, values))
    {
        Some(listing) => listing.to_vec(),
        None => values.to_vec(),
    }
}

/// Whether two value lists hold the same values, in whatever order each holds them.
fn orders_the_same_set(one: &[&str], other: &[&str]) -> bool {
    if one.len() != other.len() {
        return false;
    }
    let mut one: Vec<&str> = one.to_vec();
    let mut other: Vec<&str> = other.to_vec();
    one.sort_unstable();
    other.sort_unstable();
    one == other
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

/// The `@pattern` the model declares for an identifier shape whose refusals this service
/// states WITHOUT the value ([`VALUE_FREE_SHAPES`]).
///
/// `AdvancedPromptOptimizationJobIdentifier` (`query="shape"`, shape
/// `AdvancedPromptOptimizationJobIdentifier`): reached as the `jobIdentifier` label of the
/// job's read and stop, and as the element of its `jobIdentifiers` batch-delete list.
const ADVANCED_PROMPT_OPTIMIZATION_JOB_IDENTIFIER: &str =
    "^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})$";

/// `AutomatedReasoningPolicyTestCaseId` (`query="shape"`, shape
/// `AutomatedReasoningPolicyTestCaseId`): the `testCaseId` label of the test case's read,
/// update and delete and of a test result's read, and the element of the
/// `testCaseIds` list a test workflow takes.
const AUTOMATED_REASONING_POLICY_TEST_CASE_ID: &str = "^[0-9A-Z]{12}$";

/// The SHAPES whose refusals this service states by the member's position alone, each as
/// the `@pattern` the model declares for it — the shape's identity in the model, and what a
/// `@pattern` violation carries, which is how [`crate::service`]'s own ARN seam identifies
/// the member a violation came from.
///
/// Whether a refusal quotes the value is not derivable from the model: this service states
/// `Value '<the identifier>' at 'guardrailIdentifier' …` for a guardrail it will not read
/// and `Value at 'jobIdentifier' …` for an advanced-prompt-optimization job, over two
/// members the model declares the same way — a pattern-constrained identifier in a
/// `@httpLabel`. It is service metadata the public model does not carry, the way
/// [`VALUE_LISTINGS`] is, and the only source for it is what the service's refusals are
/// recorded stating. So it is recorded HERE, beside the one formatter every constrained
/// member of every operation goes through, rather than in an operation handler.
///
/// Keyed by the SHAPE rather than by the member NAME: that is what lets `jobIdentifier` be
/// stated both ways — the model declares that one name over an evaluation-job identifier, a
/// model-invocation-job identifier and this one — and what applies an entry to every
/// operation, binding, member path and constraint class reaching the shape rather than to
/// the one a report happened to cite, as far as the shape is knowable from a violation
/// (rule 2's note on [`DECLARED_PATTERNS`]).
const VALUE_FREE_SHAPES: &[&str] = &[
    ADVANCED_PROMPT_OPTIMIZATION_JOB_IDENTIFIER,
    AUTOMATED_REASONING_POLICY_TEST_CASE_ID,
];

/// Whether `pattern` is the expression a shape this service states by position alone
/// declares.
fn value_free_shape(pattern: &str) -> bool {
    VALUE_FREE_SHAPES.contains(&pattern)
}

/// Whether a clause reported at `member_path` QUOTES the value the request carried there,
/// given the `@pattern`s the shapes reached at that path declare (`declared`).
///
/// Two rules, in order:
///
/// 1. the SHAPE decides it where this service is recorded stating that shape's refusals
///    without a value ([`VALUE_FREE_SHAPES`]) — whatever constraint class failed, whatever
///    binding carried it, and whichever operation reached it. Only when EVERY shape the
///    clause can be about is such a shape: `declared` holds the expressions the clause's own
///    violations carried, which name the shape the request actually broke, and falls back to
///    the ones the model declares at that path ([`DECLARED_PATTERNS`]) for a clause whose
///    violations carry none — so one member NAME the model declares over several shapes is
///    reported for the shape that failed, and keeps the binding rule below when its shape
///    cannot be told apart;
/// 2. otherwise WHERE the service read the value decides it, which is what the model's HTTP
///    binding says: a member bound to the request URI — a `@httpLabel` path segment or a
///    `@httpQuery` parameter, the tokens the service reads off the request line — is quoted
///    back, and a member read anywhere else (the request body, a header) is reported by its
///    position alone ([`URI_BOUND_MEMBERS`]). The binding is a property of the MEMBER, so
///    one decision covers every constraint class reaching it; a member NESTED inside a body
///    structure is never URI-bound, so its path reaches this with no entry and is reported
///    by position.
fn quotes_the_value(member_path: &str, declared: &[&str]) -> bool {
    let model = DECLARED_PATTERNS.get(member_path);
    // The shapes the clause can be about: the ones its violations named, else the ones the
    // model declares at that path.
    let shapes: &[&str] = if declared.is_empty() {
        model.map(Vec::as_slice).unwrap_or_default()
    } else {
        declared
    };
    if !shapes.is_empty() && shapes.iter().all(|pattern| value_free_shape(pattern)) {
        return false;
    }
    URI_BOUND_MEMBERS.contains(member_path)
}

/// The `@pattern`s this service's input shapes declare at each member path — the member's
/// own, and its ELEMENT's for a collection member — so a clause about a member no `@pattern`
/// violation opened still knows which shapes its path can be reporting.
///
/// Built once off the generated schemas, the way [`ELEMENT_CONSTRAINTS`] is, and read only
/// when the violations themselves carry no declared expression: a request that broke the
/// `@pattern` names the shape exactly, while one that broke only a `@length` of the same
/// member names it no more precisely than the model does.
static DECLARED_PATTERNS: LazyLock<HashMap<String, Vec<&'static str>>> = LazyLock::new(|| {
    let mut catalog: HashMap<String, Vec<&'static str>> = HashMap::new();
    for schema in input_schemas() {
        for member in schema.members() {
            let Some(name) = member.member_name() else {
                continue;
            };
            let Some(traits) = member.traits() else {
                continue;
            };
            let element = constraint_trait::<ElementConstraintsTrait>(
                traits,
                &ElementConstraintsTrait::TRAIT_ID,
            )
            .and_then(|wrapper| {
                constraint_trait::<PatternTrait>(wrapper.inner(), &PatternTrait::TRAIT_ID)
            });
            let own = constraint_trait::<PatternTrait>(traits, &PatternTrait::TRAIT_ID);
            for pattern in [own, element].into_iter().flatten() {
                let declared = catalog.entry(constraints::path::member(name)).or_default();
                if !declared.contains(&pattern.regex) {
                    declared.push(pattern.regex);
                }
            }
        }
    }
    catalog
});

/// The top-level members of this service's input shapes the model binds to the request URI,
/// keyed by the member path the validation walk reports them under.
///
/// Built once off the generated schemas, the way [`ELEMENT_CONSTRAINTS`] is — so the
/// binding is the MODEL's and a member whose binding changes there changes here with it. A
/// member NAME several operations declare is recorded only when every one of them binds it
/// to the URI: `resourceArn` is a path label on the resource-policy operations and a body
/// member on others, and a violation's path cannot tell those apart, so an ambiguous name
/// is reported the way a body member is.
static URI_BOUND_MEMBERS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let mut uri: HashSet<String> = HashSet::new();
    let mut read_elsewhere: HashSet<String> = HashSet::new();
    for schema in input_schemas() {
        for member in schema.members() {
            let Some(name) = member.member_name() else {
                continue;
            };
            let path = constraints::path::member(name);
            if member.http_label().is_some()
                || member.http_query().is_some()
                || member.http_query_params().is_some()
            {
                uri.insert(path);
            } else {
                read_elsewhere.insert(path);
            }
        }
    }
    uri.retain(|path| !read_elsewhere.contains(path));
    uri
});

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

/// The unconstrained run an expression spells for an end a `@pattern` leaves open.
const ANY: &str = ".*";

/// A `@pattern` as a requirement QUOTES it: the expression the WHOLE value must match.
///
/// A Smithy `@pattern` is a FIND rather than a total match, so the expression a member
/// declares and the expression its whole value must satisfy are two different things, and a
/// requirement states the second — it is what a caller lifts out of the message to
/// re-validate locally. The two ends are decided independently, because a model may anchor
/// one of them and leave the other open:
///
/// * an end the model ANCHORS carries the `^` / `$` away with it ([`split_anchors`]): that
///   is the model's way of making the match total at that end, not part of the character
///   grammar a caller has to satisfy;
/// * an end the model leaves OPEN admits any run of characters before (or after) the match,
///   which the requirement spells as [`ANY`] unless the expression already leaves that end
///   open with one;
/// * an expression that anchors neither end but carries an anchor inside itself
///   ([`carries_an_anchor`]) — this model declares one that anchors each of its top-level
///   alternatives — already describes the whole value, so it is quoted as it is declared.
fn whole_value_expression(pattern: &str) -> Cow<'_, str> {
    let (body, anchored_at_start, anchored_at_end) = split_anchors(pattern);
    match (anchored_at_start, anchored_at_end) {
        (true, true) => Cow::Borrowed(body),
        // Anchored inside its own alternatives rather than at either end.
        (false, false) if carries_an_anchor(pattern) => Cow::Borrowed(body),
        (start, end) => Cow::Owned(opened(body, !start, !end)),
    }
}

/// `body` with the unconstrained run [`ANY`] spelled at each open end, unless the
/// expression already leaves that end open with one.
fn opened(body: &str, at_start: bool, at_end: bool) -> String {
    let mut expression = String::with_capacity(body.len() + 2 * ANY.len());
    if at_start && !body.starts_with(ANY) {
        expression.push_str(ANY);
    }
    expression.push_str(body);
    if at_end && !body.ends_with(ANY) {
        expression.push_str(ANY);
    }
    expression
}

/// Whether `pattern` anchors its match: it carries a `^` or a `$` that is an ANCHOR rather
/// than an ordinary character.
///
/// Either character inside a character class (`[^:]+`) is an ordinary member of that class,
/// and either one escaped by a backslash is the literal character, so neither anchors
/// anything. The anchor may sit ANYWHERE in the expression, not only at its ends: this
/// model declares one that anchors each of its top-level alternatives
/// (`(^…$)|(^…$)`), which describes the whole value exactly as an expression anchored at
/// its ends does.
fn carries_an_anchor(pattern: &str) -> bool {
    let mut escaped = false;
    let mut in_class = false;
    for character in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '^' | '$' if !in_class => return true,
            _ => {}
        }
    }
    false
}

/// A `@pattern` expression without the anchors a model may spell it at its ends, and WHICH
/// of the two ends carried one — reported together, so a caller that has to tell an
/// anchored end from an open one cannot disagree with what was stripped.
///
/// A `$` preceded by an ODD number of backslashes is an escaped dollar sign — a literal
/// character of the expression, not the end anchor — so it stays and that end is open.
fn split_anchors(pattern: &str) -> (&str, bool, bool) {
    let (body, at_start) = match pattern.strip_prefix('^') {
        Some(body) => (body, true),
        None => (pattern, false),
    };
    match body.strip_suffix('$') {
        Some(trimmed)
            if (trimmed.len() - trimmed.trim_end_matches('\\').len()) % 2 == 0 =>
        {
            (trimmed, at_start, true)
        }
        _ => (body, at_start, false),
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
    /// Whether the clause QUOTES the value the request carried at this member: decided
    /// once, from where the service read it ([`quotes_the_value`]), so every violation that
    /// folds into the clause states it the same way.
    quotes_value: bool,
    /// Whether the value this clause is about was WITHHELD rather than absent: the
    /// framework hands a `@sensitive` member's violation over with no value and says so,
    /// which is what separates `Value` from the `Value null` an omitted member earns.
    withheld: bool,
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
        // A member the request OMITTED held no value at all, whatever its binding, and is
        // reported as the `null` it was. Read off what the clause COLLECTED rather than off
        // the violation that opened it, so a clause that did collect a value is never
        // reported as an absence.
        let subject = if self.withheld {
            // The member carried a value; it is `@sensitive`, so the clause names the
            // member and the requirement and nothing of what it held.
            "Value".to_string()
        } else if self.values.is_empty() {
            "Value null".to_string()
        } else if !self.quotes_value {
            "Value".to_string()
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

    // The SHAPES each member path can be reporting, as the `@pattern`s the violations
    // themselves carry — which name the shape the request actually broke. Resolved once per
    // path, so every clause about one member decides its subject the same way; a path whose
    // violations carry no expression is left to the shapes the model declares there
    // ([`quotes_the_value`]).
    let mut declared: HashMap<&str, Vec<&str>> = HashMap::new();
    for violation in violations {
        let (path, _) = collapsed(&violation.path);
        if let ViolationKind::Pattern(pattern) = &violation.kind {
            let patterns = declared.entry(path).or_default();
            let pattern: &str = pattern;
            if !patterns.contains(&pattern) {
                patterns.push(pattern);
            }
        }
    }

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
                    // A property of the SHAPE the clause is reported about, else of the
                    // MEMBER's binding, so it is the same for every violation that folds
                    // into this clause.
                    quotes_value: quotes_the_value(
                        path,
                        declared.get(path).map(Vec::as_slice).unwrap_or_default(),
                    ),
                    withheld: false,
                })
            }
        };
        clause.withheld |= violation.sensitive;
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

    /// Every enum value set this model declares on an input member, read off the generated
    /// schemas the same way the element constraint catalog is.
    fn declared_enum_sets() -> Vec<Vec<&'static str>> {
        let mut sets: Vec<Vec<&'static str>> = Vec::new();
        for schema in input_schemas() {
            for member in schema.members() {
                let Some(traits) = member.traits() else {
                    continue;
                };
                if let Some(allowed) = constraint_trait::<EnumTrait>(traits, &EnumTrait::TRAIT_ID) {
                    let set: Vec<&'static str> = allowed.values.to_vec();
                    if !sets.contains(&set) {
                        sets.push(set);
                    }
                }
            }
        }
        sets
    }

    /// A listing orders a set this MODEL declares: it is a permutation of one of them, so
    /// it can neither add a value the model does not admit nor drop one it does.
    #[test]
    fn every_listing_orders_a_value_set_this_model_declares() {
        let declared = declared_enum_sets();
        assert!(
            !declared.is_empty(),
            "this model declares enum-constrained input members, so the schemas must carry \
             value sets"
        );
        for listing in VALUE_LISTINGS {
            assert!(
                declared
                    .iter()
                    .any(|set| orders_the_same_set(listing, set)),
                "case {listing:?}: a listing must order a value set this model declares, \
                 and none of {declared:?} holds those values"
            );
        }
    }

    /// A listing's membership identifies ONE enum: no other value set this model declares
    /// on an input member holds the same values in another order.
    ///
    /// [`listed`] resolves a listing by membership, a violation carrying no shape id, so a
    /// membership two enums declare differently could not be told apart — this model
    /// declares `Active, Creating, Failed` as two enums, and `CreationTime` as three. The
    /// listings recorded today order none of those, and this is what keeps it that way: a
    /// listing added for such a membership fails here rather than silently re-ordering the
    /// other enum's clause. The sets compared are the ones the member schemas of this
    /// service's input shapes carry.
    #[test]
    fn a_listings_membership_is_declared_by_one_enum_only() {
        for listing in VALUE_LISTINGS {
            let sharing: Vec<Vec<&str>> = declared_enum_sets()
                .into_iter()
                .filter(|set| orders_the_same_set(listing, set))
                .collect();
            assert_eq!(
                sharing.len(),
                1,
                "case {listing:?}: a listing is resolved by its membership, so exactly one \
                 declared value set may hold those values, got {sharing:?}"
            );
        }
    }

    /// A listing is resolved by the SET it orders, whatever order the set arrives in, and
    /// an enum with no listing keeps the order it arrived in — the model's.
    #[test]
    fn a_listing_is_applied_only_to_the_enum_whose_set_it_orders() {
        let retention_modes = ["default", "provider_data_share", "none", "aws_review", "inherit"];
        // (case, the set as it arrives off a member schema, the order the clause states)
        let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
            (
                "the listed enum, arriving in the model's declaration order",
                vec!["default", "none", "aws_review", "provider_data_share", "inherit"],
                retention_modes.to_vec(),
            ),
            (
                "the listed enum, arriving sorted",
                vec!["aws_review", "default", "inherit", "none", "provider_data_share"],
                retention_modes.to_vec(),
            ),
            (
                "the listed enum, arriving already listed",
                retention_modes.to_vec(),
                retention_modes.to_vec(),
            ),
            (
                "an enum of the same size with another membership",
                vec!["one", "two", "three", "four", "five"],
                vec!["one", "two", "three", "four", "five"],
            ),
            (
                "a subset of a listed set: not that enum",
                vec!["default", "none", "aws_review", "provider_data_share"],
                vec!["default", "none", "aws_review", "provider_data_share"],
            ),
            (
                "a listed set with one value more: not that enum either",
                vec![
                    "default",
                    "none",
                    "aws_review",
                    "provider_data_share",
                    "inherit",
                    "grandfathered",
                ],
                vec![
                    "default",
                    "none",
                    "aws_review",
                    "provider_data_share",
                    "inherit",
                    "grandfathered",
                ],
            ),
            (
                "an enum with no listing",
                vec!["TEXT", "IMAGE", "EMBEDDING"],
                vec!["TEXT", "IMAGE", "EMBEDDING"],
            ),
            ("a single-valued enum", vec!["CreationTime"], vec!["CreationTime"]),
        ];
        for (case, arriving, expected) in rows {
            let actual = listed(&arriving);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            let requirement = enum_requirement(&arriving);
            let stated = format!(
                "Member must satisfy enum value set: [{}]",
                expected.join(", ")
            );
            assert_eq!(
                requirement, stated,
                "case {case:?}: expected the clause to state {stated:?}, got {requirement:?}"
            );
        }
    }

    /// The anchors are stripped, and only the anchors, and the END each one was stripped
    /// from is reported with the body: a `$` that is part of the expression stays and leaves
    /// that end OPEN. The forms this model reaches are covered end-to-end by
    /// `tests/all/validation_message_wording.rs`; these rows pin the branches no input
    /// shape of THIS model produces today, so the rule stays stated for a model that
    /// grows one.
    #[test]
    fn only_the_anchors_are_stripped_from_a_pattern() {
        // (the declared expression, the body, the end each anchor was stripped from)
        for (pattern, expected, at_start, at_end) in [
            (r"^[a-z]+$", "[a-z]+", true, true),
            (r"[a-z]+", "[a-z]+", false, false),
            (r"^[a-z]+", "[a-z]+", true, false),
            (r"[a-z]+$", "[a-z]+", false, true),
            // An escaped dollar is a literal character, not the end anchor.
            (r"^[a-z]+\$", r"[a-z]+\$", true, false),
            // An escaped backslash before the anchor: the anchor is still an anchor.
            (r"^[a-z]+\\$", r"[a-z]+\\", true, true),
            ("^$", "", true, true),
        ] {
            let actual = split_anchors(pattern);
            assert_eq!(
                actual,
                (expected, at_start, at_end),
                "case {pattern:?}: expected {:?}, got {actual:?}",
                (expected, at_start, at_end)
            );
        }
    }

    /// An expression anchors its match only when a `^` or a `$` of it is an ANCHOR: inside
    /// a character class, or escaped, either character is an ordinary one.
    #[test]
    fn an_anchor_is_told_apart_from_the_same_character_used_literally() {
        for (pattern, anchored) in [
            (r"^[a-z]+$", true),
            (r"^[a-z]+", true),
            (r"[a-z]+$", true),
            // Anchored inside each top-level alternative rather than at the ends, which is
            // how this model spells `TaggableResourcesArn`.
            (r"(^[a-z]+$)|(^arn:aws:[a-z]+$)", true),
            // `^` as the negation of a character class is an ordinary character of it.
            (r"arn:aws(-[^:]+)?:bedrock:.*", false),
            // `$` inside a class, and an escaped one, are the literal character.
            (r"[a-z$]+", false),
            (r"[a-z]+\$", false),
            (r"arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*", false),
        ] {
            let actual = carries_an_anchor(pattern);
            assert_eq!(
                actual, anchored,
                "case {pattern:?}: expected carries_an_anchor {anchored}, got {actual}"
            );
        }
    }

    /// The expression a requirement quotes is the one the WHOLE value must match: an
    /// anchored end loses its anchor, an open end is spelled with the run it admits, and an
    /// expression anchored only inside its alternatives is quoted as declared.
    #[test]
    fn a_quoted_pattern_is_the_expression_the_whole_value_must_match() {
        for (pattern, expected) in [
            // Anchored at both ends: the anchors go, nothing is added.
            (r"^[a-z]+$", "[a-z]+"),
            (r"^(([1-9][0-9]{0,7})|(DRAFT))$", "(([1-9][0-9]{0,7})|(DRAFT))"),
            // Anchored inside its alternatives: there are no end anchors to strip and the
            // expression already describes the whole value.
            (r"(^[a-z]+$)|(^arn:aws:[a-z]+$)", r"(^[a-z]+$)|(^arn:aws:[a-z]+$)"),
            // Unanchored: the run each open end admits is spelled out, unless the
            // expression already leaves that end open.
            (r"[a-z]+", r".*[a-z]+.*"),
            (
                r"arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*",
                r".*arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*",
            ),
            (r".*suffix", r".*suffix.*"),
            (r".*both.*", r".*both.*"),
            // Anchored at ONE end: that end loses its anchor and the OTHER is spelled with
            // the run it admits. No input shape of this model declares such an expression
            // today; the rows keep the rule stated for a model that grows one.
            (r"^[a-z]+", r"[a-z]+.*"),
            (r"[a-z]+$", r".*[a-z]+"),
            // An escaped dollar is a literal character, so that end is open.
            (r"^[a-z]+\$", r"[a-z]+\$.*"),
        ] {
            let actual = whole_value_expression(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The subject a clause opens with is decided by the SHAPE reached at the member, and
    /// otherwise by the member's HTTP binding; every URI-bound member is a top-level path.
    #[test]
    fn the_subject_follows_the_shape_then_the_models_binding() {
        let bound = &*URI_BOUND_MEMBERS;
        assert!(
            !bound.is_empty(),
            "this model binds input members to the URI, so the catalog must not be empty"
        );
        for path in bound {
            assert!(
                !path.contains('.'),
                "case {path:?}: only a top-level member carries an HTTP binding"
            );
        }
        // Every entry of the recorded shape catalog is an expression the model declares at
        // some input member, so no entry is dead; and the member paths it reaches are what
        // the entry applies to, whatever operation or binding gets there.
        for declared in VALUE_FREE_SHAPES {
            let reached: Vec<&String> = DECLARED_PATTERNS
                .iter()
                .filter(|(_, patterns)| patterns.contains(declared))
                .map(|(path, _)| path)
                .collect();
            assert!(
                !reached.is_empty(),
                "case {declared:?}: no input member of this model declares that pattern, so \
                 the entry reaches nothing"
            );
            for path in reached {
                assert!(
                    !quotes_the_value(path, &[declared]),
                    "case {declared:?}: the entry must reach every member declaring it, \
                     {path:?} quotes its value"
                );
            }
        }
        // (the member path, the `@pattern`s its clause can be reporting, the subject).
        // A recorded shape against the OTHER shapes the same member name is declared over,
        // a query parameter, a path label, a body member, a member nested in one, and the
        // member names the model binds both ways (`smithy_query(query="operation")`:
        // `ListCustomModels` binds `modelStatus` and `maxResults` to the query string;
        // `GetGuardrail` binds `guardrailIdentifier` and `GetEvaluationJob` `jobIdentifier`
        // to a path label; `CreateGuardrail` carries `name` in the body and
        // `CreateModelInvocationJob` reaches `s3Uri` inside it; `resourceArn` is a path
        // label on the resource-policy operations and a body member on others, which a
        // violation's path cannot tell apart, so an ambiguous name keeps the body form).
        let evaluation_job_identifier =
            "^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$";
        let guardrail_identifier = "^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:\
                                    [0-9]{12}:guardrail/[a-z0-9]+))$";
        for (path, declared, quotes) in [
            // The recorded shapes: no value, whatever broke and wherever they are reached.
            (
                "jobIdentifier",
                vec![ADVANCED_PROMPT_OPTIMIZATION_JOB_IDENTIFIER],
                false,
            ),
            (
                "jobIdentifiers",
                vec![ADVANCED_PROMPT_OPTIMIZATION_JOB_IDENTIFIER],
                false,
            ),
            (
                "testCaseId",
                vec![AUTOMATED_REASONING_POLICY_TEST_CASE_ID],
                false,
            ),
            // The same member NAME over another shape: the binding decides it again.
            ("jobIdentifier", vec![evaluation_job_identifier], true),
            ("guardrailIdentifier", vec![guardrail_identifier], true),
            // A clause whose violations carry no expression falls back to the shapes the
            // model declares at that path: unanimous for `testCaseId`, several for
            // `jobIdentifier`.
            ("testCaseId", vec![], false),
            ("jobIdentifier", vec![], true),
            ("modelStatus", vec![], true),
            ("maxResults", vec![], true),
            ("sortOrder", vec![], true),
            ("name", vec![], false),
            ("modelSourceIdentifier", vec![], false),
            ("inputDataConfig.s3InputDataConfig.s3Uri", vec![], false),
            ("resourceArn", vec![], false),
        ] {
            let actual = quotes_the_value(path, &declared);
            assert_eq!(
                actual, quotes,
                "case {path:?} over {declared:?}: expected quotes_the_value {quotes}, got \
                 {actual}"
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

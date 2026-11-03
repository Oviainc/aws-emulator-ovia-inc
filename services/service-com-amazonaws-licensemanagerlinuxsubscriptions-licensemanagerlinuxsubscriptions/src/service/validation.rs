// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The framework walks the modelled constraints and hands [`Refusal`] to `service.rs`
//! STRUCTURALLY — one [`Violation`] per offending value, carrying the path it sits at,
//! what it failed and what it held — and leaves the wording to the service, because no
//! model records it. This module is that wording for every modelled input constraint of
//! this service: ONE formatter, reached by every operation's boundary validation, so no
//! operation can word a refusal its own way.
//!
//! The rules it implements:
//!
//! 1. **The tagging surface answers its own taxonomy.** The tag APIs
//!    (`/tags/{resourceArn}`) address their resource with the `resourceArn` label, and
//!    a value outside that label's modelled grammar is refused with
//!    `BadRequestException` — not the generic `ValidationException`, which
//!    `UntagResource` does not even model. A tag SELECTION past its modelled size limit
//!    is refused with `BadRequestException` too, wherever that selection appears —
//!    whether it travels as the tag MAP of a write (`RegisterSubscriptionProvider`'s
//!    `Tags` as much as `TagResource`'s `tags`) or as the tag-KEY LIST of a removal
//!    (`UntagResource`'s `tagKeys`), since both select from the one tag set whose size
//!    the limit is about. And so is a required REQUEST PARAMETER the request does not
//!    carry — a member the model binds to the query string, which the request therefore
//!    has to name for the operation to be addressed at all: that refusal states the
//!    parameters to add (see [`absent_request_parameters`]), and it is the surface's
//!    because the operation whose parameter it is models no generic constraint error to
//!    state it with. All three are the refusals `crate::scaffolded` states, so the
//!    EXISTENCE stage a handler answers (`NotFoundException`) sits behind the same
//!    taxonomy.
//! 2. **Every other refusal is the AWS constraint envelope** under this service's
//!    modelled `ValidationException`: `N validation error(s) detected: ` over one
//!    clause per violation, each naming the member path, the value it held and the
//!    constraint it failed. Only the two clause rules below are this service's own; the
//!    wording of every other constraint class is the FRAMEWORK's, read back off the
//!    clause it builds, so it cannot drift from the rest of the emulator.
//! 3. **A `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors**
//!    the model spells it with: the advertised constraint is the character grammar a
//!    value must satisfy, and a caller that lifts the expression out of the message and
//!    re-applies it locally must get what the service advertises, not the model's way of
//!    making the match total.
//! 4. **A violation on an ELEMENT of a collection is reported against the COLLECTION
//!    MEMBER** — no element index and no `member` step — with the element's requirement
//!    nested inside `Member must satisfy constraint: [ … ]`, which is what tells a
//!    caller the violation is on an element rather than on the collection, and with the
//!    value rendered as a bracketed collection of the elements that failed (the
//!    structured refusal carries those, not the whole collection — see
//!    [`Clause::render`]). A violation on the collection ITSELF (its `@length`, its
//!    `@uniqueItems`) is not nested, and neither is one on a member INSIDE a structure
//!    element, which keeps its own full path.
//!
//! Nothing here is keyed to an operation or to a value: the rules are applied to
//! whatever [`Violation`] the walk produces, so they hold for every constrained member
//! of every operation in this service — the `@pattern` on a `SubscriptionProviderArn` or
//! a `SecretArn`, the `@range` on a `MaxResults`, the `@length`/`@uniqueItems` on
//! `SourceRegions`, and the enum element sets of `SubscriptionProviderSources` alike.
//!
//! A violation class this module has no wording for declines the WHOLE refusal
//! (`None`), which keeps the framework's default sentence rather than inventing one:
//! [`ViolationKind`] is `#[non_exhaustive]`, so a class can be added upstream.
//!
//! The halves of rules 3 and 4 that are NOT a per-service dialect are READ from
//! `services/common/src/validation.rs`, the one home for them in this tree, rather than
//! restated here: a `@pattern` printed without the anchors the model wraps it in
//! ([`unanchored`]) and the recognition of a collection-element path
//! ([`list_of_element`]). What stays this service's own is the tag-API taxonomy of rule
//! 1, the collection-VALUE rendering of rule 4 (see [`Clause::render`]), and the
//! requirement wording of every other class, which is read back off the framework's own
//! clause rather than copied from it ([`framework_requirement`]).

use std::collections::{HashMap, HashSet};

use service_common::validation::{list_of_element, unanchored};
use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

use crate::scaffolded::{UnmodeledRefusal, BAD_REQUEST_CODE};

/// The error code this service answers an input-constraint violation with: its own
/// modelled `ValidationException`, which carries `@httpError 400`
/// (`smithy_query(query="shape",shape="ValidationException")`). It is the code the
/// framework's default would have named too, restated because `override_refusal`
/// replaces code and message TOGETHER.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The member every tag API of this service addresses its resource with: the
/// `@httpLabel` `resourceArn` of `TagResource`, `UntagResource` and
/// `ListTagsForResource`, as `smithy_query(query="operation")` reports them.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The members a tag SELECTION travels in, each `@length 0..50`
/// (`smithy_query(query="operation")`): the `Tags` map of `RegisterSubscriptionProvider`
/// and the `tags` map of `TagResource` — both spell the same lowerCamel path, so one name
/// covers them — and the `tagKeys` list of `UntagResource`. A map names the tags a write
/// leaves and a list names the tags a removal takes away, but each is a selection out of
/// the one tag set a resource may carry, and it is that set's size the limit bounds.
const TAG_SELECTION_MEMBERS: &[&str] = &["tags", "tagKeys"];

/// The separator the framework's own clause puts before the requirement, which is how
/// [`requirement`] reads a framework wording back off a clause.
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint: ";

/// The requirement a violation states.
///
/// Only the `@pattern` wording is this service's own (the expression is quoted
/// UNANCHORED, see [`unanchored`]); every other class keeps the FRAMEWORK's wording,
/// read off the clause the framework builds for that same violation — so a constraint
/// class added upstream is reported rather than drifting from a second copy here. A
/// class whose clause the framework words differently enough that the requirement cannot
/// be read off it answers `None`, and only THAT clause falls back to the framework's own
/// sentence (see [`envelope`]); the rest of the refusal is still worded here.
fn requirement(kind: &ViolationKind) -> Option<String> {
    match kind {
        ViolationKind::Pattern(pattern) => Some(format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        )),
        kind => framework_requirement(kind),
    }
}

/// The framework's own wording for `kind`'s requirement.
///
/// Read off a clause the framework builds for a violation of that kind with NO value
/// attached: a clause carrying the value would put caller-controlled text before the
/// marker, so a value containing the marker itself would split inside the value.
fn framework_requirement(kind: &ViolationKind) -> Option<String> {
    let clause = Violation {
        path: String::new(),
        kind: kind.clone(),
        value: None,
        sensitive: false,
    }
    .message();
    clause
        .split_once(CONSTRAINT_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// `path` with every collection step walked out of, and how deep that nesting is stated.
///
/// The path is collapsed all the way to the member, but the DEPTH is clamped to
/// [`MAX_DEPTH`]: the depth is derived from a path whose entry steps carry a
/// caller-chosen map key, so without a ceiling a key spelling `.<digits>.member` would
/// let the request choose how much nesting the clause renders.
fn collapsed(path: &str) -> (&str, usize) {
    let mut path = path;
    let mut depth = 0;
    // The element step is spelled `<collection>.<1-based position>.member`, which
    // `list_of_element` is the one place in this tree that recognises.
    while let Some(collection) = list_of_element(path) {
        path = collection;
        depth += 1;
    }
    (path, depth.min(MAX_DEPTH))
}

/// `requirement` wrapped in the collection-constraint clause once per collection step,
/// built in ONE allocation rather than re-formatting the string per step.
fn nested(requirement: &str, depth: usize) -> String {
    /// The clause a collection states around its element's requirement.
    const OPEN: &str = "Member must satisfy constraint: [";
    if depth == 0 {
        return requirement.to_string();
    }
    let mut nested = String::with_capacity(requirement.len() + depth * (OPEN.len() + 1));
    for _ in 0..depth {
        nested.push_str(OPEN);
    }
    nested.push_str(requirement);
    for _ in 0..depth {
        nested.push(']');
    }
    nested
}

/// The largest number of clauses one refusal RENDERS.
///
/// The violation count is REQUEST-SHAPED — a caller chooses how many members and how
/// many elements of a collection to make invalid — so the refusal's size is bounded here
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
/// indistinguishable from a list index by the path alone: the nesting a request can ask
/// for would otherwise grow with the length of that key. This service's inputs nest one
/// collection deep (`SourceRegions`, `SubscriptionProviderSources`, `tagKeys`, `tags`),
/// so the ceiling is set one past that — enough to state a list of lists, were the model
/// to grow one, and no more.
const MAX_DEPTH: usize = 2;

/// One clause of the envelope, accumulated as the violations are walked.
///
/// A clause is keyed by the member path it is reported at and the requirement it states,
/// so the elements of ONE collection failing ONE constraint accumulate into a single
/// clause in a single pass — no violation is re-scanned for another.
#[derive(Debug)]
struct Clause {
    /// The member path the clause is reported at: the collection, for an element
    /// violation.
    path: String,
    /// The requirement, already nested once per collection step walked out of.
    requirement: String,
    /// How many collection steps were walked out of: 0 for a member reported on its own.
    depth: usize,
    /// The offending values, in walk order, capped at [`MAX_RENDERED_ELEMENTS`].
    values: Vec<String>,
    /// Whether the first violation of this clause carried no value — an absent member,
    /// which is reported as the `null` it was.
    absent: bool,
    /// The framework's own clause, kept verbatim for a violation class this module has
    /// no wording for, so one such class does not cost the refusal its other clauses.
    verbatim: Option<String>,
}

impl Clause {
    /// The clause as the envelope states it.
    fn render(&self) -> String {
        if let Some(verbatim) = &self.verbatim {
            return verbatim.clone();
        }
        let subject = if self.absent {
            "Value null".to_string()
        } else if self.depth == 0 {
            format!(
                "Value '{}'",
                self.values.first().map(String::as_str).unwrap_or_default()
            )
        } else {
            // The structured refusal carries the offending ELEMENTS, not the collection
            // they came from, so the collection rendered here is the one formed from
            // them — one bracket per collection step.
            format!(
                "Value '{}{}{}'",
                "[".repeat(self.depth),
                self.values.join(", "),
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
/// (`N validation error(s) detected: `) over one clause per offending member, in the
/// order the walk found them.
///
/// ONE pass over the violations, so the cost is linear in their number: each violation is
/// grouped by the clause it belongs to (its member path and requirement) as it is walked,
/// which is also what collapses the elements of one collection onto one clause — the
/// member is named once however many of its elements were refused. The COUNT the envelope
/// states is the real number of distinct clauses the request produced; what is bounded is
/// how much of it is RENDERED ([`MAX_CLAUSES`] clauses, [`MAX_RENDERED_ELEMENTS`] element
/// values each), because the violation count is chosen by the request.
///
/// A violation of a class this module has no wording for falls back to the FRAMEWORK's
/// own clause for that one violation, so one unworded class (a `#[non_exhaustive]`
/// `ViolationKind` added upstream, say) cannot cost the rest of the refusal its wording.
/// `None` only when there is nothing to state.
pub(crate) fn envelope(violations: &[Violation]) -> Option<String> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut clauses: HashMap<(String, String), Clause> = HashMap::new();
    // Every distinct clause the request produced, including the ones past the render cap:
    // the count AWS states is a fact about the request, so it is not truncated.
    let mut distinct: HashSet<(String, String)> = HashSet::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        // Nest the requirement once per collection step: a list of lists nests twice,
        // for the same reason it nests once. The depth is already clamped by
        // `collapsed`, so a caller cannot choose how much nesting this builds.
        let (requirement, verbatim) = match requirement(&violation.kind) {
            Some(requirement) => (nested(&requirement, depth), None),
            // Nothing to word this class with: keep the framework's own sentence for
            // THIS clause, and leave the others as this module words them.
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
                    absent: violation.value.is_none(),
                    verbatim,
                })
            }
        };
        if let Some(value) = violation.value.as_deref() {
            if clause.values.len() < MAX_RENDERED_ELEMENTS {
                clause.values.push(value.to_string());
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

/// The tagging surface's own refusal for `violations`, or `None` when none of them is a
/// refusal that surface states itself.
///
/// The FIRST violation decides, because it is the first thing the walk found wrong and
/// the tag APIs state one refusal rather than an envelope of clauses. THREE classes are
/// the surface's own, each with the sentence `crate::scaffolded` states it with (so a
/// handler reaching the same refusal from inside the operation answers identically) or,
/// for the third, with the parameters the request has to add: a value outside the
/// `resourceArn` label's grammar, a tag selection past its size limit, and a required
/// REQUEST PARAMETER the request does not carry. Every other class is the service's
/// modelled constraint envelope ([`refused`]), which is what this service's models
/// declare for a value they refuse.
fn tag_api_refusal(violations: &[Violation]) -> Option<Refused> {
    const INVALID_ARN: UnmodeledRefusal = UnmodeledRefusal::invalid_resource_arn();
    const TAG_LIMIT: UnmodeledRefusal = UnmodeledRefusal::tag_limit_exceeded();

    let first = violations.first()?;
    let (path, depth) = collapsed(&first.path);
    // Both of the surface's VALUE refusals are about the MEMBER itself — the label's own
    // grammar, and the SIZE of the tag selection — so a violation reached THROUGH a
    // collection step is not one of them, whatever member it sits under. That is also
    // what keeps a numeric tag KEY from being read as a tag-count violation: the
    // framework spells a map entry `<key>.member`, exactly as it spells a list element,
    // so a key of digits only is indistinguishable from an index by the path alone — but
    // a per-key, per-value or per-element `@length` on such an entry is not the
    // collection's own `@length`, and this check is what separates them.
    if depth != 0 {
        return None;
    }
    if path == RESOURCE_ARN_MEMBER {
        return Some(stated(&INVALID_ARN));
    }
    if !TAG_SELECTION_MEMBERS.contains(&path) {
        return None;
    }
    match first.kind {
        // The selection is past the size a resource's tag set may reach.
        ViolationKind::MaxLength(_) => Some(stated(&TAG_LIMIT)),
        // A required REQUEST PARAMETER is absent, so the request does not name what to
        // remove at all. `None` when the absent member travels in the BODY instead: a
        // payload that omits a required member is not a missing request parameter, and
        // the operations that carry a tag selection in their body do declare the
        // service's modelled constraint error for it.
        ViolationKind::MissingRequiredMember => {
            absent_request_parameters(violations).map(|message| Refused {
                code: BAD_REQUEST_CODE.to_string(),
                message,
            })
        }
        // Every other class is the service's modelled constraint envelope.
        _ => None,
    }
}

/// `refusal` as the answer the framework renders.
fn stated(refusal: &UnmodeledRefusal) -> Refused {
    Refused {
        code: refusal.code.to_string(),
        message: refusal.message.to_string(),
    }
}

/// The sentence stating the required REQUEST PARAMETERS `violations` reports absent, or
/// `None` when it reports none.
///
/// The fixed half states the condition; the variable half is the list of parameter NAMES
/// the request left out — in walk order, without repeats — because that is what tells the
/// caller what to add. Only a member that travels AS a request parameter is named, and
/// which members those are is read off the model by [`super::tag_request_parameters`]
/// rather than written out here: a body member absent from a payload is not a missing
/// request parameter.
fn absent_request_parameters(violations: &[Violation]) -> Option<String> {
    let mut named: Vec<&str> = Vec::new();
    for violation in violations {
        if !matches!(violation.kind, ViolationKind::MissingRequiredMember) {
            continue;
        }
        let (path, depth) = collapsed(&violation.path);
        if depth != 0 {
            continue;
        }
        let parameter = super::tag_request_parameters()
            .find(|(member, _)| member == path)
            .map(|(_, parameter)| parameter);
        if let Some(parameter) = parameter {
            if !named.contains(&parameter) {
                named.push(parameter);
            }
        }
    }
    if named.is_empty() {
        return None;
    }
    Some(format!(
        "Missing required request parameters: [{}]",
        named.join(", ")
    ))
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a
/// different failure with a different code and status, and nothing about it is this
/// service's to word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => match tag_api_refusal(violations) {
            Some(tag_refusal) => Some(tag_refusal),
            None => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: envelope(violations)?,
            }),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchor PAIR is stripped, and only that pair: a `$` that is part of the
    /// expression stays, and so does an expression anchored at one end only. The two
    /// forms this service's own model reaches are covered end-to-end by
    /// `tests/all/error_taxonomy.rs`; these rows pin the branches no input shape of THIS
    /// model can produce, so the rule this service states stays stated for a model that
    /// grows one. The rule itself is `service_common::validation::unanchored`'s, the one
    /// implementation of it in this tree, which these rows read it from.
    #[test]
    fn only_the_anchors_are_stripped_from_a_pattern() {
        for (pattern, expected) in [
            (r"^[a-z]+$", "[a-z]+"),
            (r"[a-z]+", "[a-z]+"),
            (r"^\$[0-9]+$", r"\$[0-9]+"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            (r"^[a-z]\\$", r"[a-z]\\"),
            // The pair is dropped POSITIONALLY and only as a PAIR: an expression whose
            // trailing `$` is ESCAPED carries no end anchor, so nothing is dropped and
            // the expression is quoted as the model spells it — and the same for one
            // anchored at one end only.
            (r"^\$", r"^\$"),
            (r"^", "^"),
            (r"[0-9]+$", r"[0-9]+$"),
        ] {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Which paths the shared element-path recognition
    /// (`service_common::validation::list_of_element`) reports a collection for, as this
    /// service's clause rule 4 reads it.
    #[test]
    fn only_an_indexed_step_is_a_collection_element() {
        for (path, expected) in [
            ("subscriptionProviderSources.1.member", Some("subscriptionProviderSources")),
            ("sourceRegions.12.member", Some("sourceRegions")),
            // A list of lists: one step out at a time.
            ("outer.2.member.3.member", Some("outer.2.member")),
            // Not an element: a map entry, a member named `member`, a plain member, and
            // a member inside a structure element. A map entry whose KEY is digits only
            // (`tags.2024.member`) is indistinguishable from a list element by the path
            // alone — see `tag_api_refusal`, which is where that matters.
            ("tags.someKey.member", None),
            ("filters.1.member.member", None),
            ("maxResults", None),
            ("filters.1.member.name", None),
        ] {
            let actual = list_of_element(path);
            assert_eq!(
                actual, expected,
                "case {path:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The nesting a clause states is bounded too, and built in one allocation: a map
    /// entry's step carries the caller's KEY, so a key spelling `.<digits>.member` must
    /// not let the request choose how deep the clause nests.
    #[test]
    fn the_nesting_a_clause_states_is_bounded() {
        // A key that spells many collection steps: the depth stops at MAX_DEPTH.
        let deep = format!("tags{}", ".1.member".repeat(40));
        let (path, depth) = collapsed(&deep);
        assert_eq!(
            depth, MAX_DEPTH,
            "case {deep:?}: expected the depth clamped to {MAX_DEPTH}, got {depth}"
        );
        assert_eq!(path, "tags", "case {deep:?}: expected the member, got {path:?}");

        // And the nesting the clause renders follows that bound.
        let violation = Violation {
            path: deep.clone(),
            kind: ViolationKind::MaxLength(1),
            value: Some("x".to_string()),
            sensitive: false,
        };
        let message = envelope(std::slice::from_ref(&violation))
            .expect("a length violation is a class this module words");
        let opens = message.matches("Member must satisfy constraint: [").count();
        assert_eq!(
            opens, MAX_DEPTH,
            "case {deep:?}: expected {MAX_DEPTH} nested constraints, got {opens} in {message:?}"
        );
    }

    /// The tag APIs' own refusals are about the MEMBER — the label's grammar and the size
    /// of a tag selection — so a violation reached through a collection step never takes
    /// them, which is what keeps a per-entry `@length` on a numeric tag key from being
    /// answered as the tag-COUNT refusal.
    #[test]
    fn only_a_violation_on_the_member_itself_takes_a_tag_api_refusal() {
        let cases = [
            // The tag map's own `@length`: the tag-count refusal.
            ("tags", true),
            // The tag-key list's own `@length`: the same refusal, same taxonomy.
            ("tagKeys", true),
            // A per-entry `@length` on an entry whose key happens to be digits only.
            ("tags.2024.member", false),
            // A per-entry `@length` on an ordinary key.
            ("tags.owner.member", false),
            // A per-ELEMENT constraint on one tag key, which is not the list's size.
            ("tagKeys.1.member", false),
        ];
        for (path, expects_tag_limit) in cases {
            let violation = Violation {
                path: path.to_string(),
                kind: ViolationKind::MaxLength(50),
                value: Some("x".to_string()),
                sensitive: false,
            };
            let refusal = tag_api_refusal(std::slice::from_ref(&violation));
            assert_eq!(
                refusal.is_some(),
                expects_tag_limit,
                "case {path:?}: expected a tag-API refusal: {expects_tag_limit}, got {refusal:?}"
            );
        }
    }

    /// Which classes of refusal on a member of the tagging surface take that surface's
    /// own code, and which keep the service's modelled constraint envelope: a value
    /// outside the label's grammar, a selection past its size, and a required REQUEST
    /// PARAMETER the request does not carry are the surface's; a required member of a
    /// request BODY, and every other class, are the modelled envelope's.
    #[test]
    fn only_the_surfaces_own_classes_take_the_surfaces_code() {
        // (member, class, the code the refusal names)
        let cases: &[(&str, ViolationKind, &str)] = &[
            (
                RESOURCE_ARN_MEMBER,
                ViolationKind::Pattern("^arn:.*$".to_string()),
                BAD_REQUEST_CODE,
            ),
            ("tagKeys", ViolationKind::MaxLength(50), BAD_REQUEST_CODE),
            ("tags", ViolationKind::MaxLength(50), BAD_REQUEST_CODE),
            // The `@httpQuery` parameter of the removal: the surface's.
            (
                "tagKeys",
                ViolationKind::MissingRequiredMember,
                BAD_REQUEST_CODE,
            ),
            // The required BODY member of the write: the modelled envelope's.
            (
                "tags",
                ViolationKind::MissingRequiredMember,
                VALIDATION_EXCEPTION,
            ),
            // A class no model of this surface reaches, had it one: the envelope's.
            ("tagKeys", ViolationKind::UniqueItems, VALIDATION_EXCEPTION),
            // And a member of another surface entirely.
            (
                "subscriptionProviderSources",
                ViolationKind::EnumValue(vec!["RedHat".to_string()]),
                VALIDATION_EXCEPTION,
            ),
        ];
        for (member, kind, expected) in cases {
            let violation = Violation {
                path: (*member).to_string(),
                kind: kind.clone(),
                value: None,
                sensitive: false,
            };
            let refused = refused(&Refusal::Invalid(vec![violation]))
                .unwrap_or_else(|| panic!("case {member} {kind:?}: nothing was stated"));
            assert_eq!(
                refused.code, *expected,
                "case {member} {kind:?}: expected the code {expected}, got {:?}",
                refused.code
            );
            assert!(
                !refused.message.is_empty(),
                "case {member} {kind:?}: the refusal must state what was wrong"
            );
        }
    }

    /// The parameters a request has to add are named from the members the walk reports
    /// absent — the parameter's wire name, in walk order, without repeats — and a member
    /// that travels in the BODY is not one of them.
    #[test]
    fn only_absent_request_parameters_are_named() {
        let absent = |path: &str| Violation {
            path: path.to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        let cases: &[(&[&str], Option<&str>)] = &[
            (
                &["tagKeys"],
                Some("Missing required request parameters: [tagKeys]"),
            ),
            (
                &["tagKeys", "tagKeys"],
                Some("Missing required request parameters: [tagKeys]"),
            ),
            // A required member of a request BODY is absent from a payload, not from the
            // request parameters.
            (&["tags"], None),
            (&["secretArn"], None),
            (&[], None),
        ];
        for (paths, expected) in cases {
            let violations: Vec<Violation> = paths.iter().map(|path| absent(path)).collect();
            let actual = absent_request_parameters(&violations);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {paths:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A class this module does not reword keeps the framework's own wording, rather
    /// than a second copy of it: the clause this module builds for such a violation and
    /// the clause the framework builds for the same violation state the same
    /// requirement.
    #[test]
    fn an_unrewarded_class_keeps_the_frameworks_own_requirement() {
        for kind in [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxLength(50),
            ViolationKind::MinValue("1".to_string()),
            ViolationKind::MaxValue("100".to_string()),
            ViolationKind::UniqueItems,
            ViolationKind::EnumValue(vec!["RedHat".to_string()]),
        ] {
            let violation = Violation {
                path: "someMember".to_string(),
                kind: kind.clone(),
                value: None,
                sensitive: false,
            };
            let ours = envelope(std::slice::from_ref(&violation))
                .unwrap_or_else(|| panic!("case {kind:?}: no envelope was produced"));
            let expected = format!("1 validation error detected: {}", violation.message());
            assert_eq!(
                ours, expected,
                "case {kind:?}: expected the framework's own clause, got {ours:?}"
            );
        }
    }

    /// The formatter is LINEAR in the violation count and its RENDERED output is
    /// bounded: a request that makes a whole collection invalid must not cost more than
    /// a pass over it, nor render every offending element into the response — while the
    /// COUNT the envelope states stays the real number of distinct clauses.
    #[test]
    fn a_large_refusal_is_bounded_in_clauses_and_in_rendered_elements() {
        // One collection with far more failing elements than the render cap: one clause,
        // at most MAX_RENDERED_ELEMENTS values in it.
        let elements: Vec<Violation> = (1..=500)
            .map(|position| Violation {
                path: format!("subscriptionProviderSources.{position}.member"),
                kind: ViolationKind::EnumValue(vec!["RedHat".to_string()]),
                value: Some(format!("bad-{position}")),
                sensitive: false,
            })
            .collect();
        let message =
            envelope(&elements).expect("an enum violation is a class this module words");
        assert!(
            message.starts_with("1 validation error detected: Value '[bad-1, "),
            "one collection must state one clause, got {message:?}"
        );
        let rendered = message.matches("bad-").count();
        assert_eq!(
            rendered, MAX_RENDERED_ELEMENTS,
            "at most {MAX_RENDERED_ELEMENTS} elements may be rendered, got {rendered}"
        );

        // More distinct offending members than the render cap: the COUNT is the real
        // number of clauses the request produced, and only the first MAX_CLAUSES are
        // spelled out, so the message cannot be sized by the request.
        let members: Vec<Violation> = (1..=500)
            .map(|index| Violation {
                path: format!("member{index}"),
                kind: ViolationKind::MaxLength(1),
                value: Some("too-long".to_string()),
                sensitive: false,
            })
            .collect();
        let message = envelope(&members).expect("a length violation is worded here");
        assert!(
            message.starts_with("500 validation errors detected: "),
            "the count must be the real number of clauses, got {message:?}"
        );
        let spelled = message.matches("failed to satisfy constraint:").count();
        assert_eq!(
            spelled, MAX_CLAUSES,
            "at most {MAX_CLAUSES} clauses may be spelled out, got {spelled}"
        );
    }
}

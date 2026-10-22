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
//!    `UntagResource` does not even model. A tag set past its modelled size limit is
//!    refused with `BadRequestException` too, wherever that tag map appears
//!    (`RegisterSubscriptionProvider`'s `Tags` as much as `TagResource`'s `tags`). Both
//!    are the refusals `crate::scaffolded` states, so the EXISTENCE stage a handler
//!    answers (`NotFoundException`) sits behind the same taxonomy.
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
//! MIGRATION NOTE (`services/common/src/validation.rs`): the three rules that are NOT a
//! per-service dialect — the requirement half read off the framework's own wording, the
//! subject half, and a `@pattern` printed without its anchors — are staged in that
//! module and inlined here as they are in ~24 other service crates, because moving them
//! is a change of its own rather than a side effect of this service's issue. This copy
//! is one of the ones that migration has to collect; what is genuinely this service's
//! own and stays here is the tag-API taxonomy and the collection-value rendering.

use std::collections::{HashMap, HashSet};

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

use crate::scaffolded::UnmodeledRefusal;

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

/// The member a tag SET travels in: the `Tags` map of `RegisterSubscriptionProvider` and
/// the `tags` map of `TagResource`, both `@length 0..50`. Both spell the same lowerCamel
/// path, so one name covers them.
const TAG_SET_MEMBER: &str = "tags";

/// The suffix the framework's validation path carries for the step from a collection
/// into one of its elements: `path::element` spells an element as its 1-based position
/// followed by this segment (`subscriptionProviderSources.1.member`).
const COLLECTION_STEP: &str = ".member";

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
    }
    .message();
    clause
        .split_once(CONSTRAINT_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// A `@pattern` expression without the anchors the model spells it with.
///
/// A `$` preceded by an odd number of backslashes is an ESCAPED dollar sign — a literal
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

/// The path of the collection a violation's path points INTO, when the violation is on
/// one of that collection's elements.
///
/// An element path ends with the collection step preceded by the element's 1-based
/// position (`subscriptionProviderSources.1.member` -> `subscriptionProviderSources`).
/// The position is what identifies the step, so a member that happens to be NAMED
/// `member` (`filters.1.member.member`) is not mistaken for one.
fn enclosing_collection(path: &str) -> Option<&str> {
    let (head, position) = path.strip_suffix(COLLECTION_STEP)?.rsplit_once('.')?;
    if position.is_empty() || !position.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(head)
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
    while let Some(collection) = enclosing_collection(path) {
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

/// The tagging surface's own refusal for `violations`, or `None` when none of them is on
/// that surface.
///
/// The FIRST violation decides, because it is the first thing the walk found wrong and
/// the tag APIs state one refusal rather than an envelope of clauses. Both refusals are
/// the constants `crate::scaffolded` states them with, so a handler reaching the same
/// refusal from inside the operation answers identically.
fn tag_api_refusal(violations: &[Violation]) -> Option<&'static UnmodeledRefusal> {
    const INVALID_ARN: UnmodeledRefusal = UnmodeledRefusal::invalid_resource_arn();
    const TAG_LIMIT: UnmodeledRefusal = UnmodeledRefusal::tag_limit_exceeded();

    let first = violations.first()?;
    let (path, depth) = collapsed(&first.path);
    // Both refusals are about the MEMBER itself — the label's own grammar, and the SIZE
    // of the tag map — so a violation reached THROUGH a collection step is not one of
    // them, whatever member it sits under. That is also what keeps a numeric tag KEY
    // from being read as a tag-count violation: the framework spells a map entry
    // `<key>.member`, exactly as it spells a list element, so a key of digits only is
    // indistinguishable from an index by the path alone — but a per-key or per-value
    // `@length` on such an entry is not the map's own `@length`, and this check is what
    // separates them.
    if depth != 0 {
        return None;
    }
    if path == RESOURCE_ARN_MEMBER {
        return Some(&INVALID_ARN);
    }
    if path == TAG_SET_MEMBER && matches!(first.kind, ViolationKind::MaxLength(_)) {
        return Some(&TAG_LIMIT);
    }
    None
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a
/// different failure with a different code and status, and nothing about it is this
/// service's to word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => match tag_api_refusal(violations) {
            Some(tag_refusal) => Some(Refused {
                code: tag_refusal.code.to_string(),
                message: tag_refusal.message.to_string(),
            }),
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

    /// The anchors are stripped, and only the anchors: a `$` that is part of the
    /// expression stays. The two forms this service's own model reaches are covered
    /// end-to-end by `tests/all/error_taxonomy.rs`; these rows pin the branches no input
    /// shape of THIS model can produce, so the rule stays stated for a model that grows
    /// one.
    #[test]
    fn only_the_anchors_are_stripped_from_a_pattern() {
        for (pattern, expected) in [
            (r"^[a-z]+$", "[a-z]+"),
            (r"[a-z]+", "[a-z]+"),
            (r"^\$[0-9]+$", r"\$[0-9]+"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            (r"^[a-z]\\$", r"[a-z]\\"),
            (r"^\$", r"\$"),
            (r"^", ""),
        ] {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

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
            let actual = enclosing_collection(path);
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
        };
        let message = envelope(std::slice::from_ref(&violation))
            .expect("a length violation is a class this module words");
        let opens = message.matches("Member must satisfy constraint: [").count();
        assert_eq!(
            opens, MAX_DEPTH,
            "case {deep:?}: expected {MAX_DEPTH} nested constraints, got {opens} in {message:?}"
        );
    }

    /// The tag APIs' own refusals are about the MEMBER — the label's grammar and the tag
    /// map's size — so a violation reached through a collection step never takes them,
    /// which is what keeps a per-entry `@length` on a numeric tag key from being
    /// answered as the tag-COUNT refusal.
    #[test]
    fn only_a_violation_on_the_member_itself_takes_a_tag_api_refusal() {
        let cases = [
            // The tag map's own `@length`: the tag-count refusal.
            ("tags", true),
            // A per-entry `@length` on an entry whose key happens to be digits only.
            ("tags.2024.member", false),
            // A per-entry `@length` on an ordinary key.
            ("tags.owner.member", false),
        ];
        for (path, expects_tag_limit) in cases {
            let violation = Violation {
                path: path.to_string(),
                kind: ViolationKind::MaxLength(50),
                value: Some("x".to_string()),
            };
            let refusal = tag_api_refusal(std::slice::from_ref(&violation));
            assert_eq!(
                refusal.is_some(),
                expects_tag_limit,
                "case {path:?}: expected a tag-API refusal: {expects_tag_limit}, got {refusal:?}"
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

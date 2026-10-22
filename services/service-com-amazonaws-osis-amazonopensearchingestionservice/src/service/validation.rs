// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The framework walks the modelled constraints and hands [`Refusal`] to
//! `service.rs` structurally — one [`Violation`] per offending value, with the
//! path it sits at, what it failed and what it held — and leaves the wording to
//! the service. For every class but one, the framework's own clause is already
//! what AWS answers, so this module declines it (`None`) and the default stands.
//!
//! The one class it rewords is a member that is a **collection of constrained
//! scalars**. AWS reports such a member ONCE, not once per offending element:
//!
//! * the entry names the COLLECTION member's own path — no element index and no
//!   `member` step (`vpcOptions.subnetIds`, not `vpcOptions.subnetIds.1.member`);
//! * the refused values are rendered as a bracketed list (`'[a, b]'`), so one
//!   entry covers however many elements failed;
//! * the requirement enumerates EVERY constraint the ELEMENT shape declares —
//!   length maximum, then length minimum, then pattern, then enum value set —
//!   whichever of them this request happened to fail, inside
//!   `Member must satisfy constraint: [ … ]`;
//! * a pattern is stated in its unanchored form (see [`unanchored`]);
//! * the entries of the message are ordered by path.
//!
//! The constraints an entry enumerates are read from this service's own Smithy
//! model (the same file the generated schemas are built from), so the rule holds
//! for every constrained collection member of every operation in this service,
//! not for the ones that happened to be observed. Nothing is keyed to an
//! operation name, a member name or a value.
//!
//! One thing this seam cannot see: the framework hands over the values it
//! REFUSED, not the member's whole value, so a list of which only some elements
//! were refused renders the refused ones. There is no request and no input value
//! here to read the accepted elements from.

use std::collections::HashMap;
use std::sync::LazyLock;

use aws_smithy_schema_server::constraints::{path, Violation, ViolationKind};
use serde_json::Value;
use smithy_server_runtime::{Refusal, Refused};

/// This service's Smithy model: the one source for the constraints an entry
/// enumerates, including the ones the request did not fail (the framework
/// reports only what failed, and AWS states the member's whole contract).
const MODEL_JSON: &str =
    include_str!("../../smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json");

/// The wire error code for a refused request on this service — the code the
/// framework's own default names too, kept explicit because the seam replaces
/// code and message together and a client branches on the code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The separator between a clause's `Value '…' at '…'` half and its requirement
/// half, exactly as [`Violation::message`] writes it.
const REQUIREMENT_PREFIX: &str = "failed to satisfy constraint: ";

/// What the ELEMENT shape of one collection member must satisfy, read from the
/// model. `None` fields are constraints the element shape does not declare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ElementRules {
    /// Whether the member is a list. A map is NOT rendered as a bracketed list
    /// of values, so its entries keep the framework's per-entry clause.
    list: bool,
    min_length: Option<i64>,
    max_length: Option<i64>,
    pattern: Option<String>,
    enum_values: Option<Vec<String>>,
}

impl ElementRules {
    /// True when the element shape declares no constraint at all, in which case
    /// there is nothing to enumerate and nothing to index.
    fn is_empty(&self) -> bool {
        self.min_length.is_none()
            && self.max_length.is_none()
            && self.pattern.is_none()
            && self.enum_values.is_none()
    }

    /// Whether this element shape is the one that produced `kind` — the index
    /// and the walk must be describing the same member before an entry speaks
    /// for a violation. A violation these rules cannot account for keeps the
    /// framework's clause rather than being folded into an entry that would
    /// omit it.
    fn covers(&self, kind: &ViolationKind) -> bool {
        match kind {
            ViolationKind::MaxLength(max) => self.max_length == Some(*max),
            ViolationKind::MinLength(min) => self.min_length == Some(*min),
            ViolationKind::Pattern(pattern) => self.pattern.as_deref() == Some(pattern.as_str()),
            // A value SET is the same set whatever order the two sides list it
            // in: the model's member order and the walk's are both orders of
            // one set, so comparing them as sequences would miss a match.
            ViolationKind::EnumValue(values) => self.enum_values.as_ref().is_some_and(|mine| {
                let (mut mine, mut theirs) = (mine.clone(), values.clone());
                mine.sort();
                theirs.sort();
                mine == theirs
            }),
            _ => false,
        }
    }

    /// Every constraint the element shape declares, worded as the framework
    /// words that constraint, in the order AWS enumerates them: length maximum,
    /// length minimum, pattern, enum value set.
    ///
    /// `None` when any of them cannot be worded (see [`requirement_of`]), so a
    /// half-stated contract is never emitted.
    fn requirements(&self) -> Option<Vec<String>> {
        let mut out = Vec::new();
        if let Some(max) = self.max_length {
            out.push(requirement_of(ViolationKind::MaxLength(max))?);
        }
        if let Some(min) = self.min_length {
            out.push(requirement_of(ViolationKind::MinLength(min))?);
        }
        if let Some(pattern) = &self.pattern {
            out.push(requirement_of(ViolationKind::Pattern(unanchored(pattern)))?);
        }
        if let Some(values) = &self.enum_values {
            out.push(requirement_of(ViolationKind::EnumValue(values.clone()))?);
        }
        Some(out)
    }
}

/// The requirement half of the clause the framework writes for `kind`, asked of
/// the framework itself so the wording of a constraint stays single-homed there
/// and cannot drift from the clause a non-collection member still gets.
///
/// The framework renders the whole clause and keeps the requirement half
/// private (`Violation::requirement`), and `vendor/` is read-only from here, so
/// the half is recovered from the sentence the framework wrote. `None` when the
/// sentence does not carry the separator — i.e. the framework reworded it —
/// which makes the caller fall back to the framework's own clauses rather than
/// fold a mis-parsed sentence into a message. `the_framework_still_separates_
/// the_requirement` pins the separator so such a change fails a test instead.
fn requirement_of(kind: ViolationKind) -> Option<String> {
    let clause = Violation {
        path: String::new(),
        kind,
        value: None,
    }
    .message();
    clause
        .split_once(REQUIREMENT_PREFIX)
        .map(|(_, requirement)| requirement.to_string())
}

/// A pattern as AWS states it in a validation message: without the `^`/`$`
/// anchors the Smithy trait carries and with a trailing `.*`.
///
/// Smithy `@pattern` semantics are a find, not a full match, so the anchors are
/// the model's way of spelling "the whole value"; the service states the same
/// requirement in its own unanchored form. The value under test is unaffected —
/// this is rendering only, and the framework still enforces the anchored regex.
fn unanchored(pattern: &str) -> String {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    let body = body.strip_suffix('$').unwrap_or(body);
    if body.ends_with(".*") {
        body.to_string()
    } else {
        format!("{body}.*")
    }
}

/// What the index knows about one member path.
///
/// A path is reported by the walk without naming the operation it came from, so
/// two operations whose inputs expose the SAME path with different element
/// shapes cannot be told apart here. Rather than let one operation's bounds
/// speak for the other's request, such a path is marked [`Indexed::Conflicting`]
/// and keeps the framework's per-element clause.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Indexed {
    /// Every operation that exposes this path agrees on the element shape.
    Unique(ElementRules),
    /// Two operations disagree about it; no entry may speak for it.
    Conflicting,
}

/// Every constrained collection member reachable from an operation input,
/// keyed by the dotted lowerCamel path the validating walk reports it at.
static ELEMENT_RULES: LazyLock<HashMap<String, Indexed>> = LazyLock::new(build_element_rules);

/// Read the model once and index every collection member whose element shape
/// carries constraints.
///
/// Rooted at each operation's input shape, because that is where a validation
/// path is rooted. A collection nested inside a collection's element structure
/// is not indexed: its elements are refused at a deeper path than an entry
/// could speak for, and they keep the framework's clause.
fn build_element_rules() -> HashMap<String, Indexed> {
    let mut out = HashMap::new();
    let Ok(model) = serde_json::from_str::<Value>(MODEL_JSON) else {
        return out;
    };
    let Some(shapes) = model.get("shapes").and_then(Value::as_object) else {
        return out;
    };
    for shape in shapes.values() {
        if shape.get("type").and_then(Value::as_str) != Some("operation") {
            continue;
        }
        let Some(input) = shape
            .get("input")
            .and_then(|input| input.get("target"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        index_structure(shapes, input, "", &mut Vec::new(), &mut out);
    }
    out
}

/// Index the collection members of `shape_id`, descending through structure and
/// union members. `open` carries the shapes already on this path, so a
/// recursive shape is walked once rather than forever.
fn index_structure(
    shapes: &serde_json::Map<String, Value>,
    shape_id: &str,
    prefix: &str,
    open: &mut Vec<String>,
    out: &mut HashMap<String, Indexed>,
) {
    if open.iter().any(|seen| seen == shape_id) {
        return;
    }
    open.push(shape_id.to_string());
    if let Some(members) = shapes
        .get(shape_id)
        .and_then(|shape| shape.get("members"))
        .and_then(Value::as_object)
    {
        for (name, member) in members {
            // The walk spells a member in lowerCamel; ask it how, rather than
            // keeping a second copy of that casing rule here.
            let segment = path::member(name);
            let member_path = if prefix.is_empty() {
                segment
            } else {
                format!("{prefix}.{segment}")
            };
            let Some(target) = member.get("target").and_then(Value::as_str) else {
                continue;
            };
            match shapes
                .get(target)
                .and_then(|shape| shape.get("type"))
                .and_then(Value::as_str)
            {
                Some("structure" | "union") => {
                    index_structure(shapes, target, &member_path, open, out);
                }
                Some("list" | "map") => {
                    if let Some(rules) = element_rules(shapes, target) {
                        match out.get(&member_path) {
                            None => {
                                out.insert(member_path, Indexed::Unique(rules));
                            }
                            // Another operation already indexed this path with
                            // the same element shape — nothing changes.
                            Some(Indexed::Unique(seen)) if *seen == rules => {}
                            Some(_) => {
                                out.insert(member_path, Indexed::Conflicting);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    open.pop();
}

/// The constraints the scalar leaf of collection shape `collection` declares.
///
/// Descends every nested collection level, because the walk enforces the SCALAR
/// leaf's constraints (an intermediate collection's `@length` bounds an element
/// count, not a value), and reads them off the leaf SHAPE — which is the source
/// codegen captures for the walk, so the enumeration cannot claim a constraint
/// the request was never checked against.
fn element_rules(
    shapes: &serde_json::Map<String, Value>,
    collection: &str,
) -> Option<ElementRules> {
    let shape = shapes.get(collection)?;
    let list = match shape.get("type").and_then(Value::as_str)? {
        "list" => true,
        "map" => false,
        _ => return None,
    };
    let element = if list {
        shape.get("member")?
    } else {
        shape.get("value")?
    };
    let mut target = element.get("target").and_then(Value::as_str)?;
    // Descend to the scalar leaf. `seen` bounds the descent: a model may declare
    // a collection that reaches itself, and this walk must end either way.
    let mut seen = vec![target];
    while let Some(inner) = shapes
        .get(target)
        .and_then(|shape| match shape.get("type").and_then(Value::as_str) {
            Some("list") => shape.get("member"),
            Some("map") => shape.get("value"),
            _ => None,
        })
        .and_then(|member| member.get("target"))
        .and_then(Value::as_str)
    {
        if seen.contains(&inner) {
            return None;
        }
        seen.push(inner);
        target = inner;
    }
    let leaf = shapes.get(target)?;
    let mut rules = ElementRules {
        list,
        ..ElementRules::default()
    };
    if let Some(traits) = leaf.get("traits") {
        if let Some(length) = traits.get("smithy.api#length") {
            rules.min_length = length.get("min").and_then(Value::as_i64);
            rules.max_length = length.get("max").and_then(Value::as_i64);
        }
        rules.pattern = traits
            .get("smithy.api#pattern")
            .and_then(Value::as_str)
            .map(str::to_string);
        // The older string-shape form of an enumerated value set.
        rules.enum_values = traits
            .get("smithy.api#enum")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("value").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            });
    }
    if leaf.get("type").and_then(Value::as_str) == Some("enum") {
        // An `enum` shape carries its value set in its MEMBERS: the member's
        // `@enumValue` is what goes on the wire, the member name only a
        // fallback for a member that declares none.
        rules.enum_values = leaf
            .get("members")
            .and_then(Value::as_object)
            .map(|members| {
                members
                    .iter()
                    .map(|(name, member)| {
                        member
                            .get("traits")
                            .and_then(|traits| traits.get("smithy.api#enumValue"))
                            .and_then(Value::as_str)
                            .unwrap_or(name)
                            .to_string()
                    })
                    .collect()
            });
    }
    (!rules.is_empty()).then_some(rules)
}

/// Split a violation path into the collection member it is an element of and
/// that element's position, when it names a list element at all.
///
/// The two trailing segments are compared against the element step the walk
/// itself spells, so the two cannot drift apart.
fn element_position(violation_path: &str) -> Option<(&str, usize)> {
    let (head, step) = violation_path.rsplit_once('.')?;
    let (collection, position) = head.rsplit_once('.')?;
    let position: usize = position.parse().ok()?;
    (format!("{position}.{step}") == path::element(position)).then_some((collection, position))
}

/// One entry of a validation message: the clause, and the path it is ordered by.
struct Entry {
    path: String,
    clause: String,
}

/// The offending elements of one constrained collection member, in the order
/// the walk met them, with the contract its entry states.
struct Group {
    path: String,
    requirements: Vec<String>,
    values: Vec<(usize, String)>,
}

/// Word `violations` the way AWS words them, or `None` when this service has
/// nothing to add to the framework's own wording.
///
/// `None` is returned unless at least one violation is an element of a
/// constrained collection member: that is the only class whose rendering
/// differs, and leaving every other refusal to the framework keeps one wording
/// for it instead of two.
fn invalid_message(violations: &[Violation]) -> Option<String> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut grouped_any = false;
    for violation in violations {
        match collection_of(violation) {
            Some((collection, position, requirements)) => {
                grouped_any = true;
                let value = violation.value.clone().unwrap_or_default();
                match groups.iter_mut().find(|group| group.path == collection) {
                    Some(group) => {
                        // One element can fail several constraints; it is still
                        // one value of the list.
                        if !group.values.iter().any(|(at, _)| *at == position) {
                            group.values.push((position, value));
                        }
                    }
                    None => groups.push(Group {
                        path: collection.to_string(),
                        requirements,
                        values: vec![(position, value)],
                    }),
                }
            }
            None => entries.push(Entry {
                path: violation.path.clone(),
                clause: violation.message(),
            }),
        }
    }
    if !grouped_any {
        return None;
    }
    for group in groups {
        let mut values = group.values;
        values.sort_by_key(|(position, _)| *position);
        let rendered: Vec<&str> = values.iter().map(|(_, value)| value.as_str()).collect();
        let clause = format!(
            "Value '[{}]' at '{}' failed to satisfy constraint: \
             Member must satisfy constraint: [{}]",
            rendered.join(", "),
            group.path,
            group.requirements.join(", ")
        );
        entries.push(Entry {
            path: group.path,
            clause,
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let clauses: Vec<&str> = entries.iter().map(|entry| entry.clause.as_str()).collect();
    // The envelope AWS opens a `ValidationException` with, counting the entries
    // the message states (one per member, however many values it refused) —
    // mirroring `aws_smithy_schema_server::constraints::envelope`, which cannot
    // build it here because these clauses are not one-per-violation.
    Some(match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    })
}

/// The constrained list member `violation` is an element of, with the element's
/// position and the contract its entry states — `None` when it is not a list
/// element, when the model knows no such member or knows it ambiguously, when
/// the rules cannot account for what the walk refused, or when the contract
/// cannot be worded. Every one of those keeps the framework's own clause.
fn collection_of(violation: &Violation) -> Option<(&str, usize, Vec<String>)> {
    let (collection, position) = element_position(&violation.path)?;
    let Indexed::Unique(rules) = ELEMENT_RULES.get(collection)? else {
        return None;
    };
    if !rules.list || !rules.covers(&violation.kind) {
        return None;
    }
    Some((collection, position, rules.requirements()?))
}

/// This service's answer to a framework refusal, or `None` to keep the
/// framework's own wording.
pub fn refusal(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => invalid_message(violations).map(|message| Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message,
        }),
        // Authentication is the emulator's own concern, not this service's.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_violation(path: &str, value: &str, pattern: &str) -> Violation {
        Violation {
            path: path.to_string(),
            kind: ViolationKind::Pattern(pattern.to_string()),
            value: Some(value.to_string()),
        }
    }

    /// The index is built from the model, so a constrained collection member is
    /// found at the path the walk reports it at, with the element shape's whole
    /// contract — not only the constraint that failed.
    #[test]
    fn the_model_indexes_every_constrained_collection_member() {
        assert!(
            !ELEMENT_RULES.is_empty(),
            "the model must index at least one constrained collection member; \
             an empty index means the model was not read"
        );
        for (path, min, max) in [
            ("vpcOptions.subnetIds", 15, 24),
            ("vpcOptions.securityGroupIds", 11, 20),
            ("endpointIds", 3, 512),
        ] {
            let Some(Indexed::Unique(rules)) = ELEMENT_RULES.get(path) else {
                panic!(
                    "case {path:?}: the model declares this member once, got {:?}",
                    ELEMENT_RULES.get(path)
                )
            };
            assert_eq!(
                (rules.min_length, rules.max_length),
                (Some(min), Some(max)),
                "case {path:?}: expected the element shape's length bounds, got {rules:?}"
            );
            assert!(
                rules.pattern.is_some() && rules.list,
                "case {path:?}: expected a constrained list member, got {rules:?}"
            );
        }
        // A list whose element shape declares nothing is not indexed at all.
        assert!(
            !ELEMENT_RULES.contains_key("tagKeys"),
            "an unconstrained list member must not be indexed"
        );
    }

    /// Two operations exposing one path with DIFFERENT element shapes cannot be
    /// told apart from a violation path, so neither one's contract may speak for
    /// the other's request.
    #[test]
    fn a_path_two_operations_disagree_about_is_not_used() {
        let mut index = HashMap::new();
        let shapes = serde_json::json!({
            "ns#A": {"type": "structure", "members": {"Ids": {"target": "ns#ShortIds"}}},
            "ns#B": {"type": "structure", "members": {"Ids": {"target": "ns#LongIds"}}},
            "ns#ShortIds": {"type": "list", "member": {"target": "ns#Short"}},
            "ns#LongIds": {"type": "list", "member": {"target": "ns#Long"}},
            "ns#Short": {"type": "string", "traits": {"smithy.api#length": {"min": 1, "max": 2}}},
            "ns#Long": {"type": "string", "traits": {"smithy.api#length": {"min": 1, "max": 20}}},
        });
        let shapes = shapes.as_object().expect("a shape map");
        index_structure(shapes, "ns#A", "", &mut Vec::new(), &mut index);
        assert!(
            matches!(index.get("ids"), Some(Indexed::Unique(_))),
            "one operation's member must be indexed, got {:?}",
            index.get("ids")
        );
        index_structure(shapes, "ns#B", "", &mut Vec::new(), &mut index);
        assert_eq!(
            index.get("ids"),
            Some(&Indexed::Conflicting),
            "a disputed path must not be usable, got {:?}",
            index.get("ids")
        );
    }

    /// The requirement half is recovered from the clause the framework writes,
    /// so the separator it writes is part of the contract this module relies on.
    #[test]
    fn the_framework_still_separates_the_requirement() {
        assert_eq!(
            requirement_of(ViolationKind::MaxLength(7)).as_deref(),
            Some("Member must have length less than or equal to 7"),
            "the framework's clause must still carry {REQUIREMENT_PREFIX:?}, got {:?}",
            requirement_of(ViolationKind::MaxLength(7))
        );
    }

    /// A refusal with no collection element in it is left to the framework.
    #[test]
    fn a_scalar_refusal_keeps_the_framework_wording() {
        let violations = vec![pattern_violation("pipelineArn", "nope", "^arn:.+$")];
        assert_eq!(
            invalid_message(&violations),
            None,
            "a scalar member must keep the framework's clause"
        );
    }

    /// Several elements of one member are one entry, and a value the rules
    /// cannot account for is not folded into it.
    #[test]
    fn one_member_is_one_entry() {
        let pattern = r"^subnet-\w{8}(\w{9})?$";
        let violations = vec![
            pattern_violation("vpcOptions.subnetIds.1.member", "bad-one", pattern),
            pattern_violation("vpcOptions.subnetIds.2.member", "bad-two", pattern),
        ];
        let message = invalid_message(&violations).expect("a collection entry");
        assert!(
            message.starts_with("1 validation error detected: Value '[bad-one, bad-two]' at 'vpcOptions.subnetIds'"),
            "expected one entry listing both values, got {message:?}"
        );

        let stale = vec![pattern_violation(
            "vpcOptions.subnetIds.1.member",
            "bad-one",
            "^something-else$",
        )];
        assert_eq!(
            invalid_message(&stale),
            None,
            "a violation the model cannot account for must keep the framework's clause"
        );
    }

    /// Anchors are the model's way of spelling "the whole value"; AWS states the
    /// same requirement unanchored, and never twice.
    #[test]
    fn a_pattern_is_stated_unanchored() {
        for (pattern, expected) in [
            (r"^sg-\w{8}(\w{9})?$", r"sg-\w{8}(\w{9})?.*"),
            (r"^[a-z][a-z0-9\-]+$", r"[a-z][a-z0-9\-]+.*"),
            ("plain", "plain.*"),
            ("already.*", "already.*"),
        ] {
            assert_eq!(
                unanchored(pattern),
                expected,
                "case {pattern:?}: expected {expected:?}, got {:?}",
                unanchored(pattern)
            );
        }
    }
}

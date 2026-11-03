// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The framework walks the modelled constraints and hands [`Refusal`] to
//! `service.rs` structurally — one [`Violation`] per offending value, with the
//! path it sits at, what it failed and what it held — and leaves the wording to
//! the service. For most classes the framework's own clause is already what AWS
//! answers, so this module declines them (`None`) and the default stands.
//!
//! Two things it does reword.
//!
//! **The pattern a clause quotes.** Wherever a clause states a `@pattern`
//! requirement, it quotes the pattern this service PUBLISHES for that string
//! shape, not the model's regex literal (see [`published_pattern`]): the anchors
//! dropped, a redundantly escaped `-` inside a character class written
//! literally, and a trailing one-or-more repetition replaced by the explicit
//! `{lo,hi}` range the shape's `@length` bounds pin it down to — so the one
//! regex a client reads states the charset AND the length bound together. This
//! holds for every constrained string shape of this service, at a scalar member
//! and inside a collection entry alike; where the model's pattern pins nothing
//! down (no trailing one-or-more repetition, or no length maximum) the
//! unanchored find-form is quoted.
//!
//! **A member that is a collection of constrained scalars.** AWS reports such a
//! member ONCE, not once per offending element:
//!
//! * the entry names the COLLECTION member's own path — no element index and no
//!   `member` step (`vpcOptions.subnetIds`, not `vpcOptions.subnetIds.1.member`);
//! * the refused values are rendered as a bracketed list (`'[a, b]'`), so one
//!   entry covers however many elements failed;
//! * the requirement enumerates EVERY constraint the ELEMENT shape declares —
//!   length maximum, then length minimum, then pattern, then enum value set —
//!   whichever of them this request happened to fail, inside
//!   `Member must satisfy constraint: [ … ]`;
//! * a pattern is stated in its published form (see [`published_pattern`]);
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

/// What one scalar string member must satisfy, read from the model: the shape's
/// `@pattern` and the `@length` bounds that pin its repetition down.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScalarRules {
    min_length: Option<i64>,
    max_length: Option<i64>,
    pattern: String,
}

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
            out.push(requirement_of(ViolationKind::Pattern(published_pattern(
                pattern,
                self.min_length,
                self.max_length,
            )))?);
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
        sensitive: false,
    }
    .message();
    clause
        .split_once(REQUIREMENT_PREFIX)
        .map(|(_, requirement)| requirement.to_string())
}

/// The pattern this service PUBLISHES for a string shape whose model form is
/// `pattern` and whose `@length` bounds are `min_length`/`max_length` — the
/// pattern a validation message quotes.
///
/// Three steps, each one a rendering of what the model already says:
///
/// * a `-` escaped inside a character class where the escape is redundant is
///   written literally (see [`literal_class_hyphens`]);
/// * the `^`/`$` anchors are dropped: Smithy `@pattern` semantics are a find,
///   not a full match, so the anchors are the model's way of spelling "the whole
///   value", which a published pattern states by being the whole pattern;
/// * a trailing one-or-more repetition is stated as the explicit `{lo,hi}` range
///   the length bounds pin it down to (see [`bounded_repetition`]), so the
///   quoted regex carries the length bound too. Where it cannot be — the
///   pattern has no such tail, or the shape declares no length maximum — the
///   find-form `.*` tail is kept instead, because then the pattern alone says
///   nothing about how long the value may be.
///
/// Rendering only: the framework still enforces the model's anchored regex, so
/// which values are refused does not change.
fn published_pattern(pattern: &str, min_length: Option<i64>, max_length: Option<i64>) -> String {
    let literal = literal_class_hyphens(pattern);
    let body = literal.strip_prefix('^').unwrap_or(&literal);
    let body = body.strip_suffix('$').unwrap_or(body);
    if let Some(bounded) = bounded_repetition(body, min_length, max_length) {
        return bounded;
    }
    // A tail that already matches any remainder is a find-form tail already.
    if body.ends_with(".*") || body.ends_with(".+") {
        body.to_string()
    } else {
        format!("{body}.*")
    }
}

/// `body` with its trailing one-or-more repetition stated as the explicit
/// repetition range the length bounds imply, or `None` when the model does not
/// pin that range down.
///
/// The tail must be a single-character matcher quantified `+`, and everything
/// before it must match a FIXED number of characters: only then does a length
/// bound on the whole value translate into a bound on the repetition. The range
/// is the length bound less that fixed width, never below the one repetition the
/// `+` itself requires.
fn bounded_repetition(
    body: &str,
    min_length: Option<i64>,
    max_length: Option<i64>,
) -> Option<String> {
    let elements = elements(body)?;
    let (tail, head) = elements.split_last()?;
    if tail.quantifier != "+" || !tail.single_char {
        return None;
    }
    let mut fixed: i64 = 0;
    for element in head {
        fixed = fixed.checked_add(element.fixed_width()?)?;
    }
    // Without a maximum there is no range to state: `+` already says "at least
    // one", and a minimum alone leaves the upper end open.
    let max = max_length?;
    let low = min_length.unwrap_or(0).saturating_sub(fixed).max(1);
    let high = max.checked_sub(fixed)?;
    if high < low {
        return None;
    }
    let range = if low == high {
        format!("{{{low}}}")
    } else {
        format!("{{{low},{high}}}")
    };
    Some(format!("{}{range}", &body[..tail.matcher_end]))
}

/// One element of a regex: a matcher and the quantifier applied to it.
#[derive(Debug)]
struct Element {
    /// Byte offset in the pattern just past the matcher, before its quantifier.
    matcher_end: usize,
    /// Whether the matcher matches exactly one character — a literal, an escape
    /// sequence, a `.` or a character class — rather than a group whose width
    /// this reading cannot know.
    single_char: bool,
    /// The quantifier as written, empty when the matcher carries none.
    quantifier: String,
}

impl Element {
    /// How many characters this element matches, when that count is fixed.
    fn fixed_width(&self) -> Option<i64> {
        if !self.single_char {
            return None;
        }
        match self.quantifier.as_str() {
            "" => Some(1),
            // `{n}` is a fixed count; `{n,m}`, `?`, `*` and `+` are not.
            other => {
                let inner = other.strip_prefix('{')?.strip_suffix('}')?;
                (!inner.is_empty() && inner.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| inner.parse().ok())
                    .flatten()
            }
        }
    }
}

/// Split a regex into its elements, or `None` when it uses a construct this
/// reading does not model: a top-level alternation (whose branches have no one
/// width), an unterminated group or class, or a quantifier with no matcher.
/// `None` simply means no repetition range is derived and the find-form stands.
fn elements(body: &str) -> Option<Vec<Element>> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let (matcher_end, single_char) = match bytes[at] {
            // An alternation spans the whole expression, so no element before it
            // has a width of its own.
            b'|' => return None,
            b'*' | b'+' | b'?' | b'{' => return None,
            b'\\' => {
                let next = body.get(at + 1..)?.chars().next()?;
                (at + 1 + next.len_utf8(), true)
            }
            b'[' => (class_end(bytes, at)?, true),
            b'(' => (group_end(bytes, at)?, false),
            _ => (at + body.get(at..)?.chars().next()?.len_utf8(), true),
        };
        let mut end = matcher_end;
        match bytes.get(end) {
            Some(b'*' | b'+' | b'?') => end += 1,
            Some(b'{') => {
                let close = bytes.iter().skip(end).position(|byte| *byte == b'}')?;
                end += close + 1;
            }
            _ => {}
        }
        // A lazy or possessive marker belongs to the quantifier it follows.
        if end > matcher_end && matches!(bytes.get(end), Some(b'?' | b'+')) {
            end += 1;
        }
        out.push(Element {
            matcher_end,
            single_char,
            quantifier: body.get(matcher_end..end)?.to_string(),
        });
        at = end;
    }
    (!out.is_empty()).then_some(out)
}

/// The byte offset just past the character class that starts at `open`.
fn class_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut at = open + 1;
    if bytes.get(at) == Some(&b'^') {
        at += 1;
    }
    // A `]` first in the class is the literal character, not the close.
    if bytes.get(at) == Some(&b']') {
        at += 1;
    }
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b']' => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

/// The byte offset just past the group that starts at `open`, counting nested
/// groups and ignoring a `)` inside an escape or a character class.
fn group_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut at = open;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'[' => at = class_end(bytes, at)?,
            b'(' => {
                depth += 1;
                at += 1;
            }
            b')' => {
                depth -= 1;
                at += 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => at += 1,
        }
    }
    None
}

/// A pattern with every redundantly escaped `-` inside a character class written
/// as the literal character the published pattern states: `[a-z0-9\-]` becomes
/// `[a-z0-9-]`.
///
/// Only where the escape is redundant — first in the class or last before its
/// `]` — because anywhere else dropping the backslash would turn the character
/// into a RANGE and change what the pattern means. An escape outside a class is
/// left alone for the same reason: it is not this rendering's business.
fn literal_class_hyphens(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len());
    let mut at = 0usize;
    let mut in_class = false;
    let mut at_class_start = false;
    while at < chars.len() {
        let current = chars[at];
        if !in_class {
            out.push(current);
            at += 1;
            match current {
                '\\' => {
                    if let Some(escaped) = chars.get(at) {
                        out.push(*escaped);
                        at += 1;
                    }
                }
                '[' => {
                    in_class = true;
                    at_class_start = true;
                    if chars.get(at) == Some(&'^') {
                        out.push('^');
                        at += 1;
                    }
                }
                _ => {}
            }
            continue;
        }
        if current == '\\' {
            let escaped = chars.get(at + 1).copied();
            if escaped == Some('-') && (at_class_start || chars.get(at + 2) == Some(&']')) {
                out.push('-');
                at += 2;
            } else {
                out.push(current);
                if let Some(escaped) = escaped {
                    out.push(escaped);
                    at += 2;
                } else {
                    at += 1;
                }
            }
            at_class_start = false;
            continue;
        }
        if current == ']' && !at_class_start {
            in_class = false;
        }
        out.push(current);
        at_class_start = false;
        at += 1;
    }
    out
}

/// What the index knows about one member path.
///
/// A path is reported by the walk without naming the operation it came from, so
/// two operations whose inputs expose the SAME path with different shapes cannot
/// be told apart here. Rather than let one operation's constraints speak for the
/// other's request, such a path is marked [`Indexed::Conflicting`] and keeps the
/// framework's own clause.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Indexed<T> {
    /// Every operation that exposes this path agrees on the shape.
    Unique(T),
    /// Two operations disagree about it; no entry may speak for it.
    Conflicting,
}

/// Record `rules` for `member_path`, unless another operation already indexed
/// that path with a different shape.
fn record<T: PartialEq>(out: &mut HashMap<String, Indexed<T>>, member_path: String, rules: T) {
    match out.get(&member_path) {
        None => {
            out.insert(member_path, Indexed::Unique(rules));
        }
        // Another operation already indexed this path with the same shape —
        // nothing changes.
        Some(Indexed::Unique(seen)) if *seen == rules => {}
        Some(_) => {
            out.insert(member_path, Indexed::Conflicting);
        }
    }
}

/// What this service's model says about the member paths a refusal can name,
/// keyed by the dotted lowerCamel path the validating walk reports them at.
#[derive(Debug, Default)]
struct ModelIndex {
    /// Every collection member whose element shape carries constraints.
    collections: HashMap<String, Indexed<ElementRules>>,
    /// Every scalar member whose shape declares a `@pattern`.
    scalars: HashMap<String, Indexed<ScalarRules>>,
}

/// The model, read once.
static MODEL: LazyLock<ModelIndex> = LazyLock::new(build_index);

/// Read the model once and index the members a refusal can name.
///
/// Rooted at each operation's input shape, because that is where a validation
/// path is rooted. A collection nested inside a collection's element structure
/// is not indexed: its elements are refused at a deeper path than an entry
/// could speak for, and they keep the framework's clause.
fn build_index() -> ModelIndex {
    let mut out = ModelIndex::default();
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

/// Index the members of `shape_id`, descending through structure and union
/// members. `open` carries the shapes already on this path, so a recursive shape
/// is walked once rather than forever.
fn index_structure(
    shapes: &serde_json::Map<String, Value>,
    shape_id: &str,
    prefix: &str,
    open: &mut Vec<String>,
    out: &mut ModelIndex,
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
                        record(&mut out.collections, member_path, rules);
                    }
                }
                Some(_) => {
                    if let Some(rules) = scalar_rules(shapes, target) {
                        record(&mut out.scalars, member_path, rules);
                    }
                }
                None => {}
            }
        }
    }
    open.pop();
}

/// The constraints scalar shape `shape_id` declares, when it declares a
/// `@pattern` at all — the only constraint whose clause this service rewords for
/// a scalar member. Read off the SHAPE, which is the source codegen captures for
/// the validating walk.
fn scalar_rules(shapes: &serde_json::Map<String, Value>, shape_id: &str) -> Option<ScalarRules> {
    let traits = shapes.get(shape_id)?.get("traits")?;
    let pattern = traits
        .get("smithy.api#pattern")
        .and_then(Value::as_str)?
        .to_string();
    let length = traits.get("smithy.api#length");
    Some(ScalarRules {
        min_length: length
            .and_then(|length| length.get("min"))
            .and_then(Value::as_i64),
        max_length: length
            .and_then(|length| length.get("max"))
            .and_then(Value::as_i64),
        pattern,
    })
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
/// `None` is returned unless at least one violation is reworded — an element of
/// a constrained collection member, or a scalar member whose published pattern
/// is not the model's literal. Leaving every other refusal to the framework
/// keeps one wording for it instead of two.
fn invalid_message(violations: &[Violation]) -> Option<String> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut reworded_any = false;
    for violation in violations {
        match collection_of(violation) {
            Some((collection, position, requirements)) => {
                reworded_any = true;
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
            None => {
                let clause = match scalar_clause(violation) {
                    Some(clause) => {
                        reworded_any = true;
                        clause
                    }
                    None => violation.message(),
                };
                entries.push(Entry {
                    path: violation.path.clone(),
                    clause,
                });
            }
        }
    }
    if !reworded_any {
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
    let Indexed::Unique(rules) = MODEL.collections.get(collection)? else {
        return None;
    };
    if !rules.list || !rules.covers(&violation.kind) {
        return None;
    }
    Some((collection, position, rules.requirements()?))
}

/// The clause for a SCALAR member refused on its `@pattern`: the framework's own
/// sentence with the pattern this service publishes in place of the model's
/// regex literal.
///
/// `None` — keeping the framework's clause — when the refusal is not a pattern
/// refusal, when the model knows no such scalar member or knows it ambiguously,
/// when the model's pattern is not the one the walk refused against, when the
/// published form IS the model's literal, or when the framework's sentence does
/// not carry the separator the requirement half is swapped at.
fn scalar_clause(violation: &Violation) -> Option<String> {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return None;
    };
    let Indexed::Unique(rules) = MODEL.scalars.get(&violation.path)? else {
        return None;
    };
    if rules.pattern != *pattern {
        return None;
    }
    let published = published_pattern(pattern, rules.min_length, rules.max_length);
    if published == *pattern {
        return None;
    }
    // Swap only the requirement half, so the `Value '…' at '…'` half stays the
    // framework's one rendering of it.
    let clause = violation.message();
    let (head, _) = clause.split_once(REQUIREMENT_PREFIX)?;
    let requirement = requirement_of(ViolationKind::Pattern(published))?;
    Some(format!("{head}{REQUIREMENT_PREFIX}{requirement}"))
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
            sensitive: false,
        }
    }

    /// The index is built from the model, so a constrained collection member is
    /// found at the path the walk reports it at, with the element shape's whole
    /// contract — not only the constraint that failed.
    #[test]
    fn the_model_indexes_every_constrained_collection_member() {
        assert!(
            !MODEL.collections.is_empty(),
            "the model must index at least one constrained collection member; \
             an empty index means the model was not read"
        );
        for (path, min, max) in [
            ("vpcOptions.subnetIds", 15, 24),
            ("vpcOptions.securityGroupIds", 11, 20),
            ("endpointIds", 3, 512),
        ] {
            let Some(Indexed::Unique(rules)) = MODEL.collections.get(path) else {
                panic!(
                    "case {path:?}: the model declares this member once, got {:?}",
                    MODEL.collections.get(path)
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
            !MODEL.collections.contains_key("tagKeys"),
            "an unconstrained list member must not be indexed"
        );
    }

    /// The same walk indexes the SCALAR members that declare a pattern, with the
    /// length bounds the published pattern folds in, whichever operation reaches
    /// them.
    #[test]
    fn the_model_indexes_every_patterned_scalar_member() {
        for (path, pattern, min, max) in [
            ("pipelineName", r"^[a-z][a-z0-9\-]+$", Some(3), Some(28)),
            (
                "endpointId",
                "^[a-zA-Z0-9][a-zA-Z0-9-_]+$",
                Some(3),
                Some(512),
            ),
            (
                "logPublishingOptions.cloudWatchLogDestination.logGroup",
                r"^\/aws\/vendedlogs\/[\.\-_/#A-Za-z0-9]+$",
                Some(1),
                Some(512),
            ),
            ("format", "^(YAML|JSON)$", None, None),
        ] {
            let Some(Indexed::Unique(rules)) = MODEL.scalars.get(path) else {
                panic!(
                    "case {path:?}: the model declares this member once, got {:?}",
                    MODEL.scalars.get(path)
                )
            };
            assert_eq!(
                (rules.pattern.as_str(), rules.min_length, rules.max_length),
                (pattern, min, max),
                "case {path:?}: expected the shape's own contract, got {rules:?}"
            );
        }
        // A member whose shape declares no pattern has nothing to republish.
        assert!(
            !MODEL.scalars.contains_key("pipelineConfigurationBody"),
            "a member with no pattern must not be indexed"
        );
    }

    /// Two operations exposing one path with DIFFERENT element shapes cannot be
    /// told apart from a violation path, so neither one's contract may speak for
    /// the other's request.
    #[test]
    fn a_path_two_operations_disagree_about_is_not_used() {
        let mut index = ModelIndex::default();
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
            matches!(index.collections.get("ids"), Some(Indexed::Unique(_))),
            "one operation's member must be indexed, got {:?}",
            index.collections.get("ids")
        );
        index_structure(shapes, "ns#B", "", &mut Vec::new(), &mut index);
        assert_eq!(
            index.collections.get("ids"),
            Some(&Indexed::Conflicting),
            "a disputed path must not be usable, got {:?}",
            index.collections.get("ids")
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

    /// A scalar refusal the model cannot account for — a pattern that is not the
    /// one the model declares for that member — is left to the framework.
    #[test]
    fn a_scalar_refusal_the_model_cannot_account_for_keeps_the_framework_wording() {
        let violations = vec![pattern_violation("pipelineArn", "nope", "^arn:.+$")];
        assert_eq!(
            invalid_message(&violations),
            None,
            "a pattern the model does not declare must keep the framework's clause"
        );
        // A refusal that is not about a pattern states a constraint this seam
        // does not republish, so it is left alone too.
        let too_short = vec![Violation {
            path: "pipelineName".to_string(),
            kind: ViolationKind::MinLength(3),
            value: Some("ab".to_string()),
            sensitive: false,
        }];
        assert_eq!(
            invalid_message(&too_short),
            None,
            "a length refusal must keep the framework's clause"
        );
    }

    /// A scalar member refused on its pattern quotes the pattern the service
    /// publishes, in the framework's own sentence.
    #[test]
    fn a_scalar_pattern_refusal_quotes_the_published_pattern() {
        let violations = vec![pattern_violation(
            "pipelineName",
            "Not-A-Name",
            r"^[a-z][a-z0-9\-]+$",
        )];
        let message = invalid_message(&violations).expect("a reworded clause");
        assert_eq!(
            message,
            "1 validation error detected: Value 'Not-A-Name' at 'pipelineName' \
             failed to satisfy constraint: \
             Member must satisfy regular expression pattern: [a-z][a-z0-9-]{2,27}",
            "expected the published pattern, got {message:?}"
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

    /// The published pattern states the charset and the length bound in one
    /// regex: anchors dropped, a redundantly escaped `-` written literally, and a
    /// trailing one-or-more repetition bounded by the shape's length bounds.
    /// Where the model pins no range down, the unanchored find-form stands.
    #[test]
    fn the_published_pattern_folds_the_length_bound() {
        // Rows: every patterned string shape this service declares (source:
        // `smithy_query(query="closure", operation="CreatePipeline,DeletePipeline,
        // DeletePipelineEndpoint,ListTagsForResource")`), plus the degenerate
        // bounds a model may write.
        for (case, pattern, min, max, expected) in [
            (
                "PipelineName: the bound lands on the repetition",
                r"^[a-z][a-z0-9\-]+$",
                Some(3),
                Some(28),
                r"[a-z][a-z0-9-]{2,27}",
            ),
            (
                "PipelineEndpointId: a wider bound, same shape of pattern",
                "^[a-zA-Z0-9][a-zA-Z0-9-_]+$",
                Some(3),
                Some(512),
                "[a-zA-Z0-9][a-zA-Z0-9-_]{2,511}",
            ),
            (
                "LogGroup: a long fixed prefix, and a minimum the prefix covers",
                r"^\/aws\/vendedlogs\/[\.\-_/#A-Za-z0-9]+$",
                Some(1),
                Some(512),
                r"\/aws\/vendedlogs\/[\.\-_/#A-Za-z0-9]{1,496}",
            ),
            (
                "SubnetId: no open repetition to bound",
                r"^subnet-\w{8}(\w{9})?$",
                Some(15),
                Some(24),
                r"subnet-\w{8}(\w{9})?.*",
            ),
            (
                "SecurityGroupId: no open repetition to bound",
                r"^sg-\w{8}(\w{9})?$",
                Some(11),
                Some(20),
                r"sg-\w{8}(\w{9})?.*",
            ),
            (
                "PipelineArn: a group of alternatives leaves no fixed width",
                r"^arn:(aws|aws\-cn):osis:.+:pipeline\/.+$",
                Some(46),
                Some(76),
                r"arn:(aws|aws\-cn):osis:.+:pipeline\/.+",
            ),
            (
                "TagKey: an optional tail is not a one-or-more repetition",
                ".*",
                Some(1),
                Some(128),
                ".*",
            ),
            (
                "a bound that leaves exactly one repetition count",
                "^[a-z]+$",
                Some(5),
                Some(5),
                "[a-z]{5}",
            ),
            (
                "no length maximum leaves the upper end open",
                r"^[a-z][a-z0-9\-]+$",
                Some(3),
                None,
                r"[a-z][a-z0-9-]+.*",
            ),
            (
                "no length bounds at all",
                "^plain$",
                None,
                None,
                "plain.*",
            ),
            (
                "a maximum shorter than the pattern's fixed part states no range",
                "^abcd[0-9]+$",
                Some(1),
                Some(4),
                "abcd[0-9]+.*",
            ),
        ] {
            assert_eq!(
                published_pattern(pattern, min, max),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                published_pattern(pattern, min, max)
            );
        }
    }

    /// Dropping the backslash from an escaped `-` is safe only where a literal
    /// `-` cannot start a range; anywhere else the escape stays, because the
    /// published pattern must mean what the model's pattern means.
    #[test]
    fn only_a_redundant_class_escape_is_written_literally() {
        for (case, pattern, expected) in [
            ("last in the class", r"[a-z0-9\-]", "[a-z0-9-]"),
            ("first in the class", r"[\-a-z]", "[-a-z]"),
            ("first after a negation", r"[^\-a-z]", "[^-a-z]"),
            ("between two elements", r"[a\-z0-9]", r"[a\-z0-9]"),
            ("outside any class", r"aws\-cn", r"aws\-cn"),
            (
                "a class that holds a literal ]",
                r"[]a\-]x\-y",
                r"[]a-]x\-y",
            ),
        ] {
            assert_eq!(
                literal_class_hyphens(pattern),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                literal_class_hyphens(pattern)
            );
        }
    }
}

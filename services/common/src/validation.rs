// SPDX-License-Identifier: CC0-1.0

//! The value a constraint-violation message echoes: the one the caller submitted.
//!
//! An AWS constraint-validation message is BUILT FROM THE REQUEST —
//! `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//! constraint: <requirement>` — so the value between the quotes is part of the rule, not
//! prose: the message is the only place the server reports the offending input back, and
//! a caller that compares the echo against what it sent must see its own bytes.
//!
//! The framework's constraint walk
//! ([`aws_smithy_schema_server::constraints::validate`]) stores an ELIDED rendering on
//! [`Violation::value`] — a byte-bounded prefix followed by `... [<n> bytes]` — because
//! the clause interpolates whatever is stored and a caller controls the length. That
//! marker is wire text AWS never emits, so a service that words its own refusal from
//! those violations echoes a value the caller never submitted.
//!
//! [`with_submitted_values`] closes that gap WITHOUT re-implementing any constraint
//! check: it re-walks the SAME built input the walk validated, collects every
//! string-rendered leaf under the path spelling
//! [`aws_smithy_schema_server::constraints::path`] defines, and replaces a violation's
//! stored echo with the leaf it was elided from. A stored echo is replaced only when it
//! IS that leaf's elision (identical, or a proper prefix plus the marker naming that
//! leaf's byte count), so a path whose leaf is unrelated keeps the framework's own text.
//!
//! Only string-rendered leaves are collected, because they are the only ones whose echo
//! is the caller's own bytes: the walk renders a blob as `<n> bytes`, a list as
//! `<n> items`, a map as `<n> entries` and a document as `<document>`, each a bounded
//! summary of the value rather than the value, and a numeric or timestamp leaf is already
//! far shorter than any elision cap.
//!
//! # The bounds this module states
//!
//! The framework's cap is a RESOURCE bound as well as a rendering choice: a request whose
//! members are large strings would otherwise be re-echoed into an arbitrarily large
//! response, once per clause that fired, and the violation COUNT cap
//! (`MAX_VIOLATIONS` = 100) bounds only one factor of that product. Restoring the value
//! therefore comes with its own finite bounds, chosen here and not derived from the
//! request:
//!
//! * [`MAX_RESTORED_ECHO`] (16 KiB) — the largest single value restored into a clause.
//!   64x the largest `@length` maximum an emulated member declares today (256), so the
//!   fidelity this exists for — echoing a 300-character id, an over-long ARN, a rejected
//!   enum value — always lands, while one clause can never carry request-sized content.
//! * [`MAX_RESTORED_TOTAL`] (64 KiB) — the restored bytes ALL clauses of one message may
//!   carry together, so `violations x value size` is bounded as a product. For scale, the
//!   framework's own ceiling on the same text is 100 clauses x 256 bytes ~= 25 KiB, so
//!   this is the same order of magnitude rather than a new amplification class.
//! * [`MAX_RECORDED_LEAVES`] (256) and [`MAX_RECORDED_BYTES`] (64 KiB) — the leaf count
//!   and the owned bytes (paths plus values) this walk may collect from one request. The
//!   walk stops at the leaf cap and skips any leaf that does not fit the byte budget, so
//!   the collection is never a second full copy of the request's strings.
//!
//! Beyond any of these the framework's elided text stands: a shortened echo is a
//! divergence in one message, while an unbounded one is a response sized by the request.
//!
//! # Why this lives here at all
//!
//! The structurally right place for this is the frozen crate: carry the unelided value
//! beside the elided one on `Violation`, or elide in `Violation::message` rather than
//! inside `LengthTrait::check` / `PatternTrait::check`, so no re-walk is needed. `vendor/`
//! is read-only here, so that change is filed as
//! `issues/things-that-do-not-work/20261022T133000Z-gap-smithy-schema-server-constraint-violation-stores-only-the-elided-rejected-value.md`.
//! Until it lands, the walk below MIRRORS that crate's private path bookkeeping
//! (`ValidatingSerializer`), and the two can drift: the drift is fail-safe — a path that
//! no longer matches yields no leaf, so the framework's own text stands — but this module
//! is to be deleted, not copied, once the core seam exists.
//!
//! # Still to migrate here (staged, not yet wired)
//!
//! This module owns ONE part of a validation-exception message — which value a clause
//! echoes. Three more parts are the same for EVERY service and still belong here:
//!
//! * the requirement half, read off the framework's own wording from a violation with NO
//!   value attached and split at the LAST clause marker, so neither the rejected value
//!   nor a caller-chosen map key in the member path can reach it;
//! * the subject half: a service that reports a present-but-invalid value as
//!   `Value at '<path>' …` without quoting it back, while an ABSENT member keeps
//!   `Value null at '<path>' …`;
//! * a `@pattern` printed as the expression a value has to satisfy, without the `^`/`$`
//!   anchors a model writes to demand a whole-string match.
//!
//! How the clauses are then counted, ordered and joined into an envelope is a service
//! DIALECT (`N validation error(s) detected: …,…` for Amazon EVS,
//! `N validation errors detected: …; …` elsewhere, and `artifact` prints a `@pattern`
//! WITH its anchors), so that half stays with the service. The three rules above are
//! not a dialect, and the tree carries them inlined in ~24 service crates
//! (`grep -rln 'fn unanchored' services/*/src`), each pinned by that service's own
//! recordings and tests. Migrating them is one change of its own rather than a side
//! effect of a single service's issue, so nothing is moved here by this module; the fix
//! that owns that migration moves the rules here and calls them from the service crates.
//! Nothing new should inline them again without leaving the same note.

use std::collections::{HashMap, HashSet};

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{path, Violation, ViolationKind};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The largest single submitted value restored into a clause (see the module docs).
const MAX_RESTORED_ECHO: usize = 16 * 1024;

/// The restored bytes all clauses of one message may carry together.
const MAX_RESTORED_TOTAL: usize = 64 * 1024;

/// The most leaves this walk collects from one request.
const MAX_RECORDED_LEAVES: usize = 256;

/// The owned bytes — joined paths plus values, both caller-sized — this walk may collect
/// from one request. Equal to [`MAX_RESTORED_TOTAL`]: a leaf that could never be restored
/// within that budget is not worth copying.
const MAX_RECORDED_BYTES: usize = MAX_RESTORED_TOTAL;

/// `violations`, with every elided value echo replaced by the value as submitted, within
/// the bounds this module states.
///
/// `input` and `schema` must be the ones the violations were collected from — the built
/// input and its operation's input schema — so a violation's path names a leaf of this
/// walk. A violation this walk finds no matching leaf for (an absent `@required` member,
/// a collection-level bound, a value the framework rendered as a summary) is returned
/// untouched, and so is one whose value is larger than the stated bounds allow.
#[must_use]
pub fn with_submitted_values(
    mut violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<Violation> {
    // Nothing to re-echo: a walk over the whole input is not worth doing for a refusal
    // that states no value at all (an absent `@required` member is the common case).
    if violations.iter().all(|violation| violation.value.is_none()) {
        return violations;
    }
    let leaves = submitted_leaves(input, schema);
    // ONE pass over the leaves builds every lookup the loop below needs, so the cost is
    // O(leaves) rather than O(violations x leaves).
    let index = Submitted::of(&leaves);
    let mut budget = MAX_RESTORED_TOTAL;
    for violation in &mut violations {
        let Some(stored) = violation.value.as_deref() else {
            continue;
        };
        let Some(submitted) = index.candidate(violation) else {
            continue;
        };
        // Nothing was elided: this clause already states the submitted value, and
        // rewriting it would spend budget to change nothing.
        if stored == submitted {
            continue;
        }
        // Only what the framework elided FROM this leaf, and only within the stated
        // bounds; anything else keeps the framework's own text.
        if !is_elision_of(stored, submitted)
            || submitted.len() > MAX_RESTORED_ECHO
            || submitted.len() > budget
        {
            continue;
        }
        budget -= submitted.len();
        violation.value = Some(submitted.to_string());
    }
    violations
}

/// The collected leaves of one request, indexed for the lookups a violation needs.
///
/// Both maps borrow the collected leaves and are built in a single pass over them.
#[derive(Debug)]
struct Submitted<'l> {
    /// The first leaf recorded at each dotted path: the member a clause names.
    by_path: HashMap<&'l str, &'l str>,
    /// For each list whose scalar elements were recorded, the first element value that
    /// repeated — what a `@uniqueItems` clause echoes, reported at the LIST's path.
    duplicate_in: HashMap<&'l str, &'l str>,
}

impl<'l> Submitted<'l> {
    fn of(leaves: &'l [(String, String)]) -> Self {
        // `leaves` is already bounded by `MAX_RECORDED_LEAVES` / `MAX_RECORDED_BYTES`, so
        // both maps are too.
        let mut by_path: HashMap<&str, &str> = HashMap::new();
        let mut seen: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut duplicate_in: HashMap<&str, &str> = HashMap::new();
        for (leaf_path, value) in leaves {
            by_path.entry(leaf_path.as_str()).or_insert(value.as_str());
            if let Some(list) = list_of_element(leaf_path) {
                if !seen.entry(list).or_default().insert(value.as_str()) {
                    duplicate_in.entry(list).or_insert(value.as_str());
                }
            }
        }
        Self {
            by_path,
            duplicate_in,
        }
    }

    /// The leaf `violation`'s stored echo could have been elided from.
    fn candidate(&self, violation: &Violation) -> Option<&'l str> {
        if matches!(violation.kind, ViolationKind::UniqueItems) {
            // A duplicate is reported at the LIST's path, and the value it echoes is one
            // of that list's elements — recorded one step deeper, never at the list.
            return self.duplicate_in.get(violation.path.as_str()).copied();
        }
        self.by_path.get(violation.path.as_str()).copied()
    }
}

/// The list path a leaf is a SCALAR ELEMENT of, if it is one.
///
/// A list element's path is `<list>.<1-based position>.member`
/// ([`aws_smithy_schema_server::constraints::path::element`]), so the list is that path
/// minus those two segments. Anything else — a struct member, a map entry value — belongs
/// to no list's uniqueness check.
fn list_of_element(leaf_path: &str) -> Option<&str> {
    let head = leaf_path.strip_suffix(".member")?;
    let (list, position) = head.rsplit_once('.')?;
    let is_position = !position.is_empty() && position.bytes().all(|byte| byte.is_ascii_digit());
    (is_position && !list.is_empty()).then_some(list)
}

/// Whether `stored` is what the framework's renderer makes of `submitted`: the value
/// itself, or a proper prefix of it followed by the marker naming its byte count.
///
/// Stated as a relation rather than by re-deriving the elision, so this does not depend
/// on the cap the frozen crate elides at — only on the shape of what it writes.
fn is_elision_of(stored: &str, submitted: &str) -> bool {
    if stored == submitted {
        return true;
    }
    let marker = format!("... [{} bytes]", submitted.len());
    stored
        .strip_suffix(&marker)
        .is_some_and(|prefix| prefix.len() < submitted.len() && submitted.starts_with(prefix))
}

/// Every string-rendered leaf of `input` this walk is allowed to collect, in walk order,
/// paired with the dotted member path the validating walk reports a violation on it at.
///
/// Public because restoring a violation's echo is not the only thing a service needs the
/// values a request SUBMITTED for: a service that runs its own check over a caller-
/// supplied value — an ARN it parses itself, ahead of the constraints the model declares
/// — needs the same (path, value) pairs for a value the framework never reports a
/// violation on. Reading them here keeps ONE walk in the tree: a second copy would mirror
/// the frozen crate's path bookkeeping twice and the two could drift apart.
///
/// The bounds the module docs state apply: at most [`MAX_RECORDED_LEAVES`] leaves and
/// [`MAX_RECORDED_BYTES`] of collected bytes, so this is never a second full copy of a
/// request's strings.
#[must_use]
pub fn submitted_leaves(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<(String, String)> {
    let mut walk = SubmittedValues::default();
    // The root input is a struct; drive it the way a codec — and the validating walk —
    // does.
    let _ = walk.write_struct(schema, input);
    walk.leaves
}

/// A frame of the collection stack, mirroring the one the validating walk keeps so a
/// leaf's path is spelled identically.
///
/// The generated `serialize_members` drives a list as a sequence of element writes and a
/// map as alternating key/value writes, both against the collection member's schema; a
/// struct pushes a shield, because its members report under their own names rather than
/// as elements of the collection enclosing the struct.
#[derive(Debug)]
enum Frame {
    /// A list: each element contributes `<1-based index>.member`.
    List { index: usize },
    /// A map: entries alternate key / value, and the captured key is the value's segment.
    Map {
        expecting_key: bool,
        pending_key: Option<String>,
    },
    /// Inside a struct's `serialize_members`: the enclosing collection is invisible.
    Struct,
}

/// A [`ShapeSerializer`] that records each string-rendered leaf of a built value with
/// the path the validating walk names it by, instead of producing output.
///
/// It checks nothing: the constraints have already been checked, and this walk exists
/// only to recover the values those checks elided. What it collects is bounded by
/// [`MAX_RECORDED_LEAVES`] and [`MAX_RECORDED_BYTES`], so neither its memory nor its
/// traversal is sized by the request.
#[derive(Debug, Default)]
struct SubmittedValues {
    /// Recorded leaves in walk order (a `@uniqueItems` duplicate is the first repeat in
    /// that order).
    leaves: Vec<(String, String)>,
    /// Owned bytes recorded so far: path plus value, both caller-sized.
    recorded: usize,
    /// Dotted path segments from the root to the value being written.
    path: Vec<String>,
    /// Active collection frames, innermost last.
    collections: Vec<Frame>,
}

impl SubmittedValues {
    /// True once no further leaf can be recorded, so the rest of the walk is skipped.
    fn done(&self) -> bool {
        self.leaves.len() >= MAX_RECORDED_LEAVES
    }

    /// The lowerCamel path segment for `schema`, if it is a member schema.
    fn segment_for(schema: &Schema) -> Option<String> {
        schema.member_name().map(path::member)
    }

    /// Push `schema`'s member-name segment, run `body`, then pop. A schema that is not a
    /// member schema (the root input) leaves the path unchanged.
    fn with_segment<R>(&mut self, schema: &Schema, body: impl FnOnce(&mut Self) -> R) -> R {
        match Self::segment_for(schema) {
            Some(segment) => {
                self.path.push(segment);
                let out = body(self);
                self.path.pop();
                out
            }
            None => body(self),
        }
    }

    /// The path segment for a child written directly inside the innermost collection
    /// frame, advancing list state; `None` when this write is not a collection child, so
    /// the caller falls back to the member-name segment.
    ///
    /// `terminal` is true when the child itself is the value: a list element names its
    /// position either way, while a map entry names just its key for the value itself and
    /// `<key>.member` when the walk is descending into it.
    fn begin_child(&mut self, terminal: bool) -> Option<String> {
        match self.collections.last_mut()? {
            Frame::List { index } => {
                *index += 1;
                Some(path::element(*index))
            }
            Frame::Map { pending_key, .. } => {
                let key = pending_key.clone().unwrap_or_default();
                Some(if terminal { key } else { path::entry(&key) })
            }
            Frame::Struct => None,
        }
    }

    /// After a child completes inside a map frame, expect the next entry's key.
    fn end_child(&mut self) {
        if let Some(Frame::Map { expecting_key, .. }) = self.collections.last_mut() {
            *expecting_key = true;
        }
    }

    /// Record the leaf now being written, when it has a string rendering and fits the
    /// bounds this walk states.
    fn record(&mut self, value: Option<&str>) {
        let Some(value) = value else {
            return;
        };
        // A value no clause could carry anyway (see `MAX_RESTORED_ECHO`), or one that
        // does not fit what is left of the byte budget, is not copied at all. A later,
        // smaller leaf can still be recorded, so this is not terminal.
        let leaf_path = path::join(&self.path);
        let bytes = leaf_path.len() + value.len();
        if value.len() > MAX_RESTORED_ECHO || self.recorded + bytes > MAX_RECORDED_BYTES {
            return;
        }
        self.recorded += bytes;
        self.leaves.push((leaf_path, value.to_string()));
    }

    /// One leaf write. `value` carries the leaf's text when its echo is the caller's own
    /// bytes (a string, or a big-number rendering) and is `None` for every other leaf
    /// kind — which is still walked, so a later leaf's path stays correct.
    fn leaf(&mut self, schema: &Schema, value: Option<&str>) {
        if self.done() {
            return;
        }
        // A map ENTRY KEY is written as a string against the map member's schema before
        // the entry's value. It is the value's path segment, not a leaf of its own.
        if let Some(Frame::Map {
            expecting_key,
            pending_key,
        }) = self.collections.last_mut()
        {
            if *expecting_key {
                *pending_key = value.map(str::to_string);
                *expecting_key = false;
                return;
            }
        }
        match self.begin_child(true) {
            Some(segment) => {
                self.path.push(segment);
                self.record(value);
                self.path.pop();
                self.end_child();
            }
            None => self.with_segment(schema, |this| this.record(value)),
        }
    }

    /// Walk a collection (list or map): a collection that is itself a collection element
    /// shares the enclosing member's schema, whose name the enclosing level already
    /// pushed, so it contributes no segment of its own.
    fn collection(
        &mut self,
        schema: &Schema,
        frame: Frame,
        write_children: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.done() {
            return Ok(());
        }
        let element = self.begin_child(false);
        let is_element = element.is_some();
        if let Some(segment) = element {
            self.path.push(segment);
        }
        let out = if is_element {
            self.walk_children(frame, write_children)
        } else {
            self.with_segment(schema, |this| this.walk_children(frame, write_children))
        };
        if is_element {
            self.path.pop();
            self.end_child();
        }
        out
    }

    /// Open `frame`, write the collection's children into it, then close it.
    fn walk_children(
        &mut self,
        frame: Frame,
        write_children: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.collections.push(frame);
        let out = write_children(self);
        self.collections.pop();
        out
    }
}

impl ShapeSerializer for SubmittedValues {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        if self.done() {
            return Ok(());
        }
        // A struct is the root input (no member name, no segment), a nested struct member
        // (member name -> segment), a list element or a map value (the frame supplies the
        // segment).
        match self.begin_child(false) {
            Some(segment) => {
                self.path.push(segment);
                self.collections.push(Frame::Struct);
                let out = value.serialize_members(self);
                self.collections.pop();
                self.path.pop();
                self.end_child();
                out
            }
            None => self.with_segment(schema, |this| {
                this.collections.push(Frame::Struct);
                let out = value.serialize_members(this);
                this.collections.pop();
                out
            }),
        }
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.collection(schema, Frame::List { index: 0 }, write_elements)
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.collection(
            schema,
            Frame::Map {
                expecting_key: true,
                pending_key: None,
            },
            write_entries,
        )
    }

    fn write_boolean(&mut self, schema: &Schema, _value: bool) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_byte(&mut self, schema: &Schema, _value: i8) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_short(&mut self, schema: &Schema, _value: i16) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_integer(&mut self, schema: &Schema, _value: i32) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_long(&mut self, schema: &Schema, _value: i64) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_float(&mut self, schema: &Schema, _value: f32) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_double(&mut self, schema: &Schema, _value: f64) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value.as_ref()));
        Ok(())
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value.as_ref()));
        Ok(())
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value));
        Ok(())
    }

    fn write_blob(&mut self, schema: &Schema, _value: &[u8]) -> Result<(), SerdeError> {
        // The walk renders a blob as its byte count, never its bytes, so there is no
        // caller-submitted text to re-echo here.
        self.leaf(schema, None);
        Ok(())
    }

    fn write_timestamp(&mut self, schema: &Schema, _value: &DateTime) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_document(&mut self, schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        self.leaf(schema, None);
        Ok(())
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        // Deliberately nothing, bug for bug with the walk this one mirrors: the
        // validating serializer's `write_null` also returns without touching the path or
        // the collection frames. A null therefore has the SAME effect on both walks'
        // bookkeeping (including leaving a map frame mid-entry), which is what keeps a
        // leaf's path here identical to the path the violation was reported at. Handling
        // it "better" here than there would spell some paths differently from the
        // violations this module has to match. Either way the outcome is fail-safe: a
        // path that does not match yields no leaf, so the framework's own text stands.
        // Generated `serialize_members` skips an absent member rather than writing null,
        // so this is reached only by a codec that writes one explicitly.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_elision_of, list_of_element, Submitted, MAX_RECORDED_BYTES, MAX_RECORDED_LEAVES,
        MAX_RESTORED_ECHO, MAX_RESTORED_TOTAL,
    };
    use aws_smithy_schema_server::constraints::{Violation, ViolationKind};

    /// RULE: a stored echo is the submitted value's elision when it is that value, or a
    /// proper prefix of it followed by the marker naming the value's byte count.
    #[test]
    fn an_elision_is_recognised_by_its_shape_not_by_a_cap() {
        let long = "t".repeat(300);
        let prefix: String = long.chars().take(256).collect();
        let rows: [(&str, String, bool); 6] = [
            ("the value itself", long.clone(), true),
            (
                "prefix plus the marker",
                format!("{prefix}... [300 bytes]"),
                true,
            ),
            (
                "prefix plus the marker, a shorter cap",
                format!("{}... [300 bytes]", "t".repeat(16)),
                true,
            ),
            (
                "the marker naming another byte count",
                format!("{prefix}... [301 bytes]"),
                false,
            ),
            (
                "a prefix of another value",
                format!("{}... [300 bytes]", "x".repeat(16)),
                false,
            ),
            ("an unrelated rendering", "300 items".to_string(), false),
        ];
        for (case, stored, expected) in rows {
            let actual = is_elision_of(&stored, &long);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected is_elision_of == {expected}, got {actual}"
            );
        }
    }

    /// RULE: a leaf is a scalar list element exactly when its path ends in a 1-based
    /// position followed by the collection step, and the list it belongs to is that path
    /// without those two segments.
    #[test]
    fn a_list_element_path_names_its_list() {
        let rows: [(&str, &str, Option<&str>); 7] = [
            ("a top-level list element", "tags.1.member", Some("tags")),
            (
                "a nested list element",
                "config.rules.12.member",
                Some("config.rules"),
            ),
            (
                "an element of a list inside a list",
                "outer.2.member.inner.3.member",
                Some("outer.2.member.inner"),
            ),
            ("a struct member", "trajectoryId", None),
            ("a nested struct member", "tags.1.member.value", None),
            ("a map entry value", "attributes.someKey", None),
            ("a map key spelled like the step", "attributes.member", None),
        ];
        for (case, leaf_path, expected) in rows {
            let actual = list_of_element(leaf_path);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the single index pass answers a clause's path with the leaf recorded there,
    /// and a `@uniqueItems` clause — reported at the LIST's path — with the first element
    /// value that repeated in walk order.
    #[test]
    fn the_index_answers_each_violation_class() {
        let leaves: Vec<(String, String)> = [
            ("trajectoryId", "the-id"),
            ("tags.1.member", "a"),
            ("tags.2.member", "b"),
            ("tags.3.member", "a"),
            ("attributes.someKey", "entry-value"),
        ]
        .into_iter()
        .map(|(leaf_path, value)| (leaf_path.to_string(), value.to_string()))
        .collect();
        let index = Submitted::of(&leaves);

        let rows: [(&str, &str, ViolationKind, Option<&str>); 5] = [
            (
                "a member clause",
                "trajectoryId",
                ViolationKind::MaxLength(256),
                Some("the-id"),
            ),
            (
                "a list-element clause",
                "tags.2.member",
                ViolationKind::MaxLength(1),
                Some("b"),
            ),
            (
                "a map-entry clause",
                "attributes.someKey",
                ViolationKind::Pattern("^x$".to_string()),
                Some("entry-value"),
            ),
            (
                "a uniqueItems clause at the list",
                "tags",
                ViolationKind::UniqueItems,
                Some("a"),
            ),
            (
                "a clause on a member no leaf was recorded for",
                "clientToken",
                ViolationKind::MinLength(1),
                None,
            ),
        ];
        for (case, leaf_path, kind, expected) in rows {
            let violation = Violation {
                path: leaf_path.to_string(),
                kind,
                value: Some("whatever".to_string()),
            };
            let actual = index.candidate(&violation);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the stated bounds are finite and ordered — no single echo may exceed the
    /// per-message budget, and nothing is collected that could never be restored.
    #[test]
    fn the_stated_bounds_are_coherent() {
        const COHERENT: () = {
            assert!(MAX_RESTORED_ECHO <= MAX_RESTORED_TOTAL);
            assert!(MAX_RECORDED_BYTES <= MAX_RESTORED_TOTAL);
            assert!(MAX_RECORDED_LEAVES > 0 && MAX_RECORDED_BYTES > 0);
        };
        let () = COHERENT;
    }
}


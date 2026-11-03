// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-validation refusal: how a set of constraint
//! violations becomes the `Message` the modelled `ValidationException` carries.
//!
//! [`crate::service`] owns the SEAM — which requests are validated and how the modelled
//! shape is serialized — and this module owns the sentence, so the clause rules are in one
//! place rather than spread through the seam that applies them. What the rules enforce is
//! always the model's: every violation handed to [`refusal_message`] comes from the
//! framework's own boundary walk.
//!
//! The rules, each holding for every member of every operation rather than for the members
//! a report happened to cite:
//!
//! 1. the envelope is AWS's `<N> validation error(s) detected: ` over one clause per
//!    failing member, joined with `; ` ([`refusal_message`]);
//! 2. a clause states the submitted value, the member's dotted path and the requirement it
//!    failed — except where the MODEL marks the value's shape, the member declaring it, or
//!    (for a collection, whose rendering states what it holds) its elements `@sensitive`,
//!    whose value is never echoed ([`is_sensitive`], [`subject`]);
//! 3. a reported `@pattern` is the expression without the `^`/`$` anchors a model writes
//!    to demand a whole-string match ([`unanchored`]);
//! 4. a violation on the ELEMENTS of a list member is stated once, at that member, with
//!    the element's requirement nested and, where the elements have a rendering of their
//!    own, the list itself as the value ([`list_element_path`], [`list_values`]);
//! 5. the clauses come out in the reverse of the order the walk found them — the order
//!    this service's recorded multi-violation answers show (see [`refusal_message`]).
//!
//! NOTE on where this belongs: rules 1-4 are AWS's wording rather than this service's, and
//! ~24 sibling service crates carry their own copies of parts of them
//! (`grep -rln 'fn unanchored' services/*/src`). `services/common/src/validation.rs` is
//! the staged home for exactly those clause rules; as its header asks, this is the note
//! rather than a silent further copy — migrating them re-words services whose recorded
//! behaviour their own suites pin, so it is one change of its own. What is genuinely this
//! service's own and would stay here is the clause order.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};
use serde_json::{Map, Value};

/// This service's Smithy model, read at compile time from the SAME file this crate's
/// shapes are generated from.
///
/// Which values a clause withholds is the model's own `@sensitive`, so it is read off the
/// model rather than restated here as a list of member NAMES: a name is not the trait —
/// two filters of this service spell a member `Names`, one targeting a list of a
/// `@sensitive` string and the other a list of a plain structure — and a restated list
/// goes stale the moment the model changes.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-billingconductor-awsbillingconductor.json"
));

/// The trait a model writes on a shape, or on the member declaring it, whose value is
/// never echoed back.
const SENSITIVE_TRAIT: &str = "smithy.api#sensitive";

/// The Smithy member name a violation path carries for the step into a collection
/// (`<list>.<1-based position>.member`, `<map>.<key>.member`), as
/// [`constraints::path`] spells it. It names a position, not a member of the collection.
const COLLECTION_STEP: &str = "member";

/// How many collection levels deep a sensitivity test follows a value's shape before it
/// stops. Nothing in a Smithy model nests collections anywhere near this far; the bound
/// is what keeps the descent finite for any model at all.
const MAX_COLLECTION_DEPTH: usize = 8;

/// The model's shapes, by absolute shape id, parsed ONCE — on the first refusal this
/// service words, and never for a request it serves.
static SHAPES: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    match serde_json::from_str::<Value>(MODEL) {
        Ok(Value::Object(mut model)) => match model.remove("shapes") {
            Some(Value::Object(shapes)) => shapes,
            _ => Map::new(),
        },
        // A model this crate's own shapes were generated from parses; a model that did
        // not places no path, which [`is_sensitive`] answers in the direction that
        // cannot leak.
        _ => Map::new(),
    }
});

/// Ceiling on a value rendered INTO a message, in bytes.
///
/// The same reason the framework elides the value it stores on a [`Violation`]: a caller
/// controls the length of what it sends, and a clause interpolates it, so an unelided
/// list of 30 multi-kilobyte strings would amplify one request into an arbitrarily long
/// 400 body.
const MAX_ECHOED_VALUE: usize = 256;

/// The `[` and `]` a list rendering always carries, which both the kept-bytes budget and
/// the reported total count from.
const LIST_BRACKETS: usize = 2;

/// The `Message` for a set of constraint violations, as AWS builds it from the request:
/// the count, then one clause per failing member.
///
/// `<N> validation error(s) detected: ` — singular for one — over
/// `<subject> at '<member path>' failed to satisfy constraint: <requirement>` clauses
/// joined with `; `. A violation found on a list ELEMENT is reported at the LIST member,
/// with the element's requirement nested
/// (`Member must satisfy constraint: [<requirement>]`) and the list rendered as the value
/// that member holds, which is why two bad elements of one list are ONE error rather than
/// two.
///
/// ORDER: the clauses come out in the REVERSE of the order the boundary walk finds them,
/// which for the top-level members of an input is the reverse of the order the model
/// declares them. That is what this service's recorded multi-violation answers show, in
/// both fixtures that carry one: `AssociateAccounts` states `accountIds` (declared second)
/// before `arn`, and `AssociatePricingRules` states `pricingRuleArns` (declared second)
/// before `arn`. No ordering by member PATH explains both — ascending puts `arn` before
/// `pricingRuleArns`, descending puts it before `accountIds` — so the order is read off the
/// walk rather than off the names, which is one rule over every violation instead of a case
/// keyed on a member's name or type. It is still a total, deterministic order: the walk
/// visits a given input shape in one order.
///
/// `lists` carries the already-rendered value of every list member of the request (see
/// [`list_values`]); it may be empty, in which case the value the framework stored on the
/// violation is used. It is rendered ONCE per list member, before this loop, because the
/// walk reports one violation per failing ELEMENT: rendering it here would rebuild the
/// whole list for each of them, which is quadratic in a length the caller chooses.
///
/// `input` is the absolute shape id of the input shape the violations were found against,
/// and is what a member path is read as a position in the MODEL ([`is_sensitive`]); it is
/// `None` for a refusal that named no modelled operation, where there is no input shape to
/// read a path against.
///
/// `None` declines the whole refusal — a violation class this service has no wording for
/// ([`ViolationKind`] is `#[non_exhaustive]`) keeps the framework's own.
pub(super) fn refusal_message(
    violations: &[Violation],
    lists: &HashMap<String, String>,
    input: Option<&str>,
) -> Option<String> {
    // One clause per (member path, requirement): the several element violations a list
    // member can carry fold into the one clause stated at that member, while a member that
    // broke two different rules still states both.
    let mut stated: HashSet<(String, String)> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let (path, element) = match list_element_path(&violation.path) {
            Some(list) => (list.to_string(), true),
            None => (violation.path.clone(), false),
        };
        let requirement = match requirement_of(&violation.kind) {
            Some(requirement) if element => {
                format!("Member must satisfy constraint: [{requirement}]")
            }
            Some(requirement) => requirement,
            None => return None,
        };
        if !stated.insert((path.clone(), requirement.clone())) {
            continue;
        }
        let subject = subject(input, &path, violation, lists);
        clauses.push(format!(
            "{subject} at '{path}' failed to satisfy constraint: {requirement}"
        ));
    }
    clauses.reverse();
    let count = clauses.len();
    let joined = clauses.join("; ");
    match count {
        0 => None,
        1 => Some(format!("1 validation error detected: {joined}")),
        n => Some(format!("{n} validation errors detected: {joined}")),
    }
}

/// The rule half of a clause: what the member had to satisfy.
///
/// `None` for a violation class this service has no wording for.
fn requirement_of(kind: &ViolationKind) -> Option<String> {
    let requirement = match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        // The requirement reported is the expression itself, without the `^` / `$` the
        // model spells it with: the service states the pattern a value must satisfy, not
        // the anchoring Smithy uses to make the match total, so a client that
        // re-validates locally with the pattern it read out of the message uses the one
        // AWS documents.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    Some(requirement)
}

/// The value half of a clause: what the member held.
///
/// * an ABSENT member has no value at all and is reported as the null it was;
/// * a value the model marks `@sensitive` names no value — the constraint it broke is the
///   whole of what a caller needs, and the value belongs in nobody's logs;
/// * a LIST member is reported as the list the caller sent, bracketed and already bounded
///   by [`rendered_list`];
/// * everything else echoes the value the framework stored, already elided.
fn subject(
    input: Option<&str>,
    path: &str,
    violation: &Violation,
    lists: &HashMap<String, String>,
) -> String {
    // A value the validating walk itself withheld is a withheld value and not an absent
    // one: the walk drops a `@sensitive` member's value as it records the violation, so
    // an EMPTY value half is not evidence that the member was absent. A value that DID
    // reach this clause is held to the model's own reading of the member, which is what
    // answers a member the walk did not mark.
    if violation.sensitive || (violation.value.is_some() && is_sensitive(input, path)) {
        return "Value".to_string();
    }
    if violation.value.is_none() {
        return "Value null".to_string();
    }
    if let Some(rendered) = lists.get(path) {
        return format!("Value '{rendered}'");
    }
    match &violation.value {
        Some(shown) => format!("Value '{shown}'"),
        None => "Value null".to_string(),
    }
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$` escaped
/// by a backslash is a literal dollar sign inside the expression, not the end anchor, so
/// it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The list member a violation found on one of its ELEMENTS belongs to: the path with
/// the trailing `<1-based index>.member` step the validating walk adds removed, or
/// `None` when the path does not end in one (so it names the member itself).
fn list_element_path(path: &str) -> Option<&str> {
    let head = path.strip_suffix(".member")?;
    let (list, index) = head.rsplit_once('.')?;
    let indexed = !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit());
    indexed.then_some(list)
}

/// Does the value `path` names hold something the MODEL marks `@sensitive`, so the clause
/// about it states the constraint and no value?
///
/// Decided from the model, never from how a member is SPELLED: `path` is walked through
/// the model's own declarations from `input`, the input shape the violations were found
/// against, and the value it lands on is sensitive when that declaration — the member, the
/// shape it targets, or, where that shape is a collection, the elements (or keys and
/// values) a rendering of it would state — carries the trait. So the two members this
/// service spells `names` are answered differently, as the model declares them: one
/// targets a list of a `@sensitive` string and keeps its value out of the message, the
/// other a list of a plain structure and states what was sent.
///
/// A path the model cannot place — and every path when the refusal named no modelled
/// operation, so there is no input shape to read it against — is treated as sensitive: a
/// value whose declaration cannot be found is one whose sensitivity is unknown, and
/// stating no value is the only direction that cannot put a secret in a response.
fn is_sensitive(input: Option<&str>, path: &str) -> bool {
    let shapes = &*SHAPES;
    let Some((member, shape)) = input.and_then(|input| declared_at(shapes, input, path)) else {
        return true;
    };
    marked_sensitive(member) || sensitive_value(shapes, shape, MAX_COLLECTION_DEPTH)
}

/// The model declarations a violation path names: the MEMBER it ends at and the shape
/// that member targets, or `None` for a path the model cannot place.
///
/// Each segment is read against the shape reached so far. A named segment is a member of
/// that shape, compared in the one lowerCamel spelling every violation path is written in
/// ([`constraints::path::member`], the same call the walk that produced the path makes). A
/// segment that steps INTO a collection — a list element's 1-based position or a map
/// entry's caller-chosen key, each followed by [`COLLECTION_STEP`] when the path descends
/// through it — names a position rather than a member, so the collection's element (or
/// value) declaration is what the rest of the path is read against.
fn declared_at<'m>(
    shapes: &'m Map<String, Value>,
    input: &str,
    path: &str,
) -> Option<(&'m Value, &'m Value)> {
    let mut shape = shapes.get(input)?;
    let mut member: Option<&Value> = None;
    let mut segments = path.split('.').peekable();
    while let Some(segment) = segments.next() {
        let declaration = match shape_type(shape) {
            Some(collection @ ("list" | "map")) => {
                if segments.peek() == Some(&COLLECTION_STEP) {
                    segments.next();
                }
                let slot = if collection == "list" { "member" } else { "value" };
                shape.get(slot)?
            }
            _ => members_of(shape)?
                .iter()
                .find(|(name, _)| constraints::path::member(name) == segment)
                .map(|(_, declaration)| declaration)?,
        };
        shape = target_of(shapes, declaration)?;
        member = Some(declaration);
    }
    Some((member?, shape))
}

/// A shape's `type`, as the model spells it.
fn shape_type(shape: &Value) -> Option<&str> {
    shape.get("type")?.as_str()
}

/// The members a structure or union shape declares.
fn members_of(shape: &Value) -> Option<&Map<String, Value>> {
    shape.get("members")?.as_object()
}

/// The shape a member declaration targets.
fn target_of<'m>(shapes: &'m Map<String, Value>, declaration: &Value) -> Option<&'m Value> {
    shapes.get(declaration.get("target")?.as_str()?)
}

/// Does this declaration — a shape, or the member declaring one — carry `@sensitive`?
fn marked_sensitive(declaration: &Value) -> bool {
    declaration
        .get("traits")
        .and_then(|traits| traits.get(SENSITIVE_TRAIT))
        .is_some()
}

/// Is a value of `shape` one the model keeps out of a message: the shape itself marked
/// `@sensitive`, or — where it is a collection, whose rendering states what it holds — its
/// elements, or a map's keys and values, declared or targeted at one that is?
///
/// A STRUCTURE is not descended into: a clause states a structure member only as the null
/// it was when absent, so no member of one is ever rendered into a message.
fn sensitive_value(shapes: &Map<String, Value>, shape: &Value, depth: usize) -> bool {
    if marked_sensitive(shape) {
        return true;
    }
    // Past the bound the shape of the value is no longer known here, which is the same
    // unknown [`is_sensitive`] answers by stating no value.
    if depth == 0 {
        return true;
    }
    let slots: &[&str] = match shape_type(shape) {
        Some("list") => &["member"],
        Some("map") => &["key", "value"],
        _ => return false,
    };
    slots.iter().any(|slot| {
        shape.get(*slot).is_some_and(|declaration| {
            marked_sensitive(declaration)
                || target_of(shapes, declaration)
                    .is_some_and(|target| sensitive_value(shapes, target, depth - 1))
        })
    })
}

/// One list member as a clause states it: the elements the caller sent, joined and
/// bracketed, with the length of the WHOLE list reported when it does not fit.
///
/// Bounded as it is BUILT, not after: `elements` already stops at the echo budget (see
/// [`ListValues::record`]) and the join stops once [`MAX_ECHOED_VALUE`] bytes are in hand,
/// so a request carrying a list of multi-kilobyte strings cannot make the server
/// materialise the whole concatenation to then throw it away. `total` is the length the
/// untruncated rendering would have had — counted as the elements were collected — so the
/// message still says how long what the caller sent really was. The cut is taken at a
/// character boundary, so the prefix is always valid UTF-8.
fn rendered_list(elements: &[String], total: usize) -> String {
    let mut rendered = String::with_capacity(MAX_ECHOED_VALUE.min(total).saturating_add(1));
    rendered.push('[');
    for (index, element) in elements.iter().enumerate() {
        // Position decides the separator, not the length of what is already written: an
        // element that renders empty still takes its place in the list.
        if index > 0 {
            rendered.push_str(", ");
        }
        rendered.push_str(element);
        if rendered.len() > MAX_ECHOED_VALUE {
            break;
        }
    }
    if total <= MAX_ECHOED_VALUE {
        rendered.push(']');
        return rendered;
    }
    let mut end = MAX_ECHOED_VALUE.min(rendered.len());
    while end > 0 && !rendered.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{total} bytes]", &rendered[..end])
}

/// Every LIST member of a request whose elements a clause can state, keyed by the
/// lowerCamel member path the validating walk reports it under, and rendered ONCE (see
/// [`rendered_list`]).
///
/// A constraint on a list's element shape is violated by ONE element, and the framework
/// reports it at that element (`accountIds.1.member`); AWS reports it at the list member
/// and states the value that member holds — the whole list, not the element. That value
/// is not on the violation, so it is read off the request here, through the same
/// `SerializableStruct` walk the validator uses.
///
/// A list is collected only when EVERY element it holds rendered (see
/// [`ListValues::write_list`]): the rendering states the elements, so a list whose
/// elements have no rendering of their own — a list of structures — would otherwise be
/// stated as a shorter list than the caller sent.
pub(super) fn list_values(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> HashMap<String, String> {
    let mut capture = ListValues::default();
    // Rendering only: a walk that cannot be completed yields the paths it reached, and a
    // path it did not reach simply falls back to the violation's own value.
    let _ = capture.write_struct(schema, input);
    capture.values
}

/// Where in the value tree [`ListValues`] currently is. Only a LIST collects: a struct
/// or a map is opaque, so a leaf written inside one is not an element of the list that
/// encloses it.
#[derive(Debug)]
enum Frame {
    /// A list, as its elements are collected.
    List {
        /// The element renderings kept for the message. Collection stops once
        /// [`MAX_ECHOED_VALUE`] bytes are in hand, so one request cannot make the server
        /// hold a copy of an arbitrarily long list.
        elements: Vec<String>,
        /// How many elements rendered at all, kept or dropped for the budget — the
        /// measure of whether this list can be stated as its elements.
        rendered: usize,
        /// How many elements the list holds, each one counted whether it rendered or not.
        written: usize,
        /// The bytes `elements` would render as, the measure the budget above is spent
        /// against.
        kept: usize,
        /// The bytes the WHOLE rendering would have, every element counted — what
        /// [`rendered_list`] reports when it cannot fit.
        total: usize,
    },
    Opaque,
}

/// A [`ShapeSerializer`] that accumulates the rendering of each list member instead of
/// producing output — the read half of [`list_values`].
///
/// It walks the same `serialize_members` tree the validator walks and spells a member
/// path the same way (`constraints::path::member`), so the keys it produces are the
/// paths a [`Violation`] carries.
#[derive(Debug, Default)]
struct ListValues {
    /// Member-name segments from the root to the value being written.
    path: Vec<String>,
    /// Open frames, innermost last.
    frames: Vec<Frame>,
    /// The rendered value of every list member reached so far.
    values: HashMap<String, String>,
}

impl ListValues {
    /// Push `schema`'s member-name segment, if it has one. The root input struct and a
    /// collection element carry no member name, so they contribute no segment.
    fn push_segment(&mut self, schema: &Schema) -> bool {
        match schema.member_name() {
            Some(name) => {
                self.path.push(constraints::path::member(name));
                true
            }
            None => false,
        }
    }

    fn pop_segment(&mut self, pushed: bool) {
        if pushed {
            self.path.pop();
        }
    }

    /// Record one rendered leaf, when it is an element of the innermost list.
    ///
    /// EVERY element counts towards `total` — that is the length the message reports — but
    /// only the ones a bounded rendering can use are kept. The first element is always
    /// kept, so `elements` being empty is exactly "this is the first element", which is
    /// what decides the `, ` separator.
    fn record(&mut self, rendered: &str) {
        if let Some(Frame::List {
            elements,
            rendered: count,
            written,
            kept,
            total,
        }) = self.frames.last_mut()
        {
            *count = count.saturating_add(1);
            *written = written.saturating_add(1);
            // `, ` before every element but the first: the same shape `rendered_list`
            // builds.
            let separator: usize = if elements.is_empty() { 0 } else { 2 };
            let width = separator.saturating_add(rendered.len());
            *total = total.saturating_add(width);
            if *kept <= MAX_ECHOED_VALUE {
                *kept = kept.saturating_add(width);
                elements.push(rendered.to_string());
            }
        }
    }

    /// Count one element of the innermost list that has no rendering of its own: a
    /// structure, or a collection. It takes its place in the list's length without
    /// contributing to the rendering, which is what makes that rendering incomplete.
    fn note_unrendered(&mut self) {
        if let Some(Frame::List { written, .. }) = self.frames.last_mut() {
            *written = written.saturating_add(1);
        }
    }
}

impl ShapeSerializer for ListValues {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.note_unrendered();
        let pushed = self.push_segment(schema);
        self.frames.push(Frame::Opaque);
        let out = value.serialize_members(self);
        self.frames.pop();
        self.pop_segment(pushed);
        out
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.note_unrendered();
        let pushed = self.push_segment(schema);
        self.frames.push(Frame::List {
            elements: Vec::new(),
            rendered: 0,
            written: 0,
            kept: LIST_BRACKETS,
            total: LIST_BRACKETS,
        });
        let out = write_elements(self);
        if let Some(Frame::List {
            elements,
            rendered,
            written,
            total,
            ..
        }) = self.frames.pop()
        {
            // The rendering STATES the elements, so it is only this member's value when
            // every element the caller sent is in it: a list of structures renders no
            // element at all, and stating it as the empty list would report a value the
            // caller never sent. Such a member is left out, and its clause keeps the
            // value the framework stored on the violation.
            if rendered == written {
                self.values
                    .insert(self.path.join("."), rendered_list(&elements, total));
            }
        }
        self.pop_segment(pushed);
        out
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.note_unrendered();
        let pushed = self.push_segment(schema);
        // A map's keys and values are not list elements, and AWS states a violation
        // inside one at the entry rather than at the map, so nothing here collects.
        self.frames.push(Frame::Opaque);
        let out = write_entries(self);
        self.frames.pop();
        self.pop_segment(pushed);
        out
    }

    fn write_boolean(&mut self, _schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.record(&value.to_string());
        Ok(())
    }

    fn write_byte(&mut self, _schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.record(&value.to_string());
        Ok(())
    }

    fn write_short(&mut self, _schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.record(&value.to_string());
        Ok(())
    }

    fn write_integer(&mut self, _schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.record(&value.to_string());
        Ok(())
    }

    fn write_long(&mut self, _schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.record(&value.to_string());
        Ok(())
    }

    fn write_float(&mut self, _schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.record(&value.to_string());
        Ok(())
    }

    fn write_double(&mut self, _schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.record(&value.to_string());
        Ok(())
    }

    fn write_big_integer(&mut self, _schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        let rendered: &str = value.as_ref();
        self.record(rendered);
        Ok(())
    }

    fn write_big_decimal(&mut self, _schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        let rendered: &str = value.as_ref();
        self.record(rendered);
        Ok(())
    }

    fn write_string(&mut self, _schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.record(value);
        Ok(())
    }

    fn write_blob(&mut self, _schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        // The bytes themselves are not a rendering a message can carry; the framework's
        // own validator states a blob leaf the same way.
        self.record(&format!("{} bytes", value.len()));
        Ok(())
    }

    fn write_timestamp(&mut self, _schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.record(&value.to_string());
        Ok(())
    }

    fn write_document(&mut self, _schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        self.record("<document>");
        Ok(())
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        // A null element of a list has no rendering, so it takes its place in the list's
        // length without contributing one — the same incompleteness a structure element
        // carries, and what keeps the member from being stated as a shorter list than was
        // sent. An absent struct MEMBER reaches here too, and counts for nothing: the
        // innermost frame is then the struct's, not a list's.
        self.note_unrendered();
        Ok(())
    }
}

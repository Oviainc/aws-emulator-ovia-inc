// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Protocol-agnostic constraint-trait validation.
//!
//! Smithy constraint traits (`@length`, `@range`, `@pattern`, `@enum`,
//! `@uniqueItems`) describe structural rules an input must satisfy. `@required`
//! is NOT among them: this walk reads a value that was already built, and a
//! member absent from the request leaves nothing to visit, so absence is caught
//! during deserialization instead. See [`missing_member_message`].
//! `aws-smithy-schema` is a frozen registry crate, so we cannot add typed
//! `with_length()` fields to [`Schema`](aws_smithy_schema::Schema). Instead we
//! capture each constraint as a [`Trait`](aws_smithy_schema::Trait) object on
//! the member schema's generic [`TraitMap`](aws_smithy_schema::TraitMap), and
//! enforce them with a single generic walk over the built input value.
//!
//! # The validating walk
//!
//! Every generated input struct already implements
//! [`SerializableStruct`](aws_smithy_schema::serde::SerializableStruct), whose
//! `serialize_members` threads the member `&Schema` through every leaf write
//! (`write_string(&M_TABLE_NAME, &self.table_name)`, …). A
//! [`ShapeSerializer`](aws_smithy_schema::serde::ShapeSerializer) is documented
//! as a format-agnostic visitor — **a validator is just another "format."**
//!
//! [`ValidatingSerializer`] implements `ShapeSerializer`. On each leaf it looks
//! up constraint traits on the passed `&Schema` and runs them, **pushing**
//! [`Violation`]s into an internal `Vec` rather than returning `Err` — so the
//! walk continues and violations accumulate. [`validate`] is the entry point.
//!
//! # Path fidelity
//!
//! The walk maintains a path stack producing the dotted path style AWS uses in
//! `ValidationException` messages. Member names are lowerCamel; list elements
//! contribute a `<1-based-index>.member` segment (`keySchema.1.member.keyType`);
//! map entries contribute the string key as a segment. Collection-level
//! `@length` (list size / map entry count) is checked on the collection member
//! itself, before recursing into elements.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, Trait, TraitMap};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// A single constraint violation discovered during the validating walk.
///
/// `path` is the dotted path to the offending member (e.g. `tableName`), `kind`
/// is what it failed, and `value` is what it held. The clause a message states
/// (e.g. `Value '' at 'tableName' failed to satisfy constraint: Member must have
/// length greater than or equal to 1`) is produced by [`Violation::message`],
/// not stored here — so the words can be chosen elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Dotted path to the offending member.
    pub path: String,
    /// What the value failed, described structurally.
    pub kind: ViolationKind,
    /// The offending value as displayed, or `None` when there was no value —
    /// i.e. the member was absent.
    ///
    /// ALREADY elided to `MAX_ECHOED_VALUE`: a caller controls its length, and
    /// [`Violation::message`] interpolates it as-is, so a value stored unelided
    /// would amplify a request into an arbitrarily long response.
    pub value: Option<String>,
    /// The member is `@sensitive`: `value` is `None` and the message names only the path, so the
    /// value never reaches a response or an author's refusal override.
    pub sensitive: bool,
}

/// How a dotted member path is spelled.
///
/// Both subsystems that build one — the deserializer, as it unwinds out of an
/// absent member, and the validating walk, as it descends into a built value —
/// go through here, so the two cannot drift into different spellings of the
/// same position.
pub mod path {
    /// A member names itself in lowerCamel. That casing belongs to the path, not
    /// to the model, so it is applied here rather than stored on the member.
    pub fn member(smithy_name: &str) -> String {
        super::to_lower_camel(smithy_name)
    }

    /// A list element names its 1-based position, then the collection step.
    pub fn element(position: usize) -> String {
        format!("{position}.{COLLECTION_STEP}")
    }

    /// A map entry names its key, then the collection step. The key is a value
    /// the CALLER chose, so it is echoed exactly as sent — never re-cased the
    /// way a model name is.
    pub fn entry(key: &str) -> String {
        format!("{key}.{COLLECTION_STEP}")
    }

    /// Join segments into the path a message states.
    pub fn join(segments: &[String]) -> String {
        segments.join(".")
    }

    /// The segment marking a step from a collection into its element.
    const COLLECTION_STEP: &str = "member";
}

/// What a value failed.
///
/// Carries the facts a sentence would state rather than the sentence, so the
/// wording can be chosen by whoever is emulating a particular service. Numeric
/// bounds are held as the text they display with, which keeps a bound's
/// rendering in the one place that owns the trait.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ViolationKind {
    /// A member the shape requires was absent from the request.
    MissingRequiredMember,
    /// Shorter than `@length`'s minimum.
    MinLength(i64),
    /// Longer than `@length`'s maximum.
    MaxLength(i64),
    /// Below `@range`'s minimum.
    MinValue(String),
    /// Above `@range`'s maximum.
    MaxValue(String),
    /// Outside `@pattern`.
    Pattern(String),
    /// Outside an enumerated type's value set.
    EnumValue(Vec<String>),
    /// `@uniqueItems` with a repeated element.
    UniqueItems,
}

impl Violation {
    /// The requirement this violation states, as a service phrases it.
    fn requirement(&self) -> String {
        match &self.kind {
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
            ViolationKind::Pattern(pattern) => {
                format!("Member must satisfy regular expression pattern: {pattern}")
            }
            ViolationKind::EnumValue(allowed) => {
                format!(
                    "Member must satisfy enum value set: [{}]",
                    allowed.join(", ")
                )
            }
            ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        }
    }

    /// The full clause that goes inside the envelope. The ONE place this sentence
    /// is written: every constraint reaches it through [`Violation::requirement`],
    /// so a new constraint cannot drift from the others by spelling its own.
    pub fn message(&self) -> String {
        let requirement = self.requirement();
        if self.sensitive {
            return format!(
                "Value at '{}' failed to satisfy constraint: {requirement}",
                self.path
            );
        }
        match &self.value {
            Some(shown) => format!(
                "Value '{shown}' at '{}' failed to satisfy constraint: {requirement}",
                self.path
            ),
            // An absent member has no value to echo.
            None => format!(
                "Value null at '{}' failed to satisfy constraint: {requirement}",
                self.path
            ),
        }
    }
}

// =============================================================================
// Constraint trait types
// =============================================================================

/// The runtime representation of the Smithy `@length` trait.
///
/// Attached to a member schema's [`TraitMap`] by codegen and looked up at
/// validation time. Applies to string, blob, list and map members; the length
/// being checked is the element/byte/entry count of the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LengthTrait {
    /// Inclusive minimum length, if any.
    pub min: Option<i64>,
    /// Inclusive maximum length, if any.
    pub max: Option<i64>,
}

impl LengthTrait {
    /// The Smithy shape id of the `@length` trait (`smithy.api#length`).
    pub const TRAIT_ID: ShapeId = ShapeId::from_static("smithy.api#length", "smithy.api", "length");

    /// Construct a `@length` trait from optional bounds.
    pub const fn new(min: Option<i64>, max: Option<i64>) -> Self {
        Self { min, max }
    }

    /// Check `len` against the bounds, emitting a validation clause for the first
    /// failing bound. `value` is the rendered display of the offending value
    /// and `path` is its dotted path; both are used to build the message.
    fn check(&self, len: usize, value: &str, path: &str) -> Option<Violation> {
        if let Some(min) = self.min {
            if (len as i64) < min {
                return Some(Violation {
                    path: path.to_string(),
                    kind: ViolationKind::MinLength(min),
                    value: Some(echoed(value).into_owned()),
                    sensitive: false,
                });
            }
        }
        if let Some(max) = self.max {
            if (len as i64) > max {
                return Some(Violation {
                    path: path.to_string(),
                    kind: ViolationKind::MaxLength(max),
                    value: Some(echoed(value).into_owned()),
                    sensitive: false,
                });
            }
        }
        None
    }
}

impl Trait for LengthTrait {
    fn trait_id(&self) -> &ShapeId {
        &Self::TRAIT_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The runtime representation of the Smithy `@range` trait.
///
/// Applies to numeric members (byte/short/integer/long/float/double). Bounds
/// are stored as `f64` so a single type covers integral and floating-point
/// members; the value under test is widened to `f64` for comparison. Integral
/// bounds render without a decimal point to match AWS (`... value greater than
/// or equal to 1`, not `1.0`).
///
// TODO: the Smithy `@range` trait models its bounds as `BigDecimal`, so an
// `f64` bound loses precision for `long`/`BigInteger`/`BigDecimal` members with
// magnitudes beyond f64's 53-bit mantissa (e.g. an i64 bound near i64::MAX). DDB
// has no such bounds today, so f64 is sufficient for the prototype; switch the
// bounds (and the widening in `check`) to `BigDecimal` if a precise large-bound
// member ever appears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeTrait {
    /// Inclusive minimum, if any.
    pub min: Option<f64>,
    /// Inclusive maximum, if any.
    pub max: Option<f64>,
}

impl RangeTrait {
    /// The Smithy shape id of the `@range` trait (`smithy.api#range`).
    pub const TRAIT_ID: ShapeId = ShapeId::from_static("smithy.api#range", "smithy.api", "range");

    /// Construct a `@range` trait from optional bounds.
    pub const fn new(min: Option<f64>, max: Option<f64>) -> Self {
        Self { min, max }
    }

    fn check(&self, n: f64, value: &str, path: &str) -> Option<Violation> {
        // Echoing through `echoed` below is DEFENSIVE, not load-bearing: `value` is
        // a rendered `f64`, which cannot exceed the cap, and `check_range` is only
        // reached when the walk widened a numeric leaf. It is kept rather than
        // dropped because `@range` on bigInteger/bigDecimal is unenforced TODAY and
        // enforcing it is a follow-up — at which point `value` becomes arbitrarily
        // long and an unbounded echo here reintroduces exactly the amplification
        // `MAX_ECHOED_VALUE` exists to stop.
        if let Some(min) = self.min {
            if n < min {
                return Some(Violation {
                    path: path.to_string(),
                    kind: ViolationKind::MinValue(render_bound(min)),
                    value: Some(echoed(value).into_owned()),
                    sensitive: false,
                });
            }
        }
        if let Some(max) = self.max {
            if n > max {
                return Some(Violation {
                    path: path.to_string(),
                    kind: ViolationKind::MaxValue(render_bound(max)),
                    value: Some(echoed(value).into_owned()),
                    sensitive: false,
                });
            }
        }
        None
    }
}

impl Trait for RangeTrait {
    fn trait_id(&self) -> &ShapeId {
        &Self::TRAIT_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Render a `@range` bound the way AWS does: an integral value without a
/// trailing `.0` (`1`, not `1.0`), a fractional value with its decimals.
fn render_bound(b: f64) -> String {
    if b.fract() == 0.0 && b.is_finite() {
        format!("{}", b as i64)
    } else {
        format!("{b}")
    }
}

/// The runtime representation of the Smithy `@pattern` trait.
///
/// Applies to string members. The `regex` is the Smithy pattern verbatim
/// (e.g. `^[a-zA-Z0-9_.-]+$`); Smithy pattern semantics are a *find* (the
/// regex must match somewhere in the value), which is exactly
/// [`regex::Regex::is_match`]. Compiled patterns are cached so each distinct
/// pattern compiles at most once across the whole process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternTrait {
    /// The Smithy regular expression the value must satisfy.
    pub regex: &'static str,
}

impl PatternTrait {
    /// The Smithy shape id of the `@pattern` trait (`smithy.api#pattern`).
    pub const TRAIT_ID: ShapeId =
        ShapeId::from_static("smithy.api#pattern", "smithy.api", "pattern");

    /// Construct a `@pattern` trait from its regex source.
    pub const fn new(regex: &'static str) -> Self {
        Self { regex }
    }

    fn check(&self, value: &str, path: &str) -> Option<Violation> {
        if pattern_matches(self.regex, value) {
            return None;
        }
        Some(Violation {
            path: path.to_string(),
            kind: ViolationKind::Pattern(self.regex.to_string()),
            value: Some(echoed(value).into_owned()),
            sensitive: false,
        })
    }
}

impl Trait for PatternTrait {
    fn trait_id(&self) -> &ShapeId {
        &Self::TRAIT_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Ceiling on accumulated violations for one request.
///
/// The walk accumulates by default, and every clause interpolates the rejected value
/// AND the violated bound, so cost is O(violations x value size) with no natural
/// limit. A request whose body is a large list of structs with several constrained
/// members each turns a few MiB of request into tens of MiB of error message, an
/// amplification of roughly 12x reachable with ordinary oversized input. Capping the
/// COUNT bounds that, and real services truncate too: a caller cannot act on 10,000
/// clauses.
///
/// KNOWN DIVERGENCE, recorded rather than papered over. The AWS `ValidationException` envelope opens with
/// `N validation errors detected`, and `N` is the number of violations this walk
/// RECORDED, not the number the body contains — the walk stops here, so the rest are
/// never counted. A body with 1,000 violations is reported as 100. The sentence stays
/// literally true of what was detected, and making it true of the body would mean
/// walking everything, which is exactly the cost this cap exists to avoid. The
/// alternative — a `(truncated)` marker — would itself diverge from AWS's format.
const MAX_VIOLATIONS: usize = 100;

/// Ceiling on the rejected value as rendered INTO a message.
///
/// Separate from the count cap because one violation is enough: a single member
/// holding a multi-megabyte string produces a multi-megabyte clause. The value is
/// echoed so the caller can see what was refused, which a prefix satisfies.
const MAX_ECHOED_VALUE: usize = 256;

/// The rejected value as it should appear in a MESSAGE: itself when short, a
/// character-boundary-safe prefix with an elision when not.
///
/// Rendering only. Every caller must keep the ORIGINAL value for its own check and use
/// this solely to interpolate. Eliding before the check compares text the caller never
/// sent: a legal 300-char lowercase string becomes `xxx... [300 bytes]`, which fails
/// `^[a-z]+$`, so VALID INPUT IS REJECTED. The split is therefore load-bearing rather
/// than stylistic, and `a_long_value_is_pattern_checked_unelided` pins it through
/// `validate`.
fn echoed(value: &str) -> std::borrow::Cow<'_, str> {
    if value.len() <= MAX_ECHOED_VALUE {
        return std::borrow::Cow::Borrowed(value);
    }
    // Truncate on a char boundary, never mid-UTF-8.
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Owned(format!("{}... [{} bytes]", &value[..end], value.len()))
}

/// Process-wide cache of compiled regexes, keyed by pattern source. Patterns
/// come from a fixed model and there are only a handful, so a `Mutex<HashMap>`
/// is more than adequate (validation is not on a hot path for an emulator).
static REGEX_CACHE: LazyLock<Mutex<HashMap<&'static str, regex::Regex>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// True if `value` satisfies the Smithy `@pattern` `regex` (a find, not an
/// anchored match). A pattern that fails to compile is treated as matching
/// (we never reject a value because of an unparseable model pattern).
fn pattern_matches(regex: &'static str, value: &str) -> bool {
    // NOTHING EXPENSIVE HAPPENS UNDER THIS LOCK. It is taken twice, for a map lookup
    // and a map insert, and never held across `Regex::new` (compilation, the costliest
    // step) or `Regex::is_match` (which is milliseconds on a large value). Holding it
    // across either serialises every concurrent request's `@pattern` checks behind one,
    // collapsing concurrent throughput to roughly that of a single thread.
    //
    // `into_inner` on poisoning, never `unwrap`. A panic anywhere while this lock is
    // held would otherwise poison it for the life of the PROCESS, so every later
    // `@pattern` check panics too — and the cache holds compiled regexes, which a panic
    // elsewhere cannot corrupt.
    fn locked() -> std::sync::MutexGuard<'static, HashMap<&'static str, regex::Regex>> {
        REGEX_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // `Regex` is Arc-backed, so cloning it out of the map is cheap.
    //
    // The clone is bound in its OWN statement, and that is load-bearing rather than
    // stylistic. On edition 2021 a temporary in an `if let` scrutinee lives until the end
    // of the whole `if let`, so writing `if let Some(re) = locked().get(..).cloned()`
    // would hold the guard across `is_match` in the body — the exact serialization this
    // function is shaped to avoid, on the common path, while the comment claimed
    // otherwise. A `let` ends the temporary's scope at the semicolon.
    let cached = locked().get(regex).cloned();
    if let Some(re) = cached {
        return re.is_match(value);
    }

    // TODO: a pattern that fails to compile is treated as matching, so we
    // never reject a value because of an unparseable model pattern. A model
    // pattern that does not compile is a model bug; surfacing it as an error
    // (rather than silently passing) would catch that earlier. Out of scope
    // for the prototype — revisit once we have a place to report model-level
    // errors instead of per-value violations.
    let Ok(re) = regex::Regex::new(regex) else {
        return true;
    };

    // A concurrent miss on the same pattern compiles it twice and one insert wins.
    // That is deliberate: both compilations produce an equivalent `Regex` from the same
    // `&'static str`, so the duplicated work is bounded by the number of distinct model
    // patterns and is strictly cheaper than compiling under the lock.
    locked().entry(regex).or_insert_with(|| re.clone());
    re.is_match(value)
}

/// The runtime representation of the older Smithy `@enum` *string-constraint*
/// trait (`smithy.api#enum`), which constrains a plain `string` member to a
/// fixed value set without promoting it to a typed enum shape.
///
/// This is distinct from a Smithy `enum` *shape*, whose membership codegen
/// already enforces at deserialization time. `EnumTrait` is only emitted for
/// `string` members carrying `smithy.api#enum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumTrait {
    /// The permitted values, in declaration order (used verbatim in the
    /// message's value-set rendering).
    pub values: &'static [&'static str],
    /// Whether a value matches one of `values` without regard to ASCII case.
    pub ignore_case: bool,
}

impl EnumTrait {
    /// The Smithy shape id of the `@enum` trait (`smithy.api#enum`).
    pub const TRAIT_ID: ShapeId = ShapeId::from_static("smithy.api#enum", "smithy.api", "enum");

    /// Construct an `@enum` trait from its permitted value set.
    pub const fn new(values: &'static [&'static str]) -> Self {
        Self {
            values,
            ignore_case: false,
        }
    }

    /// Construct an `@enum` trait whose values match without regard to ASCII case.
    ///
    /// For a header the service serves in the case its clients send, such as
    /// `x-amz-checksum-mode: enabled` against the modeled `ENABLED`. A value outside the
    /// set is still refused.
    pub const fn new_ignoring_case(values: &'static [&'static str]) -> Self {
        Self {
            values,
            ignore_case: true,
        }
    }

    fn check(&self, value: &str, path: &str) -> Option<Violation> {
        let permitted = if self.ignore_case {
            self.values.iter().any(|v| v.eq_ignore_ascii_case(value))
        } else {
            self.values.contains(&value)
        };
        if permitted {
            return None;
        }
        Some(Violation {
            path: path.to_string(),
            kind: ViolationKind::EnumValue(self.values.iter().map(|v| v.to_string()).collect()),
            value: Some(echoed(value).into_owned()),
            sensitive: false,
        })
    }
}

impl Trait for EnumTrait {
    fn trait_id(&self) -> &ShapeId {
        &Self::TRAIT_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The runtime representation of the Smithy `@uniqueItems` trait.
///
/// Applies to list members: every element must be distinct. The walk collects
/// each element's rendered form as it is visited and checks for duplicates
/// after the list is fully traversed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UniqueItemsTrait;

impl UniqueItemsTrait {
    /// The Smithy shape id of the `@uniqueItems` trait
    /// (`smithy.api#uniqueItems`).
    pub const TRAIT_ID: ShapeId =
        ShapeId::from_static("smithy.api#uniqueItems", "smithy.api", "uniqueItems");

    /// Construct a `@uniqueItems` trait.
    pub const fn new() -> Self {
        Self
    }
}

impl Default for UniqueItemsTrait {
    fn default() -> Self {
        Self::new()
    }
}

impl Trait for UniqueItemsTrait {
    fn trait_id(&self) -> &ShapeId {
        &Self::TRAIT_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Constraints that belong to a collection's **scalar element/value shape**,
/// carried on the *collection member* schema.
///
/// A scalar list element (`list<TableName>` where `TableName` is
/// `@length`-constrained) and a scalar map value share the *collection member
/// schema* at serialization time — codegen has no separate element/value member
/// schema to hang the element's own `@length`/`@pattern`/… on. So the
/// collection member's [`TraitMap`] would otherwise carry only collection-level
/// traits (`@length` = element count, `@uniqueItems`), and an element's own
/// constraints would be invisible to the walk.
///
/// This wrapper closes that gap: codegen builds a second [`TraitMap`] holding
/// the element/value shape's own constraint traits and attaches it to the
/// collection member via an `ElementConstraintsTrait`. The walk, on a scalar
/// collection element/value, looks this up and runs the inner traits at the
/// element's path (`<list>.<n>.member` / `<map>.<key>`). It is **additive** —
/// no codec reads it, so serialization is unaffected. Map *keys* are not
/// covered (codegen does not capture key constraints yet).
///
/// Its contents are the **scalar leaf's** constraints, at any nesting depth:
/// `list<list<ConfigArn>>` carries `ConfigArn`'s. Intermediate collections
/// contribute nothing, because the walk reads this wrapper only on a scalar leaf
/// and an intermediate collection's `@length` bounds an element *count*. The
/// single descent in `extract_element_constraints` guarantees that.
///
/// This is a framework-internal trait, not a Smithy trait; its shape id lives in
/// a private `smithy.rust.synthetic` namespace so it can never collide with a real
/// Smithy trait id in the same map.
#[derive(Debug, Clone, Copy)]
pub struct ElementConstraintsTrait {
    inner: &'static LazyLock<TraitMap>,
}

impl ElementConstraintsTrait {
    /// Synthetic shape id for the wrapper (not a real Smithy trait).
    pub const TRAIT_ID: ShapeId = ShapeId::from_static(
        "smithy.rust.synthetic#elementConstraints",
        "smithy.rust.synthetic",
        "elementConstraints",
    );

    /// Wrap the element/value shape's own constraint trait map.
    pub const fn new(inner: &'static LazyLock<TraitMap>) -> Self {
        Self { inner }
    }

    /// The element/value shape's own constraint traits.
    pub fn inner(&self) -> &TraitMap {
        self.inner
    }
}

impl Trait for ElementConstraintsTrait {
    fn trait_id(&self) -> &ShapeId {
        &Self::TRAIT_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// =============================================================================
// Constrained — compile-time link between an enforcement type and its trait
// =============================================================================

/// Compile-time link between an enforcement type and the Smithy trait it
/// enforces.
///
/// Implementing `Constrained` ties a Rust type to a concrete constraint
/// [`Trait`] via the `SmithyTrait` associated type, so a caller-defined
/// enforcement shape is checked against the *right* Smithy trait at compile
/// time rather than by stringly-typed lookup. The framework proves the path
/// with a single example (see [`ExampleBoundedString`]). It is a proof of the
/// type-level link only: the generated validating walk reads its bounds from the
/// model, never from a `Constrained` impl.
pub trait Constrained {
    /// The Smithy constraint trait this type enforces.
    type SmithyTrait: Trait;

    /// The constraint instance describing the bounds enforced by this type.
    fn smithy_trait() -> Self::SmithyTrait;
}

/// Example [`Constrained`] type, deliberately carrying ARBITRARY bounds: it exists
/// only to prove the compile-time `Constrained` -> [`LengthTrait`] link, and is not
/// used by generated code.
///
/// The bounds here must stay service-neutral. Spelling out a real service's
/// `@length` rule would read as if the framework enforced that rule, while the
/// generated validating walk takes its bounds exclusively from the member's target
/// shape in the model and never consults a `Constrained` impl — so an implementer
/// who trusted the example would conclude a rule was handled when nothing enforces
/// it anywhere.
#[derive(Debug, Clone)]
pub struct ExampleBoundedString(pub String);

impl Constrained for ExampleBoundedString {
    type SmithyTrait = LengthTrait;

    fn smithy_trait() -> LengthTrait {
        LengthTrait::new(Some(1), Some(64))
    }
}

// =============================================================================
// The validating walk
// =============================================================================

/// Whether the walk is stepping THROUGH a collection child to reach something
/// inside it, or the child itself is what failed.
///
/// A list element names its position either way. A map entry does not: a service
/// names the entry key when a member inside the entry failed, and reports at the
/// map member itself when the entry's own value failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollectionChild {
    /// Stepping into the child to reach a member of it.
    Descending,
    /// The child itself carries the violation.
    Terminal,
}

/// A frame on the collection stack, mirroring the JSON serializer's
/// list/map framing so a path segment can be derived for each element/entry.
///
/// The generated `serialize_members` drives a list as a sequence of element
/// writes against the list member schema, and a map as alternating
/// `write_string(key)` / value writes against the map member schema (exactly
/// like the JSON codec — see `json.rs`'s `Frame::Map { expecting_key }`). This
/// frame reconstructs the element index (lists) or the entry key (maps) from
/// that flat write stream.
#[derive(Debug)]
enum CollectionFrame {
    /// A list: each element write pushes a `<1-based-index>.member` segment.
    List {
        /// Number of elements visited so far (drives the 1-based index).
        index: usize,
        /// When the list carries `@uniqueItems`, the rendered form of each
        /// element for the post-traversal duplicate check; `None` otherwise.
        seen: Option<Vec<String>>,
    },
    /// A map: entries alternate key / value. The key write is captured (not
    /// validated as a leaf) and becomes the path segment for the value write.
    Map {
        /// Whether the next write is the entry key (`true`) or value.
        expecting_key: bool,
        /// The most recently captured entry key, used as the value's segment.
        pending_key: Option<String>,
        /// Number of entries visited so far (drives collection `@length`).
        entries: usize,
    },
    /// A shield pushed while inside a struct's `serialize_members`. Its members
    /// report under their own names, so the struct's leaves must NOT be treated
    /// as elements of an enclosing list/map. This frame makes the enclosing
    /// collection invisible to those inner writes.
    Struct,
}

/// A `ShapeSerializer` that walks a built value and accumulates constraint
/// [`Violation`]s instead of producing output.
///
/// The walk maintains a `path` stack of dotted segments. On `write_struct` it
/// recurses through the struct's `serialize_members` (each member write carries
/// its own member schema, whose `member_name` becomes the path segment). On
/// `write_list`/`write_map` it pushes a [`CollectionFrame`] so each element
/// contributes the right index/key segment. On each leaf it runs the
/// constraint traits found on the passed `&Schema`.
///
/// Violations are *pushed*, never returned as `Err`, so a single walk sees the
/// whole tree. Set [`fail_fast`](Self::set_fail_fast) to stop after the first.
#[derive(Debug, Default)]
pub struct ValidatingSerializer {
    violations: Vec<Violation>,
    /// Dotted path segments from the root to the value currently being written.
    path: Vec<String>,
    /// Active collection frames (innermost last). A leaf inside a list/map uses
    /// the top frame to know its index/key segment.
    collections: Vec<CollectionFrame>,
    /// When true, stop accumulating after the first violation.
    fail_fast: bool,
}

impl ValidatingSerializer {
    /// Create an accumulating validator (sees the whole tree).
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable fail-fast mode: stop after the first violation.
    pub fn set_fail_fast(&mut self, fail_fast: bool) {
        self.fail_fast = fail_fast;
    }

    /// Consume the validator, returning the accumulated violations.
    pub fn into_violations(self) -> Vec<Violation> {
        self.violations
    }

    /// True once a violation has been recorded and fail-fast is enabled.
    fn done(&self) -> bool {
        (self.fail_fast && !self.violations.is_empty()) || self.violations.len() >= MAX_VIOLATIONS
    }

    /// The lowerCamel path segment for `schema`, if it is a member schema.
    ///
    /// Member schemas carry the Smithy member name (PascalCase, e.g.
    /// `TableName`); AWS validation paths are lowerCamel (`tableName`).
    fn segment_for(schema: &Schema) -> Option<String> {
        schema.member_name().map(to_lower_camel)
    }

    /// The current dotted path joined with `.`.
    fn current_path(&self) -> String {
        self.path.join(".")
    }

    fn push_violation(&mut self, v: Violation) {
        self.violations.push(v);
    }

    /// Run every recognised constraint trait on `schema` against a leaf value
    /// of length `len` whose display rendering is `value`. `len` is `Some` for
    /// length-bearing leaves (string/blob), `None` otherwise; `numeric` is
    /// `Some` for numeric leaves (drives `@range`).
    ///
    /// `is_collection_element` is true when this leaf is a *scalar* element of a
    /// list, or a *scalar* value of a map. In that case codegen passes the
    /// **collection member schema** as the element's schema (it has no separate
    /// element schema for scalars), so the trait map it carries describes the
    /// *collection* (`@length` = element/entry count, `@uniqueItems`), not the
    /// element. Those collection-level traits were already enforced in
    /// `walk_list` / `walk_map`, so we must not re-apply them per element — that
    /// would mistake a "max 10 elements" rule for a "max 10 characters" rule.
    fn check_leaf(
        &mut self,
        schema: &Schema,
        value: &str,
        len: Option<usize>,
        numeric: Option<f64>,
        is_collection_element: bool,
    ) {
        if self.done() {
            return;
        }
        // Record this leaf's rendering for an enclosing `@uniqueItems` list.
        if let Some(CollectionFrame::List {
            seen: Some(seen), ..
        }) = self.collections.last_mut()
        {
            seen.push(value.to_string());
        }
        let path = self.current_path();
        if is_collection_element {
            // A scalar collection element/value shares the collection member's
            // schema, so the schema's *own* trait map describes the *collection*
            // (`@length` = element count, `@uniqueItems`) — already enforced in
            // `walk_list`/`walk_map`. The element/value shape's own constraints
            // live in the `ElementConstraintsTrait` wrapper; run those instead.
            if let Some(inner) = element_constraints(schema) {
                let from = self.violations.len();
                self.run_leaf_traits(inner, value, len, numeric, &path);
                self.redact_since(from, holds_sensitive_values(schema));
            }
            return;
        }
        let Some(traits) = schema.traits() else {
            return;
        };
        let from = self.violations.len();
        self.run_leaf_traits(traits, value, len, numeric, &path);
        self.redact_since(from, is_sensitive(traits));
    }

    /// Hides the value of every violation recorded since `from` when the member is `@sensitive`.
    fn redact_since(&mut self, from: usize, sensitive: bool) {
        if !sensitive {
            return;
        }
        for violation in &mut self.violations[from..] {
            violation.value = None;
            violation.sensitive = true;
        }
    }

    /// Run the length/range/pattern/enum constraints found in `traits` against a
    /// single leaf rendered as `value` at `path`. `len` is the character/byte
    /// count for string/blob leaves; `numeric` is the widened value for numeric
    /// leaves. Honors fail-fast between checks.
    fn run_leaf_traits(
        &mut self,
        traits: &TraitMap,
        value: &str,
        len: Option<usize>,
        numeric: Option<f64>,
        path: &str,
    ) {
        if let Some(len) = len {
            if let Some(v) = check_length(traits, len, value, path) {
                self.push_violation(v);
                if self.done() {
                    return;
                }
            }
        }
        if let Some(n) = numeric {
            if let Some(v) = check_range(traits, n, value, path) {
                self.push_violation(v);
                if self.done() {
                    return;
                }
            }
        }
        // String constraints (pattern, enum value set) apply to string leaves;
        // `len` is the character count for strings.
        if len.is_some() && numeric.is_none() {
            if let Some(v) = check_pattern(traits, value, path) {
                self.push_violation(v);
                if self.done() {
                    return;
                }
            }
            if let Some(v) = check_enum(traits, value, path) {
                self.push_violation(v);
            }
        }
    }

    /// Push a member-name segment, run `body`, then pop. Members that are not
    /// member schemas (e.g. the root input struct) leave the path unchanged.
    fn with_segment<R>(&mut self, schema: &Schema, body: impl FnOnce(&mut Self) -> R) -> R {
        match Self::segment_for(schema) {
            Some(seg) => {
                self.path.push(seg);
                let out = body(self);
                self.path.pop();
                out
            }
            None => body(self),
        }
    }

    /// Check collection-level `@length` (element/entry count) on the list/map
    /// member `schema`, whose collection contains `count` items. The rendered
    /// `value` is the collection's display form.
    fn check_collection_length(&mut self, schema: &Schema, count: usize, value: &str) {
        if self.done() {
            return;
        }
        let Some(traits) = schema.traits() else {
            return;
        };
        let path = self.current_path();
        if let Some(v) = check_length(traits, count, value, &path) {
            self.push_violation(v);
        }
    }
}

/// Look up the `@length` trait on a trait map and check it against `len`.
fn check_length(traits: &TraitMap, len: usize, value: &str, path: &str) -> Option<Violation> {
    let t = traits.get(&LengthTrait::TRAIT_ID)?;
    let length = t.as_any().downcast_ref::<LengthTrait>()?;
    length.check(len, value, path)
}

/// Look up the `@range` trait on a trait map and check it against `n`.
fn check_range(traits: &TraitMap, n: f64, value: &str, path: &str) -> Option<Violation> {
    let t = traits.get(&RangeTrait::TRAIT_ID)?;
    let range = t.as_any().downcast_ref::<RangeTrait>()?;
    range.check(n, value, path)
}

/// Look up the `@pattern` trait on a trait map and check it against `value`.
fn check_pattern(traits: &TraitMap, value: &str, path: &str) -> Option<Violation> {
    let t = traits.get(&PatternTrait::TRAIT_ID)?;
    let pattern = t.as_any().downcast_ref::<PatternTrait>()?;
    pattern.check(value, path)
}

/// Look up the `@enum` trait on a trait map and check it against `value`.
fn check_enum(traits: &TraitMap, value: &str, path: &str) -> Option<Violation> {
    let t = traits.get(&EnumTrait::TRAIT_ID)?;
    let en = t.as_any().downcast_ref::<EnumTrait>()?;
    en.check(value, path)
}

/// The element/value shape's own constraint trait map, if the collection member
/// `schema` carries an [`ElementConstraintsTrait`] wrapper.
fn element_constraints(schema: &Schema) -> Option<&TraitMap> {
    let t = schema.traits()?.get(&ElementConstraintsTrait::TRAIT_ID)?;
    let wrapper = t.as_any().downcast_ref::<ElementConstraintsTrait>()?;
    Some(wrapper.inner())
}

/// `true` if `traits` marks its member `@sensitive`.
fn is_sensitive(traits: &TraitMap) -> bool {
    traits
        .get(&aws_smithy_schema::traits::SensitiveTrait::TRAIT_ID)
        .is_some()
}

/// `true` if a collection member or the shape of its elements is `@sensitive`, so no element's
/// value may be shown.
fn holds_sensitive_values(schema: &Schema) -> bool {
    let element = element_constraints(schema);
    schema.traits().is_some_and(is_sensitive) || element.is_some_and(is_sensitive)
}

/// `true` if the schema's trait map carries `@uniqueItems`.
fn has_unique_items(schema: &Schema) -> bool {
    schema
        .traits()
        .and_then(|t| t.get(&UniqueItemsTrait::TRAIT_ID))
        .is_some()
}

impl ShapeSerializer for ValidatingSerializer {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        if self.done() {
            return Ok(());
        }
        // A struct can appear as: the root input (no member name, no segment); a
        // nested struct member (member name → segment); a list element (struct's
        // own SCHEMA, no member name → element frame supplies `<n>.member`); or a
        // map value (key frame supplies the key segment).
        let element_seg = self.element_segment();
        match element_seg {
            Some(seg) => {
                self.path.push(seg);
                self.collections.push(CollectionFrame::Struct);
                let out = value.serialize_members(self);
                self.collections.pop();
                self.path.pop();
                self.end_collection_child();
                out
            }
            None => self.with_segment(schema, |this| {
                this.collections.push(CollectionFrame::Struct);
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
        if self.done() {
            return Ok(());
        }
        self.in_collection_child(schema, |this, sch, elem| {
            this.walk_list(sch, elem, write_elements)
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.done() {
            return Ok(());
        }
        self.in_collection_child(schema, |this, sch, elem| {
            this.walk_map(sch, elem, write_entries)
        })
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.write_leaf(schema, &value.to_string(), None, None);
        Ok(())
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.write_leaf(schema, &value.to_string(), None, Some(value as f64));
        Ok(())
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.write_leaf(schema, &value.to_string(), None, Some(value as f64));
        Ok(())
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.write_leaf(schema, &value.to_string(), None, Some(value as f64));
        Ok(())
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.write_leaf(schema, &value.to_string(), None, Some(value as f64));
        Ok(())
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.write_leaf(schema, &value.to_string(), None, Some(value as f64));
        Ok(())
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.write_leaf(schema, &value.to_string(), None, Some(value));
        Ok(())
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.write_leaf(schema, value.as_ref(), None, None);
        Ok(())
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.write_leaf(schema, value.as_ref(), None, None);
        Ok(())
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        // `@length` on a string counts characters, not bytes (`chars().count()`).
        let len = value.chars().count();
        self.write_leaf(schema, value, Some(len), None);
        Ok(())
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        let len = value.as_ref().len();
        self.write_leaf(schema, &format!("{len} bytes"), Some(len), None);
        Ok(())
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.write_leaf(schema, &value.to_string(), None, None);
        Ok(())
    }

    fn write_document(&mut self, schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        self.write_leaf(schema, "<document>", None, None);
        Ok(())
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        Ok(())
    }
}

impl ValidatingSerializer {
    /// Common leaf entry. A leaf write can be: a struct-member scalar (member
    /// name → segment); a map *key* (captured, not validated, toggles the map
    /// frame); a map *value* (entry-key segment); or a list element
    /// (`<n>.member` segment). After a non-key write inside a collection the
    /// frame is advanced.
    fn write_leaf(
        &mut self,
        schema: &Schema,
        value: &str,
        len: Option<usize>,
        numeric: Option<f64>,
    ) {
        if self.done() {
            return;
        }
        // Map key: the codec writes the entry key as a string against the map
        // member schema before the value. Capture it as the value's segment and
        // toggle the frame; the key itself is not a constrained leaf.
        if let Some(CollectionFrame::Map {
            expecting_key,
            pending_key,
            entries,
        }) = self.collections.last_mut()
        {
            if *expecting_key {
                *pending_key = Some(value.to_string());
                *expecting_key = false;
                *entries += 1;
                return;
            }
        }

        match self.begin_collection_child(CollectionChild::Terminal) {
            Some(seg) => {
                // A scalar list element / map value: it carries the collection
                // member schema, whose traits are collection-level. Suppress
                // per-element checks (already enforced on the collection).
                self.path.push(seg);
                self.check_leaf(schema, value, len, numeric, true);
                self.path.pop();
                self.end_collection_child();
            }
            None => {
                // Not directly inside a collection: a struct-member scalar
                // reports under its own lowerCamel name and is fully checked.
                self.with_segment(schema, |this| {
                    this.check_leaf(schema, value, len, numeric, false)
                });
            }
        }
    }

    /// The path segment for a struct/value child written directly inside the
    /// innermost collection frame, advancing list state. Returns `None` when
    /// not directly inside a collection (so the caller falls back to the
    /// member-name segment). Map-frame toggling for the *value* happens in
    /// [`end_collection_child`].
    fn begin_collection_child(&mut self, child: CollectionChild) -> Option<String> {
        match self.collections.last_mut()? {
            CollectionFrame::List { index, .. } => {
                *index += 1;
                Some(path::element(*index))
            }
            CollectionFrame::Map { pending_key, .. } => {
                let key = pending_key.clone().unwrap_or_default();
                match child {
                    // Descending INTO the entry's value to reach a member of it:
                    // the step into the collection is named, as it is for a list.
                    CollectionChild::Descending => Some(path::entry(&key)),
                    // The entry's value IS what failed. A service reports this at
                    // the map member with a bundled requirement and no key at all;
                    // we report the key instead. See
                    // `map_value_pattern_reports_key_path`.
                    CollectionChild::Terminal => Some(key),
                }
            }
            // Inside a struct: members report under their own names, so this
            // write is not a collection element.
            CollectionFrame::Struct => None,
        }
    }

    /// After a value/struct child completes inside a map frame, flip back to
    /// expecting the next entry's key. (Lists need no per-child bookkeeping
    /// beyond the index already bumped in [`begin_collection_child`].)
    fn end_collection_child(&mut self) {
        if let Some(CollectionFrame::Map { expecting_key, .. }) = self.collections.last_mut() {
            *expecting_key = true;
        }
    }

    /// The path segment to push for a struct child of the innermost collection
    /// frame (list element or map value). `None` if not directly in a
    /// collection. Used by [`write_struct`].
    fn element_segment(&mut self) -> Option<String> {
        self.begin_collection_child(CollectionChild::Descending)
    }

    /// Run `body` for a collection (list/map) write, honoring the
    /// collection-child protocol when this collection is itself an element of a
    /// parent collection: push the parent's element segment (and toggle the
    /// parent map frame) so paths and key/value alternation stay correct for
    /// arbitrarily nested containers. When not directly inside a collection,
    /// `body` runs unchanged (it will apply its own member-name segment).
    ///
    /// `body`'s `bool` argument is `true` when this collection *is* a parent
    /// collection's element. See [`walk_list`](Self::walk_list) for why that
    /// changes which traits apply.
    fn in_collection_child(
        &mut self,
        schema: &Schema,
        body: impl FnOnce(&mut Self, &Schema, bool) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        match self.begin_collection_child(CollectionChild::Descending) {
            Some(seg) => {
                self.path.push(seg);
                let out = body(self, schema, true);
                self.path.pop();
                self.end_collection_child();
                out
            }
            None => body(self, schema, false),
        }
    }

    /// The list walk: push a member-name segment (for a struct-member list),
    /// open a list frame, recurse, then check collection `@length` /
    /// `@uniqueItems`.
    ///
    /// `is_collection_element` is true when this list is itself an element of an
    /// enclosing list or map. Codegen has no separate schema for a collection
    /// element, so a nested list is written against the *enclosing collection
    /// member's* schema — the same schema the enclosing level was already
    /// checked with. Its `@length` is that level's element count and its
    /// `@uniqueItems` that level's uniqueness rule, so re-running either here
    /// would impose the outer bound on the inner list — a member bounding how many
    /// LISTS it holds would reject every inner list holding a different number of
    /// elements. Its member name is likewise already on the path, contributed by
    /// the enclosing level.
    ///
    /// A nested list's *own* `@length` therefore goes unenforced: enforcing it
    /// needs a per-level element schema to carry it, which codegen does not emit.
    /// That is the safe direction — the bound is checked one level up or not at
    /// all, rather than against the wrong value — but it is not free. Where a
    /// member's bound happens to equal its element's, the enclosing check was
    /// landing on the right number by coincidence and no longer runs.
    ///
    /// `@uniqueItems` is unenforced for a nested list either way, at both levels:
    /// [`check_leaf`](Self::check_leaf) records element renderings only into the
    /// INNERMOST frame, so an outer frame whose elements are themselves
    /// collections never sees a value to compare.
    fn walk_list(
        &mut self,
        schema: &Schema,
        is_collection_element: bool,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.with_collection_segment(schema, is_collection_element, |this| {
            let list_path = this.current_path();
            this.collections.push(CollectionFrame::List {
                index: 0,
                seen: (!is_collection_element && has_unique_items(schema)).then(Vec::new),
            });
            let result = write_elements(this);
            let frame = this.collections.pop().expect("list frame present");
            let CollectionFrame::List { index, seen, .. } = frame else {
                unreachable!("pushed a List frame");
            };
            if !is_collection_element {
                this.check_collection_length(schema, index, &format!("{index} items"));
            }
            if let Some(seen) = seen {
                if let Some(dup) = first_duplicate(&seen) {
                    if !this.done() {
                        let from = this.violations.len();
                        this.push_violation(Violation {
                            path: list_path.clone(),
                            kind: ViolationKind::UniqueItems,
                            // Elided like every other stored value: a duplicate is
                            // caller-supplied and arbitrarily long, and the clause
                            // interpolates it as-is.
                            value: Some(echoed(&dup).into_owned()),
                            sensitive: false,
                        });
                        this.redact_since(from, holds_sensitive_values(schema));
                    }
                }
            }
            result
        })
    }

    /// The map walk: push a member-name segment, open a map frame (alternating
    /// key/value), recurse, then check collection `@length`. `is_collection_element`
    /// carries the same meaning as in [`walk_list`](Self::walk_list).
    fn walk_map(
        &mut self,
        schema: &Schema,
        is_collection_element: bool,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.with_collection_segment(schema, is_collection_element, |this| {
            this.collections.push(CollectionFrame::Map {
                expecting_key: true,
                pending_key: None,
                entries: 0,
            });
            let result = write_entries(this);
            let frame = this.collections.pop().expect("map frame present");
            let CollectionFrame::Map { entries, .. } = frame else {
                unreachable!("pushed a Map frame");
            };
            if !is_collection_element {
                this.check_collection_length(schema, entries, &format!("{entries} entries"));
            }
            result
        })
    }

    /// [`with_segment`](Self::with_segment), except that a collection which is
    /// itself a collection element contributes no segment: it shares the
    /// enclosing member's schema, whose name the enclosing level already pushed.
    fn with_collection_segment<R>(
        &mut self,
        schema: &Schema,
        is_collection_element: bool,
        body: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if is_collection_element {
            body(self)
        } else {
            self.with_segment(schema, body)
        }
    }
}

/// The first value that appears more than once in `items`, if any. Preserves
/// the rendering used for the violation message.
fn first_duplicate(items: &[String]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if !seen.insert(item.as_str()) {
            return Some(item.clone());
        }
    }
    None
}

/// Lowercase the first character of a Smithy member name to produce the
/// lowerCamel validation path segment (`TableName` -> `tableName`).
pub(crate) fn to_lower_camel(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Render accumulated violations into the AWS `ValidationException` envelope.
/// `1 validation error detected: <clause>` / `N validation errors detected:
/// <clause>; <clause>`.
pub fn envelope(violations: &[Violation]) -> Option<String> {
    match violations.len() {
        0 => None,
        1 => Some(format!(
            "1 validation error detected: {}",
            violations[0].message()
        )),
        n => {
            let clauses: Vec<String> = violations.iter().map(Violation::message).collect();
            Some(format!(
                "{n} validation errors detected: {}",
                clauses.join("; ")
            ))
        }
    }
}

/// Validate a built input value against its schema, returning every constraint
/// [`Violation`] found (empty when the value is valid).
///
/// This is the public entry point: it runs the accumulating
/// [`ValidatingSerializer`] walk over `value.serialize_members`.
pub fn validate(value: &dyn SerializableStruct, schema: &Schema) -> Vec<Violation> {
    let mut serializer = ValidatingSerializer::new();
    // The root input is a struct; drive it the same way a codec would.
    let _ = serializer.write_struct(schema, value);
    serializer.into_violations()
}

/// Validate `value` and, if any violations exist, render them into the AWS
/// `ValidationException` envelope string. Returns `None` when valid.
pub fn validate_to_message(value: &dyn SerializableStruct, schema: &Schema) -> Option<String> {
    envelope(&validate(value, schema))
}

/// Render an absent required member as the same envelope a constraint violation
/// gets, so both rejection classes read alike.
///
/// `member_path` is the dot-separated trail of Smithy member names from the
/// operation input inwards, outermost first; each segment is lowerCamel-ed here
/// because that casing is a convention of the envelope, not a fact of the model.
pub fn missing_member_message(member_path: &str) -> String {
    // `member_path` arrives fully formed. Casing is decided where each segment is
    // ADDED, because only there is it known whether the segment is a model name
    // (lowerCamel) or a value the client chose, such as a map key, which is
    // echoed exactly as sent.
    let path = member_path.to_string();
    let violation = Violation {
        path,
        kind: ViolationKind::MissingRequiredMember,
        value: None,
        sensitive: false,
    };
    // One absent member is one violation, so the envelope always renders.
    envelope(&[violation]).unwrap_or_default()
}

#[cfg(test)]
mod tests {

    /// Build a path the way the deserializer does, then render it — so these
    /// cover the seam rather than the renderer in isolation.
    fn rendered(error: ::aws_smithy_schema::serde::SerdeError) -> String {
        match error {
            ::aws_smithy_schema::serde::SerdeError::MissingMember { member_name } => {
                missing_member_message(&member_name)
            }
            other => panic!("expected a missing member, got {other}"),
        }
    }

    /// A member name is a MODEL name, so it is lowerCamel-ed where it is added.
    #[test]
    fn a_missing_member_renders_like_a_constraint_violation() {
        assert_eq!(
            rendered(crate::missing_member("TableName")),
            "1 validation error detected: Value null at 'tableName' \
             failed to satisfy constraint: Member must not be null"
        );
    }

    /// Each enclosing member contributes its own lowerCamel segment.
    #[test]
    fn every_segment_of_a_nested_path_is_lowered() {
        let error = crate::within_member(
            crate::missing_member("WriteCapacityUnits"),
            "ProvisionedThroughput",
        );
        assert_eq!(
            rendered(error),
            "1 validation error detected: Value null at \
             'provisionedThroughput.writeCapacityUnits' failed to satisfy constraint: \
             Member must not be null"
        );
    }

    /// Only the FIRST character drops case: a name opening on an acronym keeps
    /// the rest, which is what a service reports.
    #[test]
    fn an_acronym_member_lowers_only_its_first_character() {
        for (smithy, want) in [("APIName", "aPIName"), ("SAMAccountName", "sAMAccountName")] {
            let got = rendered(crate::missing_member(smithy));
            assert_eq!(got.split('\'').nth(1), Some(want), "{got}");
        }
    }

    /// A list element is named by its 1-BASED position, then the literal
    /// `member` — the same vocabulary the constraint walk uses.
    #[test]
    fn a_list_element_is_named_by_its_position() {
        let error = crate::within_member(
            crate::within_list_element(crate::missing_member("ResourceKey"), 2),
            "ReservationList",
        );
        assert!(
            rendered(error).contains("'reservationList.2.member.resourceKey'"),
            "{}",
            rendered(crate::within_member(
                crate::within_list_element(crate::missing_member("ResourceKey"), 2),
                "ReservationList"
            ))
        );
    }

    /// A map key is a value the CLIENT chose, so it is echoed verbatim while the
    /// model names around it are lowered. Lowering it would rename the caller's
    /// own resource back at them.
    #[test]
    fn a_map_key_is_echoed_exactly_as_the_client_sent_it() {
        let error = crate::within_member(
            crate::within_map_entry(crate::missing_member("Keys"), "MyMixedCaseTable_ABC"),
            "RequestItems",
        );
        let got = rendered(error);
        assert!(
            got.contains("'requestItems.MyMixedCaseTable_ABC.member.keys'"),
            "{got}"
        );
    }
    use super::*;
    use std::sync::LazyLock;

    // ---- @length on a string member ---------------------------------------

    static TABLE_NAME_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(LengthTrait::new(Some(3), Some(255))));
        m.insert(Box::new(PatternTrait::new("^[a-zA-Z0-9_.-]+$")));
        m
    });

    static M_TABLE_NAME: Schema = Schema::new_member(
        ShapeId::from_static("test#FakeInput$TableName", "test", "TableName"),
        aws_smithy_schema::ShapeType::String,
        "TableName",
        0,
    )
    .with_traits(&TABLE_NAME_TRAITS);

    static INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#FakeInput", "test", "FakeInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_TABLE_NAME],
    );

    struct FakeInput {
        table_name: String,
    }

    impl SerializableStruct for FakeInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_string(&M_TABLE_NAME, &self.table_name)?;
            Ok(())
        }
    }

    /// A value longer than the echo cap must be CHECKED unelided.
    ///
    /// Regression: the elision was applied by shadowing `value` at the top of each
    /// `check`, so `pattern_matches` and the enum `contains` saw the truncated text —
    /// which contains `.`, a space, `[` and digits the caller never sent. A long
    /// value legal under its own `@pattern` was then reported as violating it.
    ///
    /// Driven through `validate`, not through `pattern_matches`, because the defect
    /// was at the CALL SITE: a test against the helper passes either way.
    #[test]
    fn a_long_value_is_pattern_checked_unelided() {
        // 300 chars, all legal under `^[a-zA-Z0-9_.-]+$`, but over @length(max 255)
        // and over MAX_ECHOED_VALUE — so exactly ONE violation is correct.
        let input = FakeInput {
            table_name: "a".repeat(300),
        };
        assert!(input.table_name.len() > MAX_ECHOED_VALUE);
        let violations = validate(&input, &INPUT_SCHEMA);
        assert!(
            violations
                .iter()
                .all(|v| !v.message().contains("regular expression pattern")),
            "the value satisfies its @pattern; judging the ELIDED form reports a \
             violation the caller never caused: {violations:?}"
        );
        assert_eq!(
            violations.len(),
            1,
            "only the length bound is violated: {violations:?}"
        );
    }

    /// A rejected value is ECHOED into the message, so one huge member must not
    /// produce a huge message. Bounded by `MAX_ECHOED_VALUE`, on a char boundary.
    #[test]
    fn a_huge_rejected_value_is_elided_in_the_message() {
        let input = FakeInput {
            table_name: "x".repeat(1_000_000),
        };
        let violations = validate(&input, &INPUT_SCHEMA);
        assert!(
            !violations.is_empty(),
            "a 1 MB value violates @length(max 255)"
        );
        // EVERY clause is bounded, not just the first: a too-long value trips the
        // length bound and can trip others on the same member, and each one echoes
        // the value independently.
        for v in &violations {
            assert!(
                v.message().len() < 1_000,
                "a 1 MB value must not yield a 1 MB clause, got {} bytes at {}",
                v.message().len(),
                v.path
            );
            assert!(
                v.message().contains("1000000 bytes"),
                "the elision must report the real size: {}",
                v.message()
            );
            assert_eq!(v.path, "tableName");
        }
        // Still usable: the violated bound survives the elision.
        assert!(
            violations
                .iter()
                .any(|v| v.message().contains("less than or equal to 255")),
            "the length bound must still be reported"
        );
    }

    /// Multi-byte values must elide on a CHARACTER boundary — slicing mid-UTF-8
    /// would panic, and this runs on caller-supplied bytes.
    #[test]
    fn eliding_a_multibyte_value_does_not_split_a_character() {
        let input = FakeInput {
            // 4-byte chars, so the byte cap lands mid-character unless adjusted.
            table_name: "\u{1F600}".repeat(1_000),
        };
        let violations = validate(&input, &INPUT_SCHEMA);
        assert!(!violations.is_empty());
        for v in &violations {
            assert!(
                v.message().contains("4000 bytes"),
                "the elision must report the real byte size: {}",
                v.message()
            );
        }
    }

    #[test]
    fn too_short_string_reports_min_with_lower_camel_path() {
        let input = FakeInput {
            table_name: "ab".to_string(),
        };
        let violations = validate(&input, &INPUT_SCHEMA);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path, "tableName");
        assert_eq!(
            violations[0].message(),
            "Value 'ab' at 'tableName' failed to satisfy constraint: \
             Member must have length greater than or equal to 3"
        );
    }

    #[test]
    fn too_long_string_reports_max() {
        let input = FakeInput {
            table_name: "x".repeat(256),
        };
        let msg = validate_to_message(&input, &INPUT_SCHEMA).unwrap();
        assert!(
            msg.ends_with("Member must have length less than or equal to 255"),
            "unexpected message: {msg}"
        );
        assert!(msg.starts_with("1 validation error detected: Value '"));
    }

    #[test]
    fn pattern_violation_reports_regex() {
        let input = FakeInput {
            table_name: "has space".to_string(),
        };
        let violations = validate(&input, &INPUT_SCHEMA);
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].message(),
            "Value 'has space' at 'tableName' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"
        );
    }

    #[test]
    fn valid_string_has_no_violations() {
        let input = FakeInput {
            table_name: "valid_table".to_string(),
        };
        assert!(validate(&input, &INPUT_SCHEMA).is_empty());
        assert!(validate_to_message(&input, &INPUT_SCHEMA).is_none());
    }

    #[test]
    fn constrained_links_type_to_length_trait() {
        // The bounds are arbitrary and service-neutral on purpose; what is under
        // test is that the associated type resolves to `LengthTrait` and that the
        // impl's own bounds come back unchanged.
        let t: LengthTrait = ExampleBoundedString::smithy_trait();
        assert_eq!(t, LengthTrait::new(Some(1), Some(64)));
    }

    #[test]
    fn fail_fast_stops_after_first_violation() {
        // `M_TABLE_NAME` carries both `@length(min:3)` and a `@pattern`. The
        // value "a@" is two violations at once: too short (2 chars < 3) and the
        // '@' breaks the pattern. The default accumulating walk sees both;
        // fail-fast stops after the first.
        let input = FakeInput {
            table_name: "a@".to_string(),
        };

        // Default: accumulate every violation.
        let all = validate(&input, &INPUT_SCHEMA);
        assert_eq!(
            all.len(),
            2,
            "expected length + pattern violations: {all:?}"
        );

        // Fail-fast: stop after the first.
        let mut serializer = ValidatingSerializer::new();
        serializer.set_fail_fast(true);
        let _ = serializer.write_struct(&INPUT_SCHEMA, &input);
        let first_only = serializer.into_violations();
        assert_eq!(
            first_only.len(),
            1,
            "fail-fast must stop at one: {first_only:?}"
        );
        // The length check runs before the pattern check, so the single
        // recorded violation is the length one.
        assert_eq!(first_only[0], all[0]);
    }

    // ---- @range on a numeric member ----------------------------------------

    static LIMIT_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(RangeTrait::new(Some(1.0), None)));
        m
    });

    static M_LIMIT: Schema = Schema::new_member(
        ShapeId::from_static("test#RangeInput$Limit", "test", "Limit"),
        aws_smithy_schema::ShapeType::Integer,
        "Limit",
        0,
    )
    .with_traits(&LIMIT_TRAITS);

    static RANGE_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#RangeInput", "test", "RangeInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_LIMIT],
    );

    struct RangeInput {
        limit: i32,
    }

    impl SerializableStruct for RangeInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_integer(&M_LIMIT, self.limit)?;
            Ok(())
        }
    }

    #[test]
    fn range_min_violation_renders_integer_bound() {
        let input = RangeInput { limit: 0 };
        let violations = validate(&input, &RANGE_INPUT_SCHEMA);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path, "limit");
        assert_eq!(
            violations[0].message(),
            "Value '0' at 'limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
        );
    }

    #[test]
    fn range_satisfied_has_no_violation() {
        let input = RangeInput { limit: 5 };
        assert!(validate(&input, &RANGE_INPUT_SCHEMA).is_empty());
    }

    // ---- @enum string constraint -------------------------------------------

    static SELECT_VALUES: &[&str] = &["A", "B", "C"];
    static SELECT_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(EnumTrait::new(SELECT_VALUES)));
        m
    });

    static M_SELECT: Schema = Schema::new_member(
        ShapeId::from_static("test#EnumInput$Select", "test", "Select"),
        aws_smithy_schema::ShapeType::String,
        "Select",
        0,
    )
    .with_traits(&SELECT_TRAITS);

    static ENUM_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#EnumInput", "test", "EnumInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_SELECT],
    );

    struct EnumInput {
        select: String,
    }

    impl SerializableStruct for EnumInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_string(&M_SELECT, &self.select)?;
            Ok(())
        }
    }

    #[test]
    fn enum_value_set_violation() {
        let input = EnumInput {
            select: "D".to_string(),
        };
        let violations = validate(&input, &ENUM_INPUT_SCHEMA);
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].message(),
            "Value 'D' at 'select' failed to satisfy constraint: \
             Member must satisfy enum value set: [A, B, C]"
        );
    }

    #[test]
    fn enum_member_present_passes() {
        let input = EnumInput {
            select: "B".to_string(),
        };
        assert!(validate(&input, &ENUM_INPUT_SCHEMA).is_empty());
    }

    /// An enum built with `new_ignoring_case` accepts a listed value in any ASCII case and
    /// still refuses a value outside the set; one built with `new` matches exactly.
    #[test]
    fn an_ignoring_case_enum_accepts_any_casing_of_a_listed_value_only() {
        static MODES: &[&str] = &["ENABLED"];
        let loose = EnumTrait::new_ignoring_case(MODES);
        for ok in ["ENABLED", "enabled", "Enabled", "eNaBlEd"] {
            assert!(loose.check(ok, "ChecksumMode").is_none(), "{ok} refused");
        }
        for bad in ["", "xyz", "ENABLE", "enabled ", "DISABLED"] {
            assert!(
                loose.check(bad, "ChecksumMode").is_some(),
                "{bad:?} accepted"
            );
        }

        let exact = EnumTrait::new(MODES);
        assert!(exact.check("ENABLED", "ChecksumMode").is_none());
        assert!(exact.check("enabled", "ChecksumMode").is_some());
    }

    // ---- list element path fidelity + collection @length -------------------

    // KeySchemaElement.KeyType carries an @enum value set.
    static KEY_TYPE_VALUES: &[&str] = &["HASH", "RANGE"];
    static KEY_TYPE_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(EnumTrait::new(KEY_TYPE_VALUES)));
        m
    });
    static M_KEY_TYPE: Schema = Schema::new_member(
        ShapeId::from_static("test#KeySchemaElement$KeyType", "test", "KeyType"),
        aws_smithy_schema::ShapeType::String,
        "KeyType",
        0,
    )
    .with_traits(&KEY_TYPE_TRAITS);
    static KEY_SCHEMA_ELEMENT: Schema = Schema::new_struct(
        ShapeId::from_static("test#KeySchemaElement", "test", "KeySchemaElement"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_KEY_TYPE],
    );

    // keySchema list member carries @length(max: 2).
    static KEY_SCHEMA_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(LengthTrait::new(None, Some(2))));
        m
    });
    static M_KEY_SCHEMA: Schema = Schema::new_member(
        ShapeId::from_static("test#CreateInput$KeySchema", "test", "KeySchema"),
        aws_smithy_schema::ShapeType::List,
        "KeySchema",
        0,
    )
    .with_traits(&KEY_SCHEMA_TRAITS);
    static CREATE_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#CreateInput", "test", "CreateInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_KEY_SCHEMA],
    );

    struct KeySchemaElement {
        key_type: String,
    }
    impl SerializableStruct for KeySchemaElement {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_string(&M_KEY_TYPE, &self.key_type)?;
            Ok(())
        }
    }
    struct CreateInput {
        key_schema: Vec<KeySchemaElement>,
    }
    impl SerializableStruct for CreateInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_list(&M_KEY_SCHEMA, &|sl| {
                for el in self.key_schema.iter() {
                    sl.write_struct(&KEY_SCHEMA_ELEMENT, el)?;
                }
                Ok(())
            })?;
            Ok(())
        }
    }

    #[test]
    fn list_element_enum_reports_indexed_member_path() {
        let input = CreateInput {
            key_schema: vec![
                KeySchemaElement {
                    key_type: "HASH".to_string(),
                },
                KeySchemaElement {
                    key_type: "BOGUS".to_string(),
                },
            ],
        };
        let violations = validate(&input, &CREATE_INPUT_SCHEMA);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "keySchema.2.member.keyType");
        assert_eq!(
            violations[0].message(),
            "Value 'BOGUS' at 'keySchema.2.member.keyType' failed to satisfy constraint: \
             Member must satisfy enum value set: [HASH, RANGE]"
        );
    }

    #[test]
    fn list_level_length_reports_collection_path() {
        let input = CreateInput {
            key_schema: vec![
                KeySchemaElement {
                    key_type: "HASH".to_string(),
                },
                KeySchemaElement {
                    key_type: "RANGE".to_string(),
                },
                KeySchemaElement {
                    key_type: "HASH".to_string(),
                },
            ],
        };
        let violations = validate(&input, &CREATE_INPUT_SCHEMA);
        // 3 > max 2: a collection-level length violation on `keySchema`.
        let len_v = violations
            .iter()
            .find(|v| v.path == "keySchema")
            .expect("collection length violation");
        assert!(
            len_v
                .message()
                .contains("Member must have length less than or equal to 2"),
            "{len_v:?}"
        );
    }

    // ---- @uniqueItems -------------------------------------------------------

    static TAGS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(UniqueItemsTrait::new()));
        m
    });
    static M_TAGS: Schema = Schema::new_member(
        ShapeId::from_static("test#UniqInput$Tags", "test", "Tags"),
        aws_smithy_schema::ShapeType::List,
        "Tags",
        0,
    )
    .with_traits(&TAGS_TRAITS);
    // The element schema is a bare (non-member) string schema, like codegen's
    // element write target.
    static TAGS_ELEMENT: Schema = Schema::new_member(
        ShapeId::from_static("test#TagList$member", "test", "member"),
        aws_smithy_schema::ShapeType::String,
        "member",
        0,
    );
    static UNIQ_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#UniqInput", "test", "UniqInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_TAGS],
    );

    struct UniqInput {
        tags: Vec<String>,
    }
    impl SerializableStruct for UniqInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_list(&M_TAGS, &|sl| {
                for t in self.tags.iter() {
                    sl.write_string(&TAGS_ELEMENT, t)?;
                }
                Ok(())
            })?;
            Ok(())
        }
    }

    #[test]
    fn unique_items_duplicate_detected() {
        let input = UniqInput {
            tags: vec!["a".to_string(), "b".to_string(), "a".to_string()],
        };
        let violations = validate(&input, &UNIQ_INPUT_SCHEMA);
        let v = violations
            .iter()
            .find(|v| v.message().contains("Member must have unique values"))
            .expect("uniqueItems violation");
        assert_eq!(v.path, "tags");
    }

    /// A duplicate is caller-supplied, so the clause naming it must be elided like
    /// every other echoed value — otherwise a large repeated element turns a small
    /// request into an arbitrarily long response.
    #[test]
    fn a_long_duplicate_is_elided_in_the_clause() {
        let long = "x".repeat(MAX_ECHOED_VALUE * 4);
        let input = UniqInput {
            tags: vec![long.clone(), long],
        };
        let violations = validate(&input, &UNIQ_INPUT_SCHEMA);
        let v = violations
            .iter()
            .find(|v| v.message().contains("Member must have unique values"))
            .expect("uniqueItems violation");
        let message = v.message();
        assert!(
            message.len() < MAX_ECHOED_VALUE * 2,
            "clause must be capped, got {} chars",
            message.len()
        );
    }

    #[test]
    fn unique_items_all_distinct_passes() {
        let input = UniqInput {
            tags: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        };
        assert!(validate(&input, &UNIQ_INPUT_SCHEMA).is_empty());
    }

    // ---- scalar list-element constraints (ElementConstraintsTrait) ----------
    //
    // Mirrors real codegen: a scalar list element is written against the
    // *collection member schema* (`M_NAMES`), which carries the element shape's
    // own `@length` via an `ElementConstraintsTrait` wrapper. The collection
    // member also carries its own `@length` (max 3 elements) to prove the two
    // do not collide.

    // Element shape (`AttributeName`) constraint: @length(min: 2).
    static NAME_ELEM_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(LengthTrait::new(Some(2), None)));
        m
    });
    // Collection member: @length(max: 3) on element count + element constraints.
    static NAMES_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(LengthTrait::new(None, Some(3))));
        m.insert(Box::new(ElementConstraintsTrait::new(&NAME_ELEM_TRAITS)));
        m
    });
    static M_NAMES: Schema = Schema::new_member(
        ShapeId::from_static("test#ElemInput$Names", "test", "Names"),
        aws_smithy_schema::ShapeType::List,
        "Names",
        0,
    )
    .with_traits(&NAMES_TRAITS);
    static ELEM_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#ElemInput", "test", "ElemInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_NAMES],
    );

    struct ElemInput {
        names: Vec<String>,
    }
    impl SerializableStruct for ElemInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            // Real codegen writes a scalar element against the collection member
            // schema (no separate element member schema).
            s.write_list(&M_NAMES, &|sl| {
                for n in self.names.iter() {
                    sl.write_string(&M_NAMES, n)?;
                }
                Ok(())
            })?;
            Ok(())
        }
    }

    /// The walk accumulates by default, so a body that violates on EVERY element
    /// would otherwise produce one clause per element with no ceiling — and each
    /// clause echoes its own value. `MAX_VIOLATIONS` bounds the count; the echo cap
    /// bounds each clause. Both are needed: either alone is still unbounded.
    #[test]
    fn accumulated_violations_stop_at_the_ceiling() {
        let input = ElemInput {
            // Ten times the ceiling, every element too short.
            names: (0..MAX_VIOLATIONS * 10).map(|_| "x".to_string()).collect(),
        };
        let violations = validate(&input, &ELEM_INPUT_SCHEMA);
        assert_eq!(
            violations.len(),
            MAX_VIOLATIONS,
            "the walk must stop at the ceiling, not emit one clause per element"
        );
        // The rendered envelope is bounded as a consequence, which is the property
        // that actually protects the wire.
        let msg = validate_to_message(&input, &ELEM_INPUT_SCHEMA).expect("violations");
        assert!(
            msg.len() < 100_000,
            "a {}-element violating body must not render an unbounded envelope, got {} bytes",
            MAX_VIOLATIONS * 10,
            msg.len()
        );
    }

    #[test]
    fn list_element_length_reports_indexed_member_path() {
        let input = ElemInput {
            names: vec!["ok".to_string(), "x".to_string()],
        };
        let violations = validate(&input, &ELEM_INPUT_SCHEMA);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "names.2.member");
        assert_eq!(
            violations[0].message(),
            "Value 'x' at 'names.2.member' failed to satisfy constraint: \
             Member must have length greater than or equal to 2"
        );
    }

    #[test]
    fn list_element_constraints_dont_clobber_collection_length() {
        // 4 elements (> max 3) AND a too-short element: both fire, proving the
        // collection's @length and the element's @length are independent.
        let input = ElemInput {
            names: vec![
                "ok".to_string(),
                "x".to_string(),
                "fine".to_string(),
                "good".to_string(),
            ],
        };
        let violations = validate(&input, &ELEM_INPUT_SCHEMA);
        assert!(
            violations.iter().any(|v| v.path == "names"
                && v.message()
                    .contains("Member must have length less than or equal to 3")),
            "expected collection-length violation: {violations:?}"
        );
        assert!(
            violations.iter().any(|v| v.path == "names.2.member"
                && v.message().contains("greater than or equal to 2")),
            "expected element-length violation: {violations:?}"
        );
    }

    #[test]
    fn list_element_all_valid_passes() {
        let input = ElemInput {
            names: vec!["ok".to_string(), "fine".to_string()],
        };
        assert!(validate(&input, &ELEM_INPUT_SCHEMA).is_empty());
    }

    // ---- scalar map-value constraints (ElementConstraintsTrait) -------------
    //
    // A scalar map value is written against the collection (map) member schema,
    // which carries the value shape's `@pattern` via `ElementConstraintsTrait`.
    // The value's path segment is the entry key.

    static TAG_VALUE_ELEM_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(PatternTrait::new("^[a-z]+$")));
        m
    });
    static MAP_TAGS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(ElementConstraintsTrait::new(
            &TAG_VALUE_ELEM_TRAITS,
        )));
        m
    });
    static M_MAP_TAGS: Schema = Schema::new_member(
        ShapeId::from_static("test#MapInput$Tags", "test", "Tags"),
        aws_smithy_schema::ShapeType::Map,
        "Tags",
        0,
    )
    .with_traits(&MAP_TAGS_TRAITS);
    static MAP_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#MapInput", "test", "MapInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_MAP_TAGS],
    );

    struct MapInput {
        tags: Vec<(String, String)>, // ordered for a deterministic path assertion
    }
    impl SerializableStruct for MapInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_map(&M_MAP_TAGS, &|sm| {
                for (k, v) in self.tags.iter() {
                    sm.write_string(&M_MAP_TAGS, k)?;
                    sm.write_string(&M_MAP_TAGS, v)?;
                }
                Ok(())
            })?;
            Ok(())
        }
    }

    /// KNOWN DIVERGENCE, left in place deliberately.
    ///
    /// A service reports two different shapes here, chosen by WHAT violates.
    /// Descending into a structure inside the map value yields the entry key and
    /// a literal `member` (`RequestItems.<key>.member.Keys`); a constraint on the
    /// map's own value shape yields NO key segment and a bracketed wrapper
    /// (`at 'requestItems' … Map value must satisfy constraint: [...]`). Both
    /// appear in one recorded service, on the same member. This walk emits one
    /// form for both, so it reads short in the first case and long in the second.
    /// Telling them apart needs the wrapper clauses, which do not exist here yet.
    /// Do NOT "fix" this by appending `member` unconditionally — that leaves the
    /// first case right and makes the second one worse.
    #[test]
    fn map_value_pattern_reports_key_path() {
        let input = MapInput {
            tags: vec![("env".to_string(), "BAD!".to_string())],
        };
        let violations = validate(&input, &MAP_INPUT_SCHEMA);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "tags.env");
        assert!(
            violations[0]
                .message()
                .contains("Member must satisfy regular expression pattern: ^[a-z]+$"),
            "{:?}",
            violations[0]
        );
    }

    #[test]
    fn map_value_valid_passes() {
        let input = MapInput {
            tags: vec![("env".to_string(), "prod".to_string())],
        };
        assert!(validate(&input, &MAP_INPUT_SCHEMA).is_empty());
    }

    // ---- nested collections (`list<list<T>>`) -------------------------------
    //
    // Modelled on `groundstation#CreateMissionProfileRequest$dataflowEdges`
    // (`list<list<ConfigArn>>`) with the outer `@length(min: 1, max: 1)` of
    // `datazone#SageMakerRunConfigurationInput$trackingAssets`, so one fixture
    // exercises both directions:
    //
    //   - the leaf's own `@length`/`@pattern` must reach each ARN, and
    //   - the outer element-count bound must NOT reach the inner list, which
    //     shares the outer member's schema.

    /// A legal `ConfigArn`: 96 characters, matching the ARN pattern.
    const CONFIG_ARN: &str =
        "arn:aws:groundstation:us-east-1:111122223333:config/tracking/3ad6e4c8-8a1f-4b2c-9d7e-0f1a2b3c4d5e";
    /// A second legal `ConfigArn`, so an edge can hold two distinct endpoints.
    const CONFIG_ARN_2: &str =
        "arn:aws:groundstation:us-east-1:111122223333:config/downlink/9bd6e4c8-8a1f-4b2c-9d7e-0f1a2b3c4d5f";

    /// The scalar leaf `ConfigArn`'s own constraints, captured by codegen from
    /// the innermost element shape.
    static ARN_ELEM_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(LengthTrait::new(Some(82), Some(424))));
        m.insert(Box::new(PatternTrait::new("^arn:aws:groundstation:")));
        m
    });
    /// The collection member: its own `@length` and `@uniqueItems` describe the
    /// OUTER list — how many edges, and that the edges differ.
    static EDGES_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(LengthTrait::new(Some(1), Some(1))));
        m.insert(Box::new(UniqueItemsTrait::new()));
        m.insert(Box::new(ElementConstraintsTrait::new(&ARN_ELEM_TRAITS)));
        m
    });
    static M_EDGES: Schema = Schema::new_member(
        ShapeId::from_static("test#EdgeInput$Edges", "test", "Edges"),
        aws_smithy_schema::ShapeType::List,
        "Edges",
        0,
    )
    .with_traits(&EDGES_TRAITS);
    static EDGE_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#EdgeInput", "test", "EdgeInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_EDGES],
    );

    struct EdgeInput {
        edges: Vec<Vec<String>>,
    }
    impl SerializableStruct for EdgeInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            // Real codegen writes a nested list against the same collection
            // member schema — there is no inner-list member schema.
            s.write_list(&M_EDGES, &|sl| {
                for edge in self.edges.iter() {
                    sl.write_list(&M_EDGES, &|se| {
                        for arn in edge.iter() {
                            se.write_string(&M_EDGES, arn)?;
                        }
                        Ok(())
                    })?;
                }
                Ok(())
            })?;
            Ok(())
        }
    }

    #[test]
    fn nested_list_of_legal_values_passes() {
        // One outer element (legal under `@length(1, 1)`) holding two legal
        // ARNs. The inner list's own element count is 2, which the outer bound
        // would reject if it were re-applied a level down.
        let input = EdgeInput {
            edges: vec![vec![CONFIG_ARN.to_string(), CONFIG_ARN_2.to_string()]],
        };
        let violations = validate(&input, &EDGE_INPUT_SCHEMA);
        assert!(violations.is_empty(), "{violations:?}");
    }

    /// `@uniqueItems` on the member describes the OUTER list: the edges differ.
    /// Applied a level down it becomes "the ARNs within one edge differ", which
    /// is not a modelled rule.
    #[test]
    fn outer_unique_items_is_not_applied_within_a_nested_list() {
        let input = EdgeInput {
            edges: vec![vec![CONFIG_ARN.to_string(), CONFIG_ARN.to_string()]],
        };
        let violations = validate(&input, &EDGE_INPUT_SCHEMA);
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn nested_list_leaf_constraints_are_enforced() {
        let input = EdgeInput {
            edges: vec![vec![CONFIG_ARN.to_string(), "too-short".to_string()]],
        };
        let violations = validate(&input, &EDGE_INPUT_SCHEMA);
        // The short value breaks both the leaf `@length` and the leaf `@pattern`,
        // and both report at the leaf's own path — no duplicated member segment.
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(
            violations
                .iter()
                .all(|v| v.path == "edges.1.member.2.member"),
            "{violations:?}"
        );
        assert!(
            violations[0]
                .message()
                .contains("Member must have length greater than or equal to 82"),
            "{:?}",
            violations[0]
        );
        assert!(
            violations[1]
                .message()
                .contains("Member must satisfy regular expression pattern"),
            "{:?}",
            violations[1]
        );
    }

    #[test]
    fn outer_element_count_bound_still_applies_to_the_outer_list() {
        // Suppressing the nested check must not suppress the real one.
        let input = EdgeInput {
            edges: vec![vec![CONFIG_ARN.to_string()], vec![CONFIG_ARN.to_string()]],
        };
        let violations = validate(&input, &EDGE_INPUT_SCHEMA);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "edges");
        assert!(
            violations[0].message().contains("'2 items'"),
            "{:?}",
            violations[0]
        );
    }

    // ---- a nested MAP (`list<map<K, V>>`) -----------------------------------
    //
    // The map half of the same rule, which `walk_map` enforces separately. A
    // `list<map>` member — `application-signals` and `ssm` both model several —
    // carries a bound on the number of ENTRIES IN THE LIST, while the maps inside
    // it carry their own or none. Measuring an inner map's entry count against the
    // outer bound rejects a map that holds more pairs than the list may hold maps.

    static FILTERS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(LengthTrait::new(Some(1), Some(1))));
        m
    });
    static M_FILTERS: Schema = Schema::new_member(
        ShapeId::from_static("test#FilterInput$Filters", "test", "Filters"),
        aws_smithy_schema::ShapeType::List,
        "Filters",
        0,
    )
    .with_traits(&FILTERS_TRAITS);
    static FILTER_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#FilterInput", "test", "FilterInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_FILTERS],
    );

    struct FilterInput {
        /// Ordered for a deterministic path assertion.
        filters: Vec<Vec<(String, String)>>,
    }
    impl SerializableStruct for FilterInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_list(&M_FILTERS, &|sl| {
                for entries in self.filters.iter() {
                    sl.write_map(&M_FILTERS, &|sm| {
                        for (k, v) in entries.iter() {
                            sm.write_string(&M_FILTERS, k)?;
                            sm.write_string(&M_FILTERS, v)?;
                        }
                        Ok(())
                    })?;
                }
                Ok(())
            })?;
            Ok(())
        }
    }

    #[test]
    fn outer_element_count_bound_is_not_applied_to_a_nested_map() {
        // One filter (legal under `@length(1, 1)`) holding two entries. The
        // nested map shares the member schema, so measuring its entry count
        // would reject it.
        let input = FilterInput {
            filters: vec![vec![
                ("env".to_string(), "prod".to_string()),
                ("tier".to_string(), "web".to_string()),
            ]],
        };
        let violations = validate(&input, &FILTER_INPUT_SCHEMA);
        assert!(violations.is_empty(), "{violations:?}");
    }

    // ---- a list inside a struct that is itself a list element ---------------
    //
    // `CollectionFrame::Struct` must shield the inner list: it has its OWN member
    // schema, so its bound is its own and its member name belongs on the path.
    // Only a collection sharing the enclosing member's schema is suppressed.

    static INNER_TAGS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(LengthTrait::new(Some(1), Some(1))));
        m
    });
    static M_INNER_TAGS: Schema = Schema::new_member(
        ShapeId::from_static("test#Item$Tags", "test", "Tags"),
        aws_smithy_schema::ShapeType::List,
        "Tags",
        0,
    )
    .with_traits(&INNER_TAGS_TRAITS);
    static ITEM_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#Item", "test", "Item"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_INNER_TAGS],
    );
    static M_ITEMS: Schema = Schema::new_member(
        ShapeId::from_static("test#ItemInput$Items", "test", "Items"),
        aws_smithy_schema::ShapeType::List,
        "Items",
        0,
    );
    static ITEM_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#ItemInput", "test", "ItemInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[&M_ITEMS],
    );

    struct Item {
        tags: Vec<String>,
    }
    impl SerializableStruct for Item {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_list(&M_INNER_TAGS, &|sl| {
                for t in self.tags.iter() {
                    sl.write_string(&M_INNER_TAGS, t)?;
                }
                Ok(())
            })?;
            Ok(())
        }
    }
    struct ItemInput {
        items: Vec<Item>,
    }
    impl SerializableStruct for ItemInput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            // A struct element carries its own SCHEMA, which is what shields it.
            s.write_list(&M_ITEMS, &|sl| {
                for item in self.items.iter() {
                    sl.write_struct(&ITEM_SCHEMA, item)?;
                }
                Ok(())
            })?;
            Ok(())
        }
    }

    #[test]
    fn a_list_inside_a_struct_element_keeps_its_own_bound_and_segment() {
        let input = ItemInput {
            items: vec![Item {
                tags: vec!["a".to_string(), "b".to_string()],
            }],
        };
        let violations = validate(&input, &ITEM_INPUT_SCHEMA);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "items.1.member.tags");
        assert!(
            violations[0].message().contains("'2 items'"),
            "{:?}",
            violations[0]
        );
    }

    #[test]
    fn outer_element_count_bound_still_applies_to_the_list_of_maps() {
        let input = FilterInput {
            filters: vec![
                vec![("env".to_string(), "prod".to_string())],
                vec![("env".to_string(), "beta".to_string())],
            ],
        };
        let violations = validate(&input, &FILTER_INPUT_SCHEMA);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "filters");
        assert!(
            violations[0].message().contains("'2 items'"),
            "{:?}",
            violations[0]
        );
    }
}

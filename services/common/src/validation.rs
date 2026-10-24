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
//! [`refusing_violations`] is the step before it: the violations of one request, collected
//! from the framework's own deserializer and constraint walk and handed back with their
//! echoes already restored, for a service that words its own pre-handler refusal.
//!
//! Only string-rendered leaves are collected, because they are the only ones whose echo
//! is the caller's own bytes: the walk renders a blob as `<n> bytes`, a list as
//! `<n> items`, a map as `<n> entries` and a document as `<document>`, each a bounded
//! summary of the value rather than the value, and a numeric or timestamp leaf is already
//! far shorter than any elision cap.
//!
//! # A clause on the collection itself
//!
//! `@length` on a LIST is checked on the collection member, before the walk descends into
//! its elements, and the only rendering the walk has for a collection is the count —
//! `<n> items`. AWS echoes the member's VALUE there too, rendering the list the way a list
//! renders: `[<element>, <element>, …]`. So a clause whose stored echo is that summary is
//! answered from the element leaves already collected one step deeper
//! (`<list>.<1-based position>.member`), and only when ALL of them were collected — the
//! count the summary states equals the number recorded — so a list whose elements are not
//! string-rendered (a list of structures), or one the leaf caps truncated, keeps the
//! framework's own summary rather than an echo stating fewer elements than were sent.
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
//!   leaf count is over every leaf VISITED, kept or dropped, so the traversal stops at the
//!   cap even for a request whose every value is too large to keep; the walk skips any
//!   leaf that does not fit the byte budget, so the collection is never a second full copy
//!   of the request's strings. A rendered composite is bounded the same way, by child
//!   count and by the bytes its children hold ([`MAX_RENDERED_CHILDREN`]).
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
//! # What of a validation message this module now owns, and what is still to come
//!
//! Four parts of a validation-exception message are the same for EVERY service, and three
//! of them live here:
//!
//! * which value a clause echoes — the one the caller submitted ([`with_submitted_values`]),
//!   and, for a clause stated at a collection member, the collection itself
//!   ([`submitted_lists`]);
//! * a `@pattern` printed as the expression a value has to satisfy, without the `^`/`$`
//!   anchors a model writes to demand a whole-string match ([`unanchored`], and
//!   [`unanchored_grouping_alternation`] for the services whose recordings show the pair
//!   dropped from a top-level alternation, where the alternation is kept grouped so the
//!   expression still matches what it matched anchored) — LANDED here,
//!   while ~26 service crates still carry their own inlined copy
//!   (`grep -rln 'fn unanchored' services/*/src`), each pinned by that service's own
//!   recordings and tests; deleting those copies in favour of this one is the migration's
//!   own change, and nothing new should inline the rule again. The copies that are not
//!   wired to this one still decline to drop the pair around a top-level alternation, which
//!   the recordings show AWS dropping ([`unanchored`] explains why); that difference goes
//!   away as each is deleted;
//! * the subject half's one fact a violation cannot carry: WHETHER the member was absent
//!   or was present with no text rendering of its own, so a service that states
//!   `Value at '<path>' …` for a present-but-unquotable value can still state
//!   `Value null at '<path>' …` for an absent one ([`SubmittedEcho`],
//!   [`with_submitted_echoes`]) — which value a clause then quotes, and how long a value it
//!   is willing to quote, stays with the service.
//!
//! One part still belongs here and is not here yet:
//!
//! * the requirement half, read off the framework's own wording from a violation with NO
//!   value attached and split at the LAST clause marker, so neither the rejected value
//!   nor a caller-chosen map key in the member path can reach it.
//!
//! How the clauses are then counted, ordered and joined into an envelope is a service
//! DIALECT (`N validation error(s) detected: …,…` for Amazon EVS,
//! `N validation errors detected: …; …` elsewhere, and `artifact` prints a `@pattern`
//! WITH its anchors), so that half stays with the service — as does which values an enum's
//! value set lists and in which order.

use std::collections::{HashMap, HashSet};

use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{self, path, Violation, ViolationKind};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};
use smithy_server_runtime::{DeserializableStruct, Operation, ServerHttpProtocol};

/// The greatest number of violations one refusal states.
///
/// The framework's constraint walk stops collecting at this many
/// (`MAX_VIOLATIONS` in [`aws_smithy_schema_server::constraints`], which is private), so a
/// service rendering its own refusal from [`refusing_violations`] can never be handed
/// more. It is restated here, once, because a refusal body renders each violation at least
/// twice — as a clause of the message and, where the shape models one, as a field-list
/// entry — so a service that bounds its own rendering needs the same number the walk used,
/// rather than a literal of its own that can drift from it
/// (`grep -rn 'MAX_VIOLATIONS' services/*/src` lists the copies left to migrate here).
pub const MAX_VIOLATIONS: usize = 100;

/// Every violation that refuses `req` before `O`'s handler runs — a `@required` member
/// absent, or a value outside a declared constraint — with each value echo restored to the
/// bytes the caller sent ([`with_submitted_values`]).
///
/// Empty when the request satisfies the model, and empty for a failure that is NOT a
/// refusal (a body the codec cannot read at all, a member of the wrong type), which the
/// framework's own walk answers in its own envelope.
///
/// This is the collecting half every service that words its own pre-handler refusal needs:
/// both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), so a caller cannot find a different set
/// of violations from the walk that follows it, and the ONE place a request has to be
/// re-read to restore an echo is here. What is then DONE with them — which envelope, which
/// error code, which prose — is the service's own, so it stays with the service.
#[must_use]
pub fn refusing_violations<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    violations_in::<O>(protocol, req, Echoes::Submitted).violations
}

/// As [`refusing_violations`], and with it every list of SCALAR elements the request
/// carried — keyed by the member path the walk reports that list at, rendered as the value
/// AWS states AT that member ([`submitted_lists`]).
///
/// For a service whose refusal states a constraint broken by a list's ELEMENTS at the LIST
/// member: the framework reports such a violation at the element, while AWS reports it at
/// the list and echoes the value that member holds — the whole list, which no violation
/// carries. Collected together with the violations because both read the SAME built input,
/// which is alive only inside this walk, and collected ONLY when a violation names a list
/// element, so a request that satisfies the model pays for no second walk.
///
/// The map is EMPTY when the walk could not collect the request's leaves in full, so a
/// clause can never state a list the caller did not send; see [`submitted_lists`].
#[must_use]
pub fn refusing_violations_with_lists<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> (Vec<Violation>, HashMap<String, String>) {
    let refusing = violations_in::<O>(protocol, req, Echoes::SubmittedWithLists);
    (refusing.violations, refusing.lists)
}

/// What one collecting walk found: the violations, and the request's lists when the caller
/// asked for them ([`Echoes::SubmittedWithLists`]; otherwise empty).
#[derive(Debug, Default)]
pub(crate) struct Refusing {
    pub(crate) violations: Vec<Violation>,
    pub(crate) lists: HashMap<String, String>,
    /// How a clause can state each violation's value, in the same order as `violations`,
    /// when the caller asked for it ([`Echoes::Renderings`]; otherwise empty).
    pub(crate) echoes: Vec<SubmittedEcho>,
}

/// How a clause can state the value one violation is about.
///
/// The distinction [`Violation::value`] cannot carry: `None` there means the member was
/// ABSENT, so a value that was PRESENT but has no text rendering (a blob's bytes, a map, a
/// document) reaches a service as the framework's summary of it — `<n> bytes`,
/// `<n> entries`, `<document>` — which is wire text AWS never emits, and a service that
/// omits the value for exactly those members cannot tell them apart. Here such a value is
/// [`SubmittedEcho::Unrendered`], an ABSENT member is [`SubmittedEcho::Absent`], and
/// everything this walk can render — including a LIST as `[<element>, …]` and a STRUCTURE
/// as `<ShapeName>(<member>=<value>, …)`, the form the AWS SDKs print a modelled structure
/// in — is [`SubmittedEcho::Rendered`]. Which of them a clause quotes, and how long a
/// value it is willing to quote, stays with the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmittedEcho {
    /// The member was absent from the request: there is no submitted value at all.
    Absent,
    /// The value the caller submitted, rendered the way a value renders inside a clause:
    /// a string as its own text, a number as its digits, a list as `[<element>, …]`, a
    /// structure as `<ShapeName>(<member>=<value>, …)`.
    Rendered(String),
    /// A value that WAS submitted and has no text rendering this walk produces, or one
    /// past the bounds this module states. What a clause says in place of it — omit the
    /// value, name its class — is the service's own choice.
    Unrendered,
}

/// Every violation that refuses `req` before `O`'s handler runs, each paired with how a
/// clause can state the value it is about ([`SubmittedEcho`]).
///
/// The same collecting walk as [`refusing_violations`] — [`violations_in`], over the
/// framework's own deserializer and `constraints::validate` — so the two cannot find a
/// different set of violations. Only the echo differs: this one renders a COMPOSITE value
/// (a list, a structure) rather than leaving the framework's count summary in place, and
/// reports a value it cannot render as such instead of as a summary.
#[must_use]
pub fn refusing_violations_with_echoes<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<(Violation, SubmittedEcho)> {
    let refusing = violations_in::<O>(protocol, req, Echoes::Renderings);
    refusing
        .violations
        .into_iter()
        .zip(refusing.echoes)
        .collect()
}

/// Whether a collected violation's value echo is restored to the bytes the caller sent.
///
/// A service whose refusal QUOTES the value needs it restored; one whose refusal reports
/// only the class of failure (see [`crate::front_door`]) does not, and the walk that
/// restores it is work that would never be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Echoes {
    /// Restore each echo through [`with_submitted_values`].
    Submitted,
    /// Leave the framework's own rendering in place.
    Rendered,
    /// Restore each echo, and collect the request's lists too ([`submitted_lists`]).
    SubmittedWithLists,
    /// Pair each violation with how a clause can state its value
    /// ([`with_submitted_echoes`]), which renders a composite value instead of leaving the
    /// framework's count summary in place.
    Renderings,
}

/// The ONE collecting walk this crate performs: every violation that refuses `req` before
/// `O`'s handler runs, with `echoes` deciding whether each value echo is restored.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), so no caller of this can find a different
/// set of violations from the walk that follows it. What is then DONE with them — which
/// members they are narrowed to, which envelope, which error code, which prose — belongs to
/// the caller.
pub(crate) fn violations_in<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    echoes: Echoes,
) -> Refusing {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Refusing::default();
    };
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    match read {
        Ok(input) => {
            let violations = constraints::validate(&input, O::INPUT_SCHEMA);
            match echoes {
                Echoes::Submitted => Refusing {
                    violations: with_submitted_values(violations, &input, O::INPUT_SCHEMA),
                    lists: HashMap::new(),
                    echoes: Vec::new(),
                },
                Echoes::Rendered => Refusing {
                    violations,
                    lists: HashMap::new(),
                    echoes: Vec::new(),
                },
                Echoes::Renderings => {
                    let (violations, echoes) =
                        with_submitted_echoes(violations, &input, O::INPUT_SCHEMA)
                            .into_iter()
                            .unzip();
                    Refusing {
                        violations,
                        lists: HashMap::new(),
                        echoes,
                    }
                }
                Echoes::SubmittedWithLists => {
                    // The lists are only ever READ for a violation found on a list
                    // ELEMENT, so a request that broke no such constraint — which is every
                    // request that satisfies the model — pays nothing for them.
                    let wanted = violations
                        .iter()
                        .any(|violation| list_of_element(&violation.path).is_some());
                    Refusing {
                        violations: with_submitted_values(violations, &input, O::INPUT_SCHEMA),
                        lists: if wanted {
                            submitted_lists(&input, O::INPUT_SCHEMA)
                        } else {
                            HashMap::new()
                        },
                        echoes: Vec::new(),
                    }
                }
            }
        }
        // Absence is found while READING the request and a bad value while walking what
        // was read; to a caller they are one refusal, so they are one here. `member_name`
        // is already a violation path in the spelling the constraint walk uses
        // (`aws_smithy_schema_server::missing_member` applies `path::member`, and each
        // enclosing member prepends its own segment), so it places identically.
        Err(SerdeError::MissingMember { member_name }) => Refusing {
            violations: vec![Violation {
                path: member_name,
                kind: ViolationKind::MissingRequiredMember,
                value: None,
            }],
            lists: HashMap::new(),
            // An absent member has no submitted value, whichever echo the caller asked
            // for, so this stays in step with `violations` for every mode.
            echoes: vec![SubmittedEcho::Absent],
        },
        Err(_) => Refusing::default(),
    }
}

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

/// The most member positions [`submitted_renderings`] records from one request. The same
/// count bound the leaf collection states, for the same reason: the collection is never
/// sized by the request.
const MAX_RECORDED_RENDERINGS: usize = MAX_RECORDED_LEAVES;

/// The owned bytes — paths plus renderings — [`submitted_renderings`] may hold.
const MAX_RECORDED_RENDERED_BYTES: usize = MAX_RECORDED_BYTES;

/// The most children ONE composite renders from. A composite with more keeps no
/// rendering at all, rather than one stating fewer elements than were sent.
///
/// The other half of a composite's bound is its SIZE: its children may hold at most
/// [`MAX_RESTORED_ECHO`] rendering bytes together, and a child rendering past that bound
/// is never handed to a composite at all, so the text a nested value retains cannot be
/// sized by the request at any depth.
const MAX_RENDERED_CHILDREN: usize = 256;

/// `violations`, with every elided value echo replaced by the value as submitted, within
/// the bounds this module states.
///
/// `input` and `schema` must be the ones the violations were collected from — the built
/// input and its operation's input schema — so a violation's path names a leaf of this
/// walk, or the collection one step above one. A violation this walk finds neither for (an
/// absent `@required` member, a value the framework rendered as a summary it cannot rebuild
/// — a blob, a map, a document) is returned untouched, and so is one whose value is larger
/// than the stated bounds allow.
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
        // Only what the framework elided FROM this leaf, or — for a clause on a
        // collection, which has no leaf of its own — the collection the summary it stored
        // counts; anything else keeps the framework's own text. A stored echo that
        // already IS the submitted value yields nothing to restore, so it spends no
        // budget.
        let restored = match index.candidate(violation) {
            Some(submitted) if is_elision_of(stored, submitted) => {
                (stored != submitted).then(|| submitted.to_string())
            }
            _ => index.collection_candidate(violation, stored),
        };
        let Some(restored) = restored else {
            continue;
        };
        // And only within the stated bounds.
        if restored.len() > MAX_RESTORED_ECHO || restored.len() > budget {
            continue;
        }
        budget -= restored.len();
        violation.value = Some(restored);
    }
    violations
}

/// `violations`, each paired with how a clause can state the value it is about.
///
/// `input` and `schema` must be the ones the violations were collected from, exactly as
/// for [`with_submitted_values`]. A violation whose path this walk recorded no rendering
/// for — a value with no text rendering, a path past the stated bounds — is
/// [`SubmittedEcho::Unrendered`]; nothing is guessed from the framework's summary of it.
#[must_use]
pub fn with_submitted_echoes(
    violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<(Violation, SubmittedEcho)> {
    // A refusal that states no value at all (an absent `@required` member is the common
    // case) is not worth a walk over the whole input.
    if violations.iter().all(|violation| violation.value.is_none()) {
        return violations
            .into_iter()
            .map(|violation| (violation, SubmittedEcho::Absent))
            .collect();
    }
    let renderings = submitted_renderings(input, schema);
    // ONE pass over the renderings builds the lookup, so the cost is O(positions) rather
    // than O(violations x positions).
    let mut by_path: HashMap<&str, Option<&str>> = HashMap::new();
    for (position, rendering) in &renderings {
        by_path
            .entry(position.as_str())
            .or_insert(rendering.as_deref());
    }
    let mut budget = MAX_RESTORED_TOTAL;
    violations
        .into_iter()
        .map(|violation| {
            let echo = echo_of(&violation, &by_path, &mut budget);
            (violation, echo)
        })
        .collect()
}

/// How a clause can state `violation`'s value, within the bounds this module states.
fn echo_of(
    violation: &Violation,
    by_path: &HashMap<&str, Option<&str>>,
    budget: &mut usize,
) -> SubmittedEcho {
    if violation.value.is_none() {
        return SubmittedEcho::Absent;
    }
    let rendered = match violation.kind {
        // A `@uniqueItems` clause is reported at the LIST's path but states the ELEMENT
        // that repeated, where the rendering recorded is the whole list. The framework's
        // own echo IS that element, already bounded, so it stands.
        ViolationKind::UniqueItems => violation.value.as_deref(),
        _ => match by_path.get(violation.path.as_str()) {
            Some(Some(rendering)) => Some(*rendering),
            // A value with no text rendering, or a position this walk recorded nothing
            // for (its bounds, or a path the frozen crate spells differently).
            _ => None,
        },
    };
    match rendered {
        Some(rendering) if rendering.len() <= MAX_RESTORED_ECHO && rendering.len() <= *budget => {
            *budget -= rendering.len();
            SubmittedEcho::Rendered(rendering.to_string())
        }
        _ => SubmittedEcho::Unrendered,
    }
}

/// The collected leaves of one request, indexed for the lookups a violation needs.
///
/// All three maps borrow the collected leaves and are built in a single pass over them.
#[derive(Debug)]
struct Submitted<'l> {
    /// The first leaf recorded at each dotted path: the member a clause names.
    by_path: HashMap<&'l str, &'l str>,
    /// For each list whose scalar elements were recorded, the first element value that
    /// repeated — what a `@uniqueItems` clause echoes, reported at the LIST's path.
    duplicate_in: HashMap<&'l str, &'l str>,
    /// For each list whose scalar elements were recorded, those elements in walk order —
    /// what a clause on the LIST itself echoes, reported at the list's own path.
    elements_in: HashMap<&'l str, Vec<&'l str>>,
}

impl<'l> Submitted<'l> {
    fn of(leaves: &'l [(String, String)]) -> Self {
        // `leaves` is already bounded by `MAX_RECORDED_LEAVES` / `MAX_RECORDED_BYTES`, so
        // every map is too.
        let mut by_path: HashMap<&str, &str> = HashMap::new();
        let mut seen: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut duplicate_in: HashMap<&str, &str> = HashMap::new();
        let mut elements_in: HashMap<&str, Vec<&str>> = HashMap::new();
        for (leaf_path, value) in leaves {
            by_path.entry(leaf_path.as_str()).or_insert(value.as_str());
            if let Some(list) = list_of_element(leaf_path) {
                if !seen.entry(list).or_default().insert(value.as_str()) {
                    duplicate_in.entry(list).or_insert(value.as_str());
                }
                elements_in.entry(list).or_default().push(value.as_str());
            }
        }
        Self {
            by_path,
            duplicate_in,
            elements_in,
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

    /// The COLLECTION `violation`'s stored echo summarises, rendered as the value the
    /// caller submitted, when the two describe the same collection.
    ///
    /// A `@length` clause on a list is reported at the LIST's path, where the walk records
    /// no leaf of its own — the elements are recorded one step deeper — and the echo it
    /// stored is the framework's count rendering rather than the value. The count is what
    /// ties the two together: a summary of `<n> items` is rebuilt only from exactly `n`
    /// recorded elements, so a partially collected list (the leaf caps) and a list whose
    /// elements have no string rendering yield nothing and keep the framework's summary.
    fn collection_candidate(&self, violation: &Violation, stored: &str) -> Option<String> {
        let count = collection_count(stored)?;
        let elements = self.elements_in.get(violation.path.as_str())?;
        (elements.len() == count).then(|| rendered_list(elements))
    }
}

/// How many elements the framework's rendering of a collection states, when `stored` is
/// one: the walk renders a list as `<n> items`
/// ([`aws_smithy_schema_server::constraints`]'s collection-length check).
fn collection_count(stored: &str) -> Option<usize> {
    stored.strip_suffix(" items")?.parse::<usize>().ok()
}

/// `elements` as a list renders: the elements in order, comma-separated, in brackets —
/// the form AWS echoes a list-valued member back in.
fn rendered_list(elements: &[&str]) -> String {
    format!("[{}]", elements.join(", "))
}

/// `regex` without the whole-value anchors a model adds to express the match AWS's own
/// pattern makes implicitly: the requirement a refusal states is the expression a value has
/// to satisfy, and the `^` / `$` are how Smithy turns a find into a whole-string match
/// rather than part of the character grammar — so a client that lifts the expression out of
/// the message and re-validates locally gets the one the service advertises.
///
/// THE STAGED HOME for this rendering, which the module docs above list as one of the parts
/// of a validation message that are AWS's rather than any one service's. ~26 service crates
/// carry their own inlined copy (`grep -rln 'fn unanchored' services/*/src`), each pinned by
/// that service's own recordings; those copies are to be DELETED in favour of this one by the
/// change that owns that migration, and nothing new should inline the rule again.
///
/// The pair is dropped POSITIONALLY: the leading `^` and the trailing `$`, and only when
/// BOTH ends carry one. A `^` alone, or a `$` alone, is an expression anchored at one end —
/// a different expression, not an anchored spelling of this one — so it is emitted
/// verbatim, as is an expression that carries no `^` at its start or no `$` at its end
/// because its own branches carry them (`(^a$)|(^b$)`). A `$` that is ESCAPED is a literal
/// dollar, not an anchor, so it stays and nothing is dropped.
///
/// WHY POSITIONALLY, and not only when the pair demonstrably wraps the whole expression:
/// the published `@pattern` is the service's own full-match expression with that pair
/// written around it by the model, so recovering the expression the service advertises is
/// the textual inverse of that wrapping. The recordings show it applied to a pattern whose
/// top-level alternation makes the pair's placement semantically significant —
/// `com.amazonaws.lambdacore#NetworkConnectorName` is modelled
/// `^(arn:…:network-connector:…)|[a-zA-Z0-9_-]{1,64}$` and a refusal of
/// `CreateNetworkConnector$Name` quotes `(arn:…:network-connector:…)|[a-zA-Z0-9_-]{1,64}` —
/// so a renderer that declined the strip there would state an expression AWS never
/// publishes. The CHECK always runs the model's pattern as written; only this rendering
/// drops the pair.
///
/// THE CONSEQUENCE, stated plainly: an expression whose own branches carry the pair at the
/// ends (`^a$|^b$`, `^a|b$`) is quoted as `a$|^b` / `a|b`, which does NOT describe the same
/// language as the model's text. That is the rule's faithful output, not an oversight: such
/// a model text is itself the wrapping of an expression that already anchored a branch, and
/// what AWS publishes is that inner expression. A model that means an alternation of
/// whole-value branches parenthesises them (`(^a$)|(^b$)`), which carries no outer pair and
/// is quoted verbatim. Every one of these forms is a row in the module's own table test.
#[must_use]
pub fn unanchored(regex: &str) -> String {
    let Some(body) = regex.strip_prefix('^') else {
        return regex.to_string();
    };
    match body.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) => inner.to_string(),
        _ => regex.to_string(),
    }
}

/// [`unanchored`], with a top-level ALTERNATION left GROUPED so the expression quoted still
/// matches exactly what the model's anchored text matched.
///
/// Dropping the pair from `^a|b$` turns "starts with `a`, or ends with `b`" into "contains
/// `a`, or contains `b`"; wrapping what is left in one group — `(a|b)` — restores it, since
/// that expression matches, anywhere in a value, exactly what the anchored one matched at
/// the ends. A service whose recordings show the quoted expression grouped states it this
/// way; one whose recordings show it bare (lambdacore's `NetworkConnectorName`, quoted in
/// [`unanchored`]) states the plain rendering. Which of the two a service uses is part of
/// its validation-message dialect (the module docs above), so the choice stays with the
/// service and both rules live here.
///
/// The DROP is [`unanchored`]'s, called rather than restated, so the two cannot disagree on
/// it: this adds exactly the group, and only when the pair was dropped AND what is left
/// alternates at the top level. An alternation already inside a group (`^(a|b)$`) is not
/// wrapped again, and neither is an expression that keeps its pair.
#[must_use]
pub fn unanchored_grouping_alternation(regex: &str) -> String {
    let stated = unanchored(regex);
    if stated == regex || !alternates_at_top_level(&stated) {
        return stated;
    }
    format!("({stated})")
}

/// Whether `text` alternates at its TOP level — a `|` that is not inside a group and not
/// inside a character class, where it is the literal character. Escapes are honoured
/// throughout, so an escaped `|`, `[`, `]`, `(` or `)` is a literal too.
///
/// A `]` in the FIRST position of a class is the literal bracket rather than the class's
/// end (`[]|]` is the two-character class `]|`, not an empty class followed by `|]`), and a
/// leading `^` does not count as a position, so `[^]|]` is read the same way. Tracking the
/// position inside the class is what keeps a `|` in such a class from being read as an
/// alternation.
fn alternates_at_top_level(text: &str) -> bool {
    let mut escaped = false;
    // `Some(n)`: inside a class, with `n` characters seen in it (a leading `^` excluded).
    let mut in_class: Option<usize> = None;
    let mut depth = 0_usize;
    for character in text.chars() {
        if escaped {
            escaped = false;
            // An escaped character still occupies a position in the class it is in.
            in_class = in_class.map(|seen| seen + 1);
            continue;
        }
        match (character, in_class) {
            ('\\', _) => escaped = true,
            // The negation marker, which is not a position of its own.
            ('^', Some(0)) => {}
            // A `]` where the class has no members yet is the literal bracket.
            (']', Some(0)) => in_class = Some(1),
            (']', Some(_)) => in_class = None,
            (_, Some(seen)) => in_class = Some(seen + 1),
            ('[', None) => in_class = Some(0),
            ('(', None) => depth += 1,
            (')', None) => depth = depth.saturating_sub(1),
            ('|', None) if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Whether `text` ends in an escape — an odd number of trailing backslashes, so the
/// character that followed them was escaped.
fn ends_with_escape(text: &str) -> bool {
    text.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// The list path a leaf is a SCALAR ELEMENT of, if it is one.
///
/// A list element's path is `<list>.<1-based position>.member`
/// ([`aws_smithy_schema_server::constraints::path::element`]), so the list is that path
/// minus those two segments. Anything else — a struct member, a map entry value — belongs
/// to no list's uniqueness check.
///
/// Public because a service that states a clause at the LIST member when one of its
/// ELEMENTS failed has to recognise an element path to do it, and that spelling belongs
/// with the walk that mirrors it rather than being re-derived per service.
#[must_use]
pub fn list_of_element(leaf_path: &str) -> Option<&str> {
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

/// Every list of SCALAR elements `input` carried, keyed by the member path the validating
/// walk reports that list at, and rendered as the value AWS states AT that member: the
/// elements in position order, comma-separated, in brackets ([`rendered_list`]).
///
/// What this is for: a constraint on a list's element shape is violated by ONE element and
/// the framework reports it at that element (`eventPublishers.1.member`), while AWS reports
/// it at the list member and echoes the value that member holds — the WHOLE list, which no
/// violation carries. A service wording such a clause reads the list from here rather than
/// rebuilding the walk, and never from a violation.
///
/// A list this walk could not collect in FULL is NOT offered, so a clause can never state
/// a list the caller did not send. The leaf collection is bounded ([`MAX_RECORDED_LEAVES`],
/// [`MAX_RESTORED_ECHO`], [`MAX_RECORDED_BYTES`]) and a leaf those bounds drop leaves a hole
/// the leaves alone cannot reveal — a short list reads exactly like a short list the caller
/// sent — so once ANY leaf has been dropped ([`SubmittedValues::skipped`]) nothing is
/// offered at all and the framework's own echo stands. Within the bounds every rendering is
/// covered by [`MAX_RECORDED_BYTES`], the budget the leaves themselves were collected
/// under.
///
/// A list whose elements have no string rendering (a list of structures, a list of blobs)
/// contributes no entry either, for the same reason: its elements are not recorded.
#[must_use]
pub fn submitted_lists(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> HashMap<String, String> {
    let (leaves, complete) = collected_leaves(input, schema);
    if !complete {
        return HashMap::new();
    }
    let mut elements: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (leaf_path, value) in &leaves {
        let Some(list) = list_of_element(leaf_path) else {
            continue;
        };
        let held = elements.entry(list).or_default();
        if held.is_empty() {
            order.push(list);
        }
        held.push(value.as_str());
    }
    order
        .into_iter()
        .map(|list| (list.to_string(), rendered_list(&elements[list])))
        .collect()
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
    collected_leaves(input, schema).0
}

/// The collected leaves of `input`, and whether the collection is COMPLETE — false once any
/// string-rendered leaf was dropped by one of the stated bounds (see
/// [`SubmittedValues::skipped`]).
///
/// A caller that reads ONE named leaf does not care: a dropped leaf yields no candidate and
/// the framework's own text stands. A caller that reads a whole COLLECTION does, because a
/// hole in the leaves is invisible in the collection it renders.
fn collected_leaves(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> (Vec<(String, String)>, bool) {
    let mut walk = SubmittedValues::default();
    // The root input is a struct; drive it the way a codec — and the validating walk —
    // does.
    let _ = walk.write_struct(schema, input);
    (walk.leaves, !walk.skipped)
}

/// Every member position of `input` this walk reaches, in walk order, paired with the
/// rendering a clause states the value held there with — `None` for a value this walk
/// produces no text rendering for (a blob, a map, a document) or one past its bounds.
///
/// The SIBLING of [`submitted_leaves`] and [`submitted_lists`], over the SAME walk and so
/// the same path spelling: the positions are the ones the validating walk reports a
/// violation at, including a COMPOSITE member (a list, a structure), which that walk
/// reports a `@length` clause on and has only a count rendering for.
///
/// A composite renders from its children: a list as `[<element>, …]`, a structure as
/// `<ShapeName>(<member>=<value>, …)` over the members that were present. A composite
/// with any unrenderable child renders as `None` rather than as one missing a part of the
/// value, and so does a map (no rule for how AWS states one in a clause is known here)
/// and a structure reached as a struct MEMBER, whose schema names the member rather than
/// the shape.
///
/// Bounded by [`MAX_RECORDED_RENDERINGS`], [`MAX_RECORDED_RENDERED_BYTES`],
/// [`MAX_RENDERED_CHILDREN`] and [`MAX_RESTORED_ECHO`], so neither the memory nor the
/// text this produces is sized by the request.
#[must_use]
pub fn submitted_renderings(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<(String, Option<String>)> {
    // The ONLY walk that composes renderings: a caller that reads one named leaf
    // ([`submitted_leaves`], [`submitted_lists`]) pays nothing for them.
    let mut walk = SubmittedValues {
        renders: true,
        ..SubmittedValues::default()
    };
    let _ = walk.write_struct(schema, input);
    walk.renderings
}

/// One child of an open composite: the member name it reported under (absent for a
/// collection element) and how its value renders.
#[derive(Debug)]
struct RenderedChild {
    name: Option<String>,
    rendering: Option<String>,
}

/// The children of one composite being walked, and whether more arrived than
/// [`MAX_RENDERED_CHILDREN`] allows, one of them was larger than a composite may hold,
/// or one of them was never written.
#[derive(Debug, Default)]
struct Composite {
    children: Vec<RenderedChild>,
    /// Rendering bytes the children hold together, bounded by [`MAX_RESTORED_ECHO`]: a
    /// composite that could never be quoted in a clause is not worth holding, and holding
    /// it is what would let one request's text be retained at every nesting level.
    bytes: usize,
    truncated: bool,
}

impl Composite {
    /// Take one child, or mark this composite truncated once it is full — by child COUNT
    /// or by the bytes its children already hold. A truncated composite renders as nothing
    /// ([`SubmittedValues::close_children`]), so dropping a child never states a value
    /// missing a part of itself.
    fn take(&mut self, child: RenderedChild) {
        let bytes = child.name.as_ref().map_or(0, String::len)
            + child.rendering.as_ref().map_or(0, String::len);
        if self.children.len() >= MAX_RENDERED_CHILDREN
            || self.bytes + bytes > MAX_RESTORED_ECHO
        {
            self.truncated = true;
            return;
        }
        self.bytes += bytes;
        self.children.push(child);
    }
}

/// The text `children` would render to, including the separators and brackets a list or a
/// structure puts between them, or `None` when any child has no text rendering.
///
/// Measured BEFORE anything is joined, so a rendering past [`MAX_RESTORED_ECHO`] is never
/// allocated only to be discarded.
fn rendered_length(children: &[RenderedChild], named: bool) -> Option<usize> {
    let mut length = 2; // the brackets, or the parentheses after a shape's name
    for (index, child) in children.iter().enumerate() {
        if index > 0 {
            length += 2; // ", "
        }
        if named {
            // `<member>=`
            length += child.name.as_ref()?.len() + 1;
        }
        length += child.rendering.as_ref()?.len();
    }
    Some(length)
}

/// `children` as a list renders: the elements in order, comma-separated, in brackets,
/// through the one renderer of that form ([`rendered_list`]). `None` when any element has
/// no text rendering, so a list of blobs states no value rather than one missing an
/// element, and `None` when the rendering would be longer than [`MAX_RESTORED_ECHO`] —
/// measured before it is built, so an oversized rendering is never allocated.
fn rendered_sequence(children: &[RenderedChild]) -> Option<String> {
    if rendered_length(children, false)? > MAX_RESTORED_ECHO {
        return None;
    }
    let elements: Vec<&str> = children
        .iter()
        .map(|child| child.rendering.as_deref())
        .collect::<Option<Vec<_>>>()?;
    Some(rendered_list(&elements))
}

/// `children` as a structure renders: the shape's name over the members that were
/// present, each `<member>=<value>`, in the order the value wrote them — the form the AWS
/// SDKs print a modelled structure in. `None` when a member that WAS written has no text
/// rendering, so a structure carrying a blob or a map states no value at all rather than
/// one missing a part of itself. (An ABSENT member is never written, so it is not a child
/// here and does not suppress the rendering.) `None` too when the rendering would be
/// longer than [`MAX_RESTORED_ECHO`], measured before it is built.
fn rendered_structure(name: &str, children: &[RenderedChild]) -> Option<String> {
    if name.len() + rendered_length(children, true)? > MAX_RESTORED_ECHO {
        return None;
    }
    let members: Vec<String> = children
        .iter()
        .map(|child| {
            let member = child.name.as_deref()?;
            let rendering = child.rendering.as_deref()?;
            Some(format!("{member}={rendering}"))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(format!("{name}({})", members.join(", ")))
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
    /// Every member position reached, paired with its clause rendering — see
    /// [`submitted_renderings`]. Empty unless `renders`.
    renderings: Vec<(String, Option<String>)>,
    /// Owned bytes `renderings` holds: positions plus renderings.
    rendered: usize,
    /// Leaf positions VISITED, recorded or not: the walk's own stop condition
    /// ([`Self::done`]), which must not depend on whether a leaf was kept.
    visited: usize,
    /// Whether this walk composes renderings at all. Off by default, so a caller that
    /// only reads leaves pays neither the frames nor the owned text for them.
    renders: bool,
    /// The composites currently open, innermost last, collecting the children each
    /// renders from. Pushed and popped with `collections`.
    open: Vec<Composite>,
    /// Dotted path segments from the root to the value being written.
    path: Vec<String>,
    /// Active collection frames, innermost last.
    collections: Vec<Frame>,
    /// Set once any string-rendered leaf was DROPPED rather than recorded — because the
    /// leaf cap stopped the walk, or because one value was larger than
    /// [`MAX_RESTORED_ECHO`], or because the byte budget was spent. The collection is then
    /// no longer a faithful picture of the request, which matters to a caller that reads a
    /// WHOLE collection out of it ([`submitted_lists`]) rather than one named leaf: with a
    /// leaf missing, a list would render short, stating a value the caller never sent.
    /// Restoring a single named echo is unaffected, since an absent leaf simply yields no
    /// candidate.
    skipped: bool,
}

impl SubmittedValues {
    /// True once no further leaf can be recorded, so the rest of the walk is skipped.
    ///
    /// Counted over every leaf VISITED rather than over the leaves kept: a leaf dropped
    /// for its size is not recorded, so a request made of oversized values would
    /// otherwise keep this false and let the walk run over every member of it.
    ///
    /// Reaching the cap also means a leaf the walk has not got to will be dropped, so it
    /// records that the collection is incomplete.
    fn done(&mut self) -> bool {
        let done = self.visited >= MAX_RECORDED_LEAVES;
        self.skipped |= done;
        done
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

    /// The walk stopped before the value now being written: the composite that value
    /// belongs to is missing a child, so it must render as nothing rather than as a value
    /// with a part of itself left out.
    fn abandoned(&mut self) {
        if let Some(open) = self.open.last_mut() {
            open.truncated = true;
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
        // smaller leaf can still be recorded, so this is not terminal — but the collection
        // now has a hole in it, which `skipped` records.
        let leaf_path = path::join(&self.path);
        let bytes = leaf_path.len() + value.len();
        if value.len() > MAX_RESTORED_ECHO || self.recorded + bytes > MAX_RECORDED_BYTES {
            self.skipped = true;
            return;
        }
        self.recorded += bytes;
        self.leaves.push((leaf_path, value.to_string()));
    }

    /// Record the position now being written with how a clause renders the value it held,
    /// and hand that rendering to the composite this value is a child of, if any.
    ///
    /// Called with the path still naming this value, and AFTER a composite's own frame is
    /// closed, so the child lands on its PARENT.
    fn produced(&mut self, schema: &Schema, rendering: Option<String>) {
        if !self.renders {
            return;
        }
        let position = path::join(&self.path);
        // A rendering no clause could carry (see `MAX_RESTORED_ECHO`) is neither kept nor
        // handed to the composite this value is a child of: holding it would retain
        // request-sized text once per nesting level. The composite then renders as
        // nothing, rather than as a value missing a part of itself.
        let rendering = rendering.filter(|rendered| rendered.len() <= MAX_RESTORED_ECHO);
        let bytes = position.len() + rendering.as_ref().map_or(0, String::len);
        if self.renderings.len() < MAX_RECORDED_RENDERINGS
            && self.rendered + bytes <= MAX_RECORDED_RENDERED_BYTES
        {
            self.rendered += bytes;
            self.renderings.push((position, rendering.clone()));
        }
        if let Some(open) = self.open.last_mut() {
            open.take(RenderedChild {
                name: Self::segment_for(schema),
                rendering,
            });
        }
    }

    /// One leaf write. `value` carries the leaf's text when its echo is the caller's own
    /// bytes (a string, or a big-number rendering) and is `None` for every other leaf
    /// kind — which is still walked, so a later leaf's path stays correct. `rendering` is
    /// how a clause states that value, which a number or a boolean has even though its
    /// text is not the caller's own bytes, and a blob or a document has not at all.
    fn leaf(&mut self, schema: &Schema, value: Option<&str>, rendering: Option<&str>) {
        if self.done() {
            self.abandoned();
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
        let rendering = rendering.map(str::to_string);
        // Every leaf POSITION counts against the walk's budget, whether or not its value
        // is kept, so the traversal is bounded by the cap even when nothing is recorded.
        self.visited += 1;
        match self.begin_child(true) {
            Some(segment) => {
                self.path.push(segment);
                self.record(value);
                self.produced(schema, rendering);
                self.path.pop();
                self.end_child();
            }
            None => self.with_segment(schema, |this| {
                this.record(value);
                this.produced(schema, rendering);
            }),
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
            self.abandoned();
            return Ok(());
        }
        let element = self.begin_child(false);
        let is_element = element.is_some();
        if let Some(segment) = element {
            self.path.push(segment);
        }
        let out = if is_element {
            self.walk_children(schema, frame, write_children)
        } else {
            self.with_segment(schema, |this| this.walk_children(schema, frame, write_children))
        };
        if is_element {
            self.path.pop();
            self.end_child();
        }
        out
    }

    /// Open `frame`, write the composite's children into it, then close it and record how
    /// the composite itself renders.
    fn walk_children(
        &mut self,
        schema: &Schema,
        frame: Frame,
        write_children: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.collections.push(frame);
        if self.renders {
            self.open.push(Composite::default());
        }
        let out = write_children(self);
        let rendering = self.close_children(schema);
        self.produced(schema, rendering);
        out
    }

    /// Close the innermost composite and render it from the children it collected.
    fn close_children(&mut self, schema: &Schema) -> Option<String> {
        let frame = self.collections.pop();
        if !self.renders {
            return None;
        }
        let composite = self.open.pop().unwrap_or_default();
        if composite.truncated {
            return None;
        }
        let rendering = match frame {
            Some(Frame::List { .. }) => rendered_sequence(&composite.children),
            // A structure renders under its SHAPE's name, which only the shape's own
            // schema carries: a list element is written against the element struct's own
            // schema (`&Inner::SCHEMA`, from the generated `serialize_members`), while a
            // struct reached as a MEMBER carries the member schema, whose name is the
            // member's. Rendering that one would state `<member>(…)`, a form AWS never
            // emits, so such a structure renders as nothing and the service states no
            // value for it.
            Some(Frame::Struct) if schema.member_name().is_none() => {
                rendered_structure(schema.shape_id().shape_name(), &composite.children)
            }
            // A map: no rule for how AWS states one inside a clause is known here, so a
            // clause on a map member states no value rather than a form AWS never emits.
            _ => None,
        };
        rendering.filter(|rendered| rendered.len() <= MAX_RESTORED_ECHO)
    }
}

impl ShapeSerializer for SubmittedValues {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        if self.done() {
            self.abandoned();
            return Ok(());
        }
        // A struct is the root input (no member name, no segment), a nested struct member
        // (member name -> segment), a list element or a map value (the frame supplies the
        // segment).
        let write_members = |s: &mut dyn ShapeSerializer| value.serialize_members(s);
        match self.begin_child(false) {
            Some(segment) => {
                self.path.push(segment);
                let out = self.walk_children(schema, Frame::Struct, &write_members);
                self.path.pop();
                self.end_child();
                out
            }
            None => self.with_segment(schema, |this| {
                this.walk_children(schema, Frame::Struct, &write_members)
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

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        // A number or a boolean is not the caller's own BYTES (so no leaf is recorded for
        // re-echoing one), but a clause does state it, in the same text the validating
        // walk renders it with.
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value.as_ref()), Some(value.as_ref()));
        Ok(())
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value.as_ref()), Some(value.as_ref()));
        Ok(())
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value), Some(value));
        Ok(())
    }

    fn write_blob(&mut self, schema: &Schema, _value: &[u8]) -> Result<(), SerdeError> {
        // The walk renders a blob as its byte count, never its bytes, so there is no
        // caller-submitted text to re-echo here — and no text rendering of the value for
        // a clause either.
        self.leaf(schema, None, None);
        Ok(())
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_document(&mut self, schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        // The walk renders a document as `<document>`, which is a summary rather than the
        // value, so this position has no text rendering.
        self.leaf(schema, None, None);
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
        echo_of, is_elision_of, list_of_element, rendered_sequence, rendered_structure, unanchored,
        unanchored_grouping_alternation, Composite, RenderedChild, Submitted, SubmittedEcho,
        SubmittedValues, MAX_RECORDED_BYTES, MAX_RECORDED_LEAVES, MAX_RECORDED_RENDERED_BYTES,
        MAX_RECORDED_RENDERINGS, MAX_RENDERED_CHILDREN, MAX_RESTORED_ECHO, MAX_RESTORED_TOTAL,
    };
    use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
    use std::collections::HashMap;

    /// RULE: a `@pattern` is quoted as the model's expression minus the leading `^` and the
    /// trailing `$` the model writes around the service's own full-match expression — the
    /// pair dropped positionally, and only when both ends carry one.
    #[test]
    fn a_pattern_loses_the_anchor_pair_the_model_wraps_it_in() {
        // (case, the modelled expression, the expression a clause quotes)
        let rows: [(&str, &str, &str); 11] = [
            ("a wrapped character class", r"^[a-z]+$", r"[a-z]+"),
            ("a wrapped group", r"^([\p{L}\p{N}_.:/=+\-@]*)$", r"([\p{L}\p{N}_.:/=+\-@]*)"),
            // The recorded case: the pair is dropped even though the expression
            // alternates at the top level (`NetworkConnectorName`, lambdacore).
            (
                "a wrapped top-level alternation",
                r"^(arn:aws:lambda:[a-z-]+-\d:\d{12}:network-connector:[a-z0-9-]+)|[a-zA-Z0-9_-]{1,64}$",
                r"(arn:aws:lambda:[a-z-]+-\d:\d{12}:network-connector:[a-z0-9-]+)|[a-zA-Z0-9_-]{1,64}",
            ),
            ("anchored at the start only", r"^arn:aws.*", r"^arn:aws.*"),
            ("anchored at the end only", r"[0-9]{12}$", r"[0-9]{12}$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            // Anchored inside its branches rather than at its ends: no outer pair.
            ("anchors inside the branches", r"(^a$)|(^b$)", r"(^a$)|(^b$)"),
            // The same two branches WITHOUT the parentheses carry the pair at the ends,
            // so the pair is what is dropped — the rule's faithful output even though the
            // result describes another language. See this function's doc comment.
            ("branches sharing the outer pair", r"^a$|^b$", r"a$|^b"),
            ("one branch anchored at each end", r"^a|b$", r"a|b"),
            // An escaped trailing `$` is a literal dollar, not the end anchor.
            ("an escaped trailing dollar", r"^costs\$", r"^costs\$"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            ("an escaped backslash then the anchor", r"^[a-z]\\$", r"[a-z]\\"),
        ];
        for (case, modelled, expected) in rows {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {modelled:?} to be quoted as {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    /// RULE: the collection is COMPLETE only while every string-rendered leaf was
    /// recorded; any leaf one of the stated bounds drops marks it incomplete, which is what
    /// stops [`super::submitted_lists`] from offering a collection with a hole in it.
    #[test]
    fn a_leaf_a_bound_dropped_marks_the_collection_incomplete() {
        // (case, the leaf values written in order, whether the walk stayed complete)
        let rows: [(&str, Vec<String>, bool); 5] = [
            ("nothing written at all", Vec::new(), true),
            (
                "two small leaves",
                vec!["first".to_string(), "second".to_string()],
                true,
            ),
            (
                "a leaf at the per-value ceiling",
                vec!["x".repeat(MAX_RESTORED_ECHO)],
                true,
            ),
            (
                "a leaf one byte past the per-value ceiling",
                vec!["x".repeat(MAX_RESTORED_ECHO + 1)],
                false,
            ),
            (
                "leaves that exhaust the byte budget",
                (0..=MAX_RECORDED_BYTES / MAX_RESTORED_ECHO)
                    .map(|_| "y".repeat(MAX_RESTORED_ECHO))
                    .collect(),
                false,
            ),
        ];

        for (case, values, expected) in rows {
            let mut walk = SubmittedValues::default();
            walk.path.push("member".to_string());
            for value in &values {
                walk.record(Some(value));
            }
            assert_eq!(
                !walk.skipped, expected,
                "case {case:?}: expected complete == {expected}, got {} (recorded {} leaves)",
                !walk.skipped,
                walk.leaves.len()
            );
        }
    }

    /// RULE: reaching the leaf cap also marks the collection incomplete — the walk stops
    /// there, so a leaf beyond it is a leaf that was dropped — and the cap is counted over
    /// every leaf VISITED, so a request whose values are all dropped for their size still
    /// stops the walk.
    #[test]
    fn the_leaf_cap_marks_the_collection_incomplete() {
        let mut walk = SubmittedValues::default();
        walk.path.push("member".to_string());
        assert!(
            !walk.done() && !walk.skipped,
            "case an empty walk: expected it to be complete and not done"
        );
        for index in 0..MAX_RECORDED_LEAVES {
            walk.visited += 1;
            walk.record(Some(&index.to_string()));
        }
        assert_eq!(
            walk.leaves.len(),
            MAX_RECORDED_LEAVES,
            "case a full walk: expected the cap to be reached, got {} leaves",
            walk.leaves.len()
        );
        assert!(
            walk.done() && walk.skipped,
            "case a full walk: expected it to be done and incomplete"
        );

        // Nothing is recorded for a value past the echo bound, so a walk whose stop
        // condition counted RECORDED leaves would never stop on a request made of them.
        let mut dropping = SubmittedValues::default();
        dropping.path.push("member".to_string());
        let oversized = "x".repeat(MAX_RESTORED_ECHO + 1);
        for _ in 0..MAX_RECORDED_LEAVES {
            dropping.visited += 1;
            dropping.record(Some(&oversized));
        }
        assert!(
            dropping.leaves.is_empty(),
            "case a walk of oversized values: expected nothing recorded, got {} leaves",
            dropping.leaves.len()
        );
        assert!(
            dropping.done() && dropping.skipped,
            "case a walk of oversized values: expected it to be done and incomplete"
        );
    }

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

    /// RULE: a clause on a LIST — whose stored echo is the framework's `<n> items`
    /// summary — is answered with the list rendered from exactly the `n` elements
    /// recorded under it, and with nothing when the two do not agree.
    #[test]
    fn a_collection_clause_is_answered_with_the_list_as_submitted() {
        let leaves: Vec<(String, String)> = [
            ("resourceIds.1.member", "table/a"),
            ("resourceIds.2.member", "table/b"),
            ("resourceIds.3.member", "table/c"),
            ("policyNames.1.member", "only"),
            ("serviceNamespace", "dynamodb"),
        ]
        .into_iter()
        .map(|(leaf_path, value)| (leaf_path.to_string(), value.to_string()))
        .collect();
        let index = Submitted::of(&leaves);

        // (case, the clause's path, the echo the framework stored, the restored echo)
        let rows: [(&str, &str, &str, Option<&str>); 6] = [
            (
                "a list of three",
                "resourceIds",
                "3 items",
                Some("[table/a, table/b, table/c]"),
            ),
            (
                "a list of one, which the framework still renders in the plural",
                "policyNames",
                "1 items",
                Some("[only]"),
            ),
            (
                "a summary counting more elements than were recorded",
                "resourceIds",
                "51 items",
                None,
            ),
            ("a list no element was recorded for", "tagKeys", "2 items", None),
            (
                "a clause on the list whose echo is not a collection summary",
                "resourceIds",
                "table/a",
                None,
            ),
            (
                "a scalar member, whose echo is never a collection summary",
                "serviceNamespace",
                "dynamodb",
                None,
            ),
        ];
        for (case, path, stored, expected) in rows {
            let violation = Violation {
                path: path.to_string(),
                kind: ViolationKind::MaxLength(50),
                value: Some(stored.to_string()),
            };
            let actual = index.collection_candidate(&violation, stored);
            assert_eq!(
                actual.as_deref(),
                expected,
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
            assert!(MAX_RECORDED_RENDERINGS > 0 && MAX_RECORDED_RENDERED_BYTES > 0);
            assert!(MAX_RENDERED_CHILDREN > 0);
        };
        let () = COHERENT;
    }

    /// RULE: a composite renders from its children — a list as `[<element>, …]`, a
    /// structure as `<ShapeName>(<member>=<value>, …)` over the members that were
    /// present — and renders as nothing at all when a child has no text rendering.
    #[test]
    fn a_composite_renders_from_the_children_it_was_given() {
        let child = |name: Option<&str>, rendering: Option<&str>| RenderedChild {
            name: name.map(str::to_string),
            rendering: rendering.map(str::to_string),
        };
        let sequences: [(&str, Vec<RenderedChild>, Option<&str>); 4] = [
            ("no elements", vec![], Some("[]")),
            ("one element", vec![child(None, Some("a"))], Some("[a]")),
            (
                "three elements keep their order",
                vec![
                    child(None, Some("a")),
                    child(None, Some("b")),
                    child(None, Some("c")),
                ],
                Some("[a, b, c]"),
            ),
            (
                "an element with no text rendering",
                vec![child(None, Some("a")), child(None, None)],
                None,
            ),
        ];
        for (case, children, expected) in sequences {
            let actual = rendered_sequence(&children);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        let structures: [(&str, Vec<RenderedChild>, Option<&str>); 5] = [
            ("no members present", vec![], Some("Thing()")),
            (
                "one member present",
                vec![child(Some("name"), Some("n"))],
                Some("Thing(name=n)"),
            ),
            (
                "two members in the order they were written",
                vec![child(Some("name"), Some("n")), child(Some("size"), Some("2"))],
                Some("Thing(name=n, size=2)"),
            ),
            (
                "a member with no text rendering suppresses the whole structure",
                vec![child(Some("name"), Some("n")), child(Some("body"), None)],
                None,
            ),
            (
                "a child with no member name at all",
                vec![child(None, Some("n"))],
                None,
            ),
        ];
        for (case, children, expected) in structures {
            let actual = rendered_structure("Thing", &children);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: what a composite may hold is bounded by BYTES as well as by child count, and
    /// a rendering past [`MAX_RESTORED_ECHO`] is refused before it is built — so neither
    /// the text this walk retains nor the text it allocates is sized by the request.
    #[test]
    fn a_composite_is_bounded_by_the_bytes_its_children_hold() {
        let child = |rendering: &str| RenderedChild {
            name: Some("name".to_string()),
            rendering: Some(rendering.to_string()),
        };

        // One child at the bound is held; the next one tips the composite over it, and a
        // truncated composite renders as nothing rather than as a part of the value.
        let mut composite = Composite::default();
        composite.take(child(&"a".repeat(MAX_RESTORED_ECHO - "name".len())));
        assert!(
            !composite.truncated && composite.children.len() == 1,
            "case a child at the byte bound: expected it held, got {composite:?}"
        );
        composite.take(child("b"));
        assert!(
            composite.truncated && composite.children.len() == 1,
            "case a child past the byte bound: expected the composite truncated with the \
             child dropped, got {composite:?}"
        );

        // A child count past the cap is the other bound, unchanged.
        let mut counted = Composite::default();
        for _ in 0..=MAX_RENDERED_CHILDREN {
            counted.take(child("a"));
        }
        assert!(
            counted.truncated && counted.children.len() == MAX_RENDERED_CHILDREN,
            "case more children than the cap: expected the composite truncated at the cap, \
             got {} children",
            counted.children.len()
        );

        // And a rendering that would exceed the bound is refused by both renderers, which
        // measure it before joining anything.
        let oversized = vec![
            child(&"a".repeat(MAX_RESTORED_ECHO)),
            child(&"b".repeat(MAX_RESTORED_ECHO)),
        ];
        let sequence = rendered_sequence(&oversized);
        assert_eq!(
            sequence, None,
            "case an oversized list: expected no rendering, got {sequence:?}"
        );
        let structure = rendered_structure("Thing", &oversized);
        assert_eq!(
            structure, None,
            "case an oversized structure: expected no rendering, got {structure:?}"
        );
    }

    /// RULE: a clause's echo is `Absent` for a member the request never sent, `Rendered`
    /// for a position this walk rendered, and `Unrendered` for a value that was present
    /// with no text rendering, or past the stated bounds.
    #[test]
    fn an_echo_distinguishes_absent_from_present_but_unrenderable() {
        let too_long = "x".repeat(MAX_RESTORED_ECHO + 1);
        let renderings: Vec<(String, Option<String>)> = [
            ("name", Some("a-name")),
            ("body", None),
            ("tags", Some("[a, b]")),
        ]
        .into_iter()
        .map(|(position, rendering)| {
            (
                position.to_string(),
                rendering.map(|text: &str| text.to_string()),
            )
        })
        .chain(std::iter::once((
            "huge".to_string(),
            Some(too_long.clone()),
        )))
        .collect();
        let mut by_path: HashMap<&str, Option<&str>> = HashMap::new();
        for (position, rendering) in &renderings {
            by_path.insert(position.as_str(), rendering.as_deref());
        }

        let rows: [(&str, &str, ViolationKind, Option<&str>, SubmittedEcho); 6] = [
            (
                "an absent member",
                "name",
                ViolationKind::MissingRequiredMember,
                None,
                SubmittedEcho::Absent,
            ),
            (
                "a rendered string",
                "name",
                ViolationKind::MaxLength(1),
                Some("a-name"),
                SubmittedEcho::Rendered("a-name".to_string()),
            ),
            (
                "a list rendered from its elements",
                "tags",
                ViolationKind::MaxLength(1),
                Some("2 items"),
                SubmittedEcho::Rendered("[a, b]".to_string()),
            ),
            (
                "a value with no text rendering",
                "body",
                ViolationKind::MaxLength(1),
                Some("4000 bytes"),
                SubmittedEcho::Unrendered,
            ),
            (
                "a position this walk recorded nothing for",
                "elsewhere",
                ViolationKind::MaxLength(1),
                Some("whatever"),
                SubmittedEcho::Unrendered,
            ),
            (
                "a rendering past the stated bound",
                "huge",
                ViolationKind::MaxLength(1),
                Some("whatever"),
                SubmittedEcho::Unrendered,
            ),
        ];
        for (case, path, kind, value, expected) in rows {
            let violation = Violation {
                path: path.to_string(),
                kind,
                value: value.map(str::to_string),
            };
            let mut budget = MAX_RESTORED_TOTAL;
            let actual = echo_of(&violation, &by_path, &mut budget);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the two renderings drop the SAME pair ([`super::unanchored`]'s positional
    /// rule, which the test above pins) and differ in exactly one thing —
    /// [`super::unanchored_grouping_alternation`] leaves a top-level ALTERNATION grouped,
    /// so the expression it quotes still matches what the model's anchored text matched,
    /// while [`super::unanchored`] quotes it bare. Nothing else is grouped: an alternation
    /// already inside a group is not wrapped again, and an expression whose pair was not
    /// dropped is stated verbatim by both.
    ///
    /// The rows are the model patterns this rule meets in the tree plus the regex
    /// constructs that can hide an anchor or an alternation — an escape, a character class,
    /// a group — and the half-anchored forms neither rendering may touch.
    #[test]
    fn only_the_grouping_rendering_wraps_a_top_level_alternation() {
        // (case, the model's pattern, what `unanchored` states, what
        // `unanchored_grouping_alternation` states)
        let rows: [(&str, &str, &str, &str); 15] = [
            ("a plain whole-value pattern", "^[0-9]{12}$", "[0-9]{12}", "[0-9]{12}"),
            (
                "an alternation already inside a group",
                "^(a|b)$",
                "(a|b)",
                "(a|b)",
            ),
            // The shape of this service family's `SessionIdentifier`: an ARN branch or a
            // bare uuid branch, sharing one outer pair.
            (
                "a top-level alternation of two groups",
                "^(arn:[^:]+:x)|([a-f0-9]{8})$",
                "(arn:[^:]+:x)|([a-f0-9]{8})",
                "((arn:[^:]+:x)|([a-f0-9]{8}))",
            ),
            (
                "a bare top-level alternation",
                "^a|b$",
                "a|b",
                "(a|b)",
            ),
            // Branches that carry an anchor of their own while SHARING the outer pair: the
            // pair is dropped positionally (see the test above), and the group this
            // rendering adds around what is left changes nothing about what it matches.
            (
                "branches anchored individually, sharing the outer pair",
                "^a$|^b$",
                "a$|^b",
                "(a$|^b)",
            ),
            // Branches parenthesised, so there is no outer pair to drop and nothing to
            // group.
            (
                "branches anchored individually inside their own groups",
                "(^a$)|(^b$)",
                "(^a$)|(^b$)",
                "(^a$)|(^b$)",
            ),
            ("anchored at the start only", "^abc", "^abc", "^abc"),
            ("anchored at the end only", "abc$", "abc$", "abc$"),
            ("no anchors at all", "[0-9]+", "[0-9]+", "[0-9]+"),
            (
                "a trailing escaped dollar, which is a literal",
                r"^cost\$$",
                r"cost\$",
                r"cost\$",
            ),
            (
                "an expression that IS a literal dollar",
                r"^\$$",
                r"\$",
                r"\$",
            ),
            (
                "a pipe inside a character class, which is a literal",
                "^[a|b]{2}$",
                "[a|b]{2}",
                "[a|b]{2}",
            ),
            (
                "a caret inside a negated class, which is not an anchor",
                "^[^:]+$",
                "[^:]+",
                "[^:]+",
            ),
            // A `]` in the first position of a class is the literal bracket, so the `|`
            // after it is still inside the class and alternates nothing.
            (
                "a literal bracket opening a class, with a pipe inside it",
                "^[]|]a$",
                "[]|]a",
                "[]|]a",
            ),
            (
                "an escaped bracket inside a class, with a pipe inside it",
                r"^[\]|]a$",
                r"[\]|]a",
                r"[\]|]a",
            ),
        ];

        for (case, pattern, plain, grouped) in rows {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, plain,
                "case {case:?}: expected unanchored {plain:?}, got {actual:?}"
            );
            let actual = unanchored_grouping_alternation(pattern);
            assert_eq!(
                actual, grouped,
                "case {case:?}: expected unanchored_grouping_alternation {grouped:?}, \
                 got {actual:?}"
            );
        }
    }
}


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
//! # What of a validation message this module now owns, and what is still to come
//!
//! Three parts of a validation-exception message are the same for EVERY service, and two
//! of them live here:
//!
//! * which value a clause echoes — the one the caller submitted ([`with_submitted_values`]),
//!   and, for a clause stated at a collection member, the collection itself
//!   ([`submitted_lists`]);
//! * a `@pattern` printed as the expression a value has to satisfy, without the `^`/`$`
//!   anchors a model writes to demand a whole-string match ([`unanchored`]) — LANDED here,
//!   while ~26 service crates still carry their own inlined copy
//!   (`grep -rln 'fn unanchored' services/*/src`), each pinned by that service's own
//!   recordings and tests; deleting those copies in favour of this one is the migration's
//!   own change, and nothing new should inline the rule again. The copies that are not
//!   wired to this one still decline to drop the pair around a top-level alternation, which
//!   the recordings show AWS dropping ([`unanchored`] explains why); that difference goes
//!   away as each is deleted.
//!
//! Two parts still belong here and are not here yet:
//!
//! * the requirement half, read off the framework's own wording from a violation with NO
//!   value attached and split at the LAST clause marker, so neither the rejected value
//!   nor a caller-chosen map key in the member path can reach it;
//! * the subject half: a service that reports a present-but-invalid value as
//!   `Value at '<path>' …` without quoting it back, while an ABSENT member keeps
//!   `Value null at '<path>' …`.
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
                },
                Echoes::Rendered => Refusing {
                    violations,
                    lists: HashMap::new(),
                },
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
    /// Reaching the cap also means a leaf the walk has not got to will be dropped, so it
    /// records that the collection is incomplete.
    fn done(&mut self) -> bool {
        let done = self.leaves.len() >= MAX_RECORDED_LEAVES;
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
        is_elision_of, list_of_element, unanchored, Submitted, SubmittedValues,
        MAX_RECORDED_BYTES, MAX_RECORDED_LEAVES, MAX_RESTORED_ECHO, MAX_RESTORED_TOTAL,
    };
    use aws_smithy_schema_server::constraints::{Violation, ViolationKind};

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
    /// there, so a leaf beyond it is a leaf that was dropped.
    #[test]
    fn the_leaf_cap_marks_the_collection_incomplete() {
        let mut walk = SubmittedValues::default();
        walk.path.push("member".to_string());
        assert!(
            !walk.done() && !walk.skipped,
            "case an empty walk: expected it to be complete and not done"
        );
        for index in 0..MAX_RECORDED_LEAVES {
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
        };
        let () = COHERENT;
    }
}


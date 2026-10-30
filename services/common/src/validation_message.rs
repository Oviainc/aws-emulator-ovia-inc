// SPDX-License-Identifier: CC0-1.0

//! How a service states the framework's boundary refusal of a request.
//!
//! The framework hands a service the refusal STRUCTURALLY — `Refusal::Invalid` holding
//! one [`Violation`] per constraint the request broke — and lets the service choose the
//! words, because no model records them. [`aws_validation_refusal`] is the wording AWS
//! itself uses, so a service only has to delegate to it.
//!
//! One thing AWS words differently from the framework is how a `@pattern` requirement
//! is quoted, and that rendering is NOT implemented here: it is
//! [`crate::validation::unanchored`], this crate's staged home for it, which every service
//! carrying its own inlined copy is to migrate onto. This module only decides WHICH
//! violations get restated and in what envelope. The envelope itself, the member paths and
//! every other requirement are the framework's, so nothing about them can drift per crate.
//!
//! A service whose recordings show the other of the two renderings that home offers
//! ([`crate::validation::unanchored_grouping_alternation`], which keeps a top-level
//! alternation grouped) delegates to [`aws_validation_refusal_with`] and passes it: the
//! rendering is the only thing a service chooses here.
//!
//! The rendering rule is framework-level — it belongs in
//! `vendor/smithy-schema-server/src/constraints.rs`'s `Violation::requirement`, which a fix
//! may not change from here; once it learns this, delete this module and every
//! `override_refusal` that delegates to it.
//!
//! One refusal does NOT come from that envelope at all: a member modelled as a MAP is
//! validated against the operation's input as a JSON OBJECT, so a map over its `@length`
//! maximum is a schema finding about the object's property count rather than a clause
//! about a member's length. [`property_count_refusal`] states that one, for a service
//! whose recordings show it, and is asked before the envelope.
//!
//! [`too_many_properties`], [`pattern_mismatch`], [`string_too_long`],
//! [`string_too_short`] and [`enum_value_not_found`] are the SENTENCES that layer states
//! for the constraint classes it reports on a member it read from the request BODY. A service whose front door words
//! those findings itself — because it has to tell a body member from one bound to the URI,
//! which the refusal seam cannot — builds them from here, so the sentences do not drift
//! per crate. [`expected_vendor`] is the one sentence here that is not a schema finding:
//! how the resource-tagging layer refuses an ARN of another vendor, which it decides
//! before any lookup.
//!
//! The other difference this module owns is the VALUE half of a clause: a COLLECTION
//! (list or map) whose `@length` was broken is stated with no value at all —
//! `Value at '<member path>' failed to satisfy constraint: …` — where a scalar states the
//! value it submitted. There is no scalar rendering of a list or a map, so none is
//! invented.
//!
//! The same half is dropped for a member whose value the refusal WITHHOLDS: a member
//! modelled `@sensitive` is one the service never discloses, so its clause names the
//! member and the requirement and states no value — neither the submitted bytes nor the
//! `null` the framework writes for an absent one. WHICH members those are is a property of
//! each service's model, so the caller decides it and passes the decision to
//! [`aws_validation_refusal_withholding`]; the clause, the envelope and every other
//! requirement stay this module's and the framework's.
//!
//! One constraint class reaches no refusal seam at all: a map's KEY, which the walk treats
//! as a path segment rather than as a constrained leaf, so nothing is reported about it and
//! the HANDLER applies the key shape's own constraints. [`map_key_refusal`] is the message
//! it states — the framework's envelope and clause, over the map member, listing what a key
//! has to satisfy — so a request refused for its key reads like one refused for its value.
//!
//! The last difference this module owns is WHERE a constraint broken by a collection's
//! ELEMENTS is stated. The framework reports such a violation at the offending element —
//! `<collection>.<1-based position>.member`, echoing that element and naming only the
//! constraint it broke — while AWS states it once at the COLLECTION MEMBER, with the
//! element shape's WHOLE declared constraint set nested inside
//! `Member must satisfy constraint: [ … ]` and no value at all.
//! [`aws_validation_refusal_over_elements`] is that rendering, for a service whose
//! recordings show it; the set is read off the MODEL (the schemas the caller hands over),
//! because the refusal carries only the constraints that failed.

use std::collections::HashSet;

use aws_smithy_schema::{Schema, ShapeId, TraitMap};
use aws_smithy_schema_server::constraints::{
    self, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait,
};
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use smithy_server_runtime::{Refusal, Refused};

use crate::validation::{list_of_element, unanchored};

/// The two suffixes the validating walk renders a COLLECTION's size with when it
/// length-checks one: a list as `<n> items`, a map as `<n> entries`
/// (`aws_smithy_schema_server::constraints`'s collection-length check).
const COLLECTION_SIZE_SUFFIXES: [&str; 2] = [" items", " entries"];

/// The size `stored` states when it is the walk's rendering of a collection ending in
/// `suffix`, and `None` for anything else — so a clause whose echo is a caller's own
/// string is never read as a size.
///
/// The rendering is the ONLY signal available here: [`ViolationKind`] carries the
/// constraint that was broken and not the KIND of shape that broke it
/// (`vendor/smithy-schema-server/src/constraints.rs`), and the module header above states
/// why that crate cannot be changed from here. The accepted collision is a STRING member
/// whose submitted value is literally `<digits> items`/`<digits> entries` and whose
/// `@length` bound that string breaks: such a refusal is stated as a collection's. No
/// emulated member admits that today (the shortest `@length` maximum is far above the
/// seven characters the shortest such string takes), and the alternative — reading the
/// shape kind — needs the frozen crate.
fn collection_size(stored: &str, suffix: &str) -> Option<usize> {
    stored.strip_suffix(suffix)?.parse::<usize>().ok()
}

/// How many entries the framework's rendering of a MAP member states, when `stored` is
/// one. `None` for a list's rendering and for a caller's own string.
///
/// `pub` because a service whose front door words its OWN schema finding (rather than
/// delegating the whole refusal to [`property_count_refusal`]) needs the same reading of
/// the same rendering: two services cannot disagree about how many entries one map held.
#[must_use]
pub fn map_entry_count(stored: &str) -> Option<usize> {
    collection_size(stored, " entries")
}

/// How many items the framework's rendering of a LIST member states, when `stored` is one.
/// `None` for a map's rendering and for a caller's own string.
///
/// The mirror of [`map_entry_count`], and `pub` for the same reason: a service that words a
/// list's own `@length` refusal itself — because the sentence its recordings show names the
/// cap the SERVICE enforces rather than the clause the framework renders — reads the size out
/// of the same rendering, so no two services disagree about how many items one list held.
#[must_use]
pub fn list_item_count(stored: &str) -> Option<usize> {
    collection_size(stored, " items")
}

/// The clause a COLLECTION member's `@length` violation states — the framework's own,
/// with the `Value '<…>'` half reduced to `Value` — or `None` when `violation` is not one.
///
/// A list or a map has no scalar rendering, so the clause names the member and the
/// requirement and nothing else; the requirement half, the path and the envelope stay the
/// framework's, so only the value half can differ.
fn collection_length_clause(violation: &Violation) -> Option<String> {
    if !matches!(
        violation.kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    ) {
        return None;
    }
    let stored = violation.value.as_deref()?;
    COLLECTION_SIZE_SUFFIXES
        .iter()
        .find_map(|suffix| collection_size(stored, suffix))?;
    valueless_clause(violation)
}

/// The clause `violation` states with its VALUE half dropped — `Value at '<member path>'
/// failed to satisfy constraint: <requirement>` — whichever half the framework wrote: the
/// submitted value a present member echoes, or the `null` an ABSENT member is stated with.
///
/// Built by removing that half from the framework's own clause rather than by spelling a
/// clause here, so the requirement, the member path and the punctuation around them stay
/// single-homed in the framework and cannot drift. `None` when the framework's clause does
/// not start with the value half this would remove.
fn valueless_clause(violation: &Violation) -> Option<String> {
    let value_half = match violation.value.as_deref() {
        Some(stored) => format!("Value '{stored}' "),
        // How the framework writes a member that was not there at all.
        None => "Value null ".to_string(),
    };
    let requirement_half = violation.message().strip_prefix(&value_half)?.to_string();
    Some(format!("Value {requirement_half}"))
}

/// The sentence a JSON-schema layer states for an object carrying more properties than
/// the schema admits — the count the request submitted and the maximum the model
/// declares, and nothing else.
#[must_use]
pub fn too_many_properties(found: usize, max: i64) -> String {
    format!("object has too many properties (found {found} but schema requires at most {max})")
}

/// The finding that layer states for a string outside the regular expression its schema
/// declares: the expression as the MODEL writes it (anchors included — this is the schema's
/// own expression, not the per-member clause's rendering that [`unanchored`] owns) and the
/// string the caller submitted.
///
/// `services/service-com-amazonaws-account-account/src/service.rs` and
/// `services/service-com-amazonaws-oam-oamservice/src/service.rs` still carry their own
/// inlined copies of this sentence, written before this module had it; they are to be
/// migrated onto this one rather than copied again.
#[must_use]
pub fn pattern_mismatch(pattern: &str, value: &str) -> String {
    format!("ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"")
}

/// The finding that layer states for a string longer than the schema's maximum: the string
/// the caller submitted, its length, and that maximum.
///
/// `length` is the length the CONSTRAINT counted (`@length` counts characters), passed in
/// rather than measured here, so the number the finding states is the one that was checked.
/// The same two services as [`pattern_mismatch`] carry their own copies of this sentence.
#[must_use]
pub fn string_too_long(value: &str, length: usize, max: i64) -> String {
    format!("string \"{value}\" is too long (length: {length}, maximum allowed: {max})")
}

/// The finding that layer states for a string shorter than the schema's minimum: the
/// string the caller submitted, its length, and that minimum.
///
/// The mirror of [`string_too_long`], down to where the number comes from: `length` is
/// the length the CONSTRAINT counted, passed in rather than measured here.
#[must_use]
pub fn string_too_short(value: &str, length: usize, min: i64) -> String {
    format!("string \"{value}\" is too short (length: {length}, required minimum: {min})")
}

/// The finding that layer states for a value outside the set its schema enumerates: the
/// value the caller submitted, then every value the schema admits, each quoted and
/// separated by a bare comma.
///
/// `permitted` is stated in the order it is handed over — the order the MODEL declares the
/// enumerated type's members in, which is the order the validating walk carries — so the
/// finding lists the type as its definition reads. An emulated service that validates a
/// DOCUMENT it was handed (rather than a modelled member) states the same sentence about
/// the values that document's format defines;
/// `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/state.rs`
/// carries its own copy for that layer, written before this module had the sentence, and
/// is to be migrated onto this one rather than copied again.
#[must_use]
pub fn enum_value_not_found<S: AsRef<str>>(value: &str, permitted: &[S]) -> String {
    let quoted: Vec<String> = permitted
        .iter()
        .map(|admitted| format!("\"{}\"", admitted.as_ref()))
        .collect();
    format!(
        "instance value (\"{value}\") not found in enum (possible values: [{}])",
        quoted.join(",")
    )
}

/// The findings of one refusal as that layer LISTS them: `[<finding>, <finding>]`.
///
/// A JSON-schema layer reports what it found as a list, so one finding and five render
/// the same way. What a service puts in FRONT of the list is its own (the recordings do
/// not even agree on the punctuation before it), but the list itself is one rendering and
/// lives here.
#[must_use]
pub fn bracketed_findings(findings: &[String]) -> String {
    format!("[{}]", findings.join(", "))
}

/// The message a service states for a MAP whose KEY broke one of the constraints the
/// map's KEY SHAPE declares — the one constraint class no walk ahead of a handler
/// reports, because that walk treats a map's key as a path segment rather than as a
/// constrained leaf, so the handler applies the key's constraints and words the refusal
/// with this.
///
/// It is the framework's own envelope and clause, over the map MEMBER: the key's
/// requirements are listed as what a key has to satisfy, each sentence the framework's
/// (so it cannot drift from the one a reported member gets), and the clause states no
/// value — a map has no scalar rendering to quote, which is the same half
/// [`collection_length_clause`] drops for a map's own `@length`.
///
/// `min_length` and `max_length` are the key shape's declared `@length` bounds and
/// `pattern` its declared `@pattern`, quoted as the caller hands it over: a service
/// whose refusals render an expression AWS's way passes it through
/// [`crate::validation::unanchored`] first, one that keeps the framework's rendering
/// passes the model's own.
#[must_use]
pub fn map_key_refusal(
    member_path: &str,
    min_length: i64,
    max_length: i64,
    pattern: &str,
) -> String {
    let requirements = [
        requirement(ViolationKind::MaxLength(max_length)),
        requirement(ViolationKind::MinLength(min_length)),
        requirement(ViolationKind::Pattern(pattern.to_string())),
    ];
    let clause = format!(
        "Value at '{member_path}' failed to satisfy constraint: Map keys must satisfy \
         constraint: {}",
        bracketed_findings(&requirements)
    );
    // The ENVELOPE is the framework's too: one placeholder violation about the same
    // member renders it, and that violation's own clause is then swapped for the one
    // about the key — the same substitution [`stated_refusal`] makes on a clause whose
    // value half it drops. `envelope` answers `None` only for no violation at all.
    let placeholder = Violation {
        path: member_path.to_string(),
        kind: ViolationKind::MissingRequiredMember,
        value: None,
    };
    match envelope(std::slice::from_ref(&placeholder)) {
        Some(rendered) => rendered.replace(&placeholder.message(), &clause),
        None => clause,
    }
}

/// The REQUIREMENT half of a violation's clause — the `Member must …` sentence — taken
/// out of the framework's own clause rather than spelled here, so the requirements a
/// handler lists and the ones a reported member gets are the same sentences.
fn requirement(kind: ViolationKind) -> String {
    /// How the framework separates a clause's member half from its requirement.
    const CLAUSE_SEPARATOR: &str = "failed to satisfy constraint: ";

    let clause = Violation {
        path: String::new(),
        kind,
        value: None,
    }
    .message();
    match clause.split_once(CLAUSE_SEPARATOR) {
        Some((_, requirement)) => requirement.to_string(),
        None => clause,
    }
}

/// The sentence the resource-tagging layer in front of a service refuses a caller-supplied
/// `resourceArn` of ANOTHER vendor with: it names the vendor it expected — the serving
/// service's own ARN namespace — and nothing of the ARN it was handed.
///
/// Not a schema finding like the three above: the layer refuses the ARN before it looks any
/// resource up (an ARN of another vendor can name none of this service's resources however
/// the account changes), so it states what it expected rather than what the value failed.
/// `services/service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs`
/// still carries its own inlined copy of this sentence; it is to be migrated onto this
/// one rather than copied again.
#[must_use]
pub fn expected_vendor(vendor: &str) -> String {
    format!("Invalid input resource arn: expected vendor {vendor}")
}

/// [`too_many_properties`] for one violation, or `None` when the violation is not a MAP
/// member over its `@length` maximum.
fn property_count_clause(violation: &Violation) -> Option<String> {
    let ViolationKind::MaxLength(max) = &violation.kind else {
        return None;
    };
    let found = map_entry_count(violation.value.as_deref()?)?;
    Some(too_many_properties(found, *max))
}

/// State `refusal` the way the JSON-SCHEMA layer in front of an AWS REST service states
/// an object with too many properties, or `None` when no member of the request broke a
/// map's `@length` maximum.
///
/// A member modelled as a MAP is validated against the operation's input as a JSON
/// OBJECT, so a map carrying more entries than the model's maximum is a schema failure
/// about the object's property count — reported as the bracketed LIST of schema
/// findings, `[<finding>, …]` — and not one of the per-member
/// `N validation error(s) detected: …` clauses the framework renders, which are what a
/// scalar member outside its own constraint produces. A service whose recordings show
/// that layer delegates its `Service::override_refusal` here FIRST and falls through to
/// the per-member envelope ([`aws_validation_refusal`]) when this answers `None`.
///
/// A request that breaks a map's maximum AND another member's constraint is stated as the
/// schema findings ALONE: the schema layer answers before the per-member walk is reported,
/// so its findings are the whole message rather than clauses mixed into the other
/// envelope.
///
/// The code travels with the message, as everywhere else in this module: `None` would
/// leave the framework's generic member-length clause, which is the text this exists to
/// replace.
pub fn property_count_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let clauses: Vec<String> = violations.iter().filter_map(property_count_clause).collect();
    if clauses.is_empty() {
        return None;
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: bracketed_findings(&clauses),
    })
}

/// The error code AWS answers a constraint refusal with on the JSON protocols, and the
/// code the framework's own default picks there — supplied so a chosen message always
/// travels with its matching code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// State `refusal` the way AWS states it, or `None` to keep the framework's own wording.
///
/// This is the whole `Service::override_refusal` body for a service on a JSON protocol
/// (restJson1 or the awsJson family), where AWS answers a constraint refusal as
/// `400 ValidationException` with the envelope
/// `N validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy
/// constraint: <requirement>` — the envelope the framework already renders, except for two
/// renderings:
///
/// * AWS quotes a `@pattern` requirement without the anchors a whole-value pattern wraps
///   it in ([`crate::validation::unanchored`]). The check itself still uses the model's
///   pattern as written: only the RENDERING drops them.
/// * a COLLECTION (list or map) that broke its `@length` is stated with no value at all,
///   `Value at '<member path>' failed to satisfy constraint: <requirement>`, because a
///   list and a map have no scalar rendering to quote.
///
/// Every other requirement, and the member path, are the framework's, so the two cannot
/// drift.
///
/// `None` for anything that is not a constraint refusal (an unauthenticated caller keeps
/// the framework's 403 code and wording), and `None` when there is nothing to state.
///
/// A service on a query protocol must not wire this in unchanged: the framework answers
/// a boundary refusal there as `MalformedQueryString`, so forcing the JSON code would
/// change what a client branches on.
pub fn aws_validation_refusal(refusal: &Refusal) -> Option<Refused> {
    aws_validation_refusal_with(refusal, unanchored)
}

/// [`aws_validation_refusal`], with the members whose value the refusal WITHHOLDS chosen by
/// the caller: `withholds_value` is asked about a violation's member path, and a clause it
/// answers `true` for states no value at all.
///
/// A service passes the members its own model marks `@sensitive`, which the runtime schemas
/// do not carry. Everything else — which refusals are restated, the code, the envelope, the
/// member paths, the `@pattern` rendering and every other requirement — is the same body
/// [`aws_validation_refusal`] runs, so the two cannot drift.
pub fn aws_validation_refusal_withholding(
    refusal: &Refusal,
    withholds_value: impl Fn(&str) -> bool,
) -> Option<Refused> {
    stated_refusal(refusal, unanchored, withholds_value)
}

/// [`aws_validation_refusal_withholding`], with the `@pattern` rendering chosen by the
/// caller too — the one entry point that takes BOTH of the choices a service makes here.
///
/// A service needs it when its recordings show a `@pattern` requirement quoted other than
/// [`unanchored`] — as the model writes it, anchors included, is one such rendering, which
/// `|pattern: &str| pattern.to_string()` states — AND its model marks members `@sensitive`.
/// Everything neither choice covers is the same [`stated_refusal`] body every other entry
/// point here runs, so no combination of the two can drift into a second envelope.
pub fn aws_validation_refusal_withholding_with(
    refusal: &Refusal,
    quote_pattern: impl Fn(&str) -> String,
    withholds_value: impl Fn(&str) -> bool,
) -> Option<Refused> {
    stated_refusal(refusal, quote_pattern, withholds_value)
}

/// [`aws_validation_refusal`], with the `@pattern` rendering CHOSEN by the caller.
///
/// Everything a service does not choose — which refusals are restated, which violations are
/// touched, the code, the envelope, the member paths and every other requirement — is this
/// one body and the framework's, so the two renderings cannot drift into two envelopes.
/// `quote_pattern` is one of [`crate::validation`]'s two (`unanchored`, or
/// `unanchored_grouping_alternation` for a service whose recordings show a top-level
/// alternation grouped); the CHECK always used the model's pattern as written, so this only
/// changes how the expression is quoted back.
pub fn aws_validation_refusal_with(
    refusal: &Refusal,
    quote_pattern: impl Fn(&str) -> String,
) -> Option<Refused> {
    // Nothing is withheld unless a service says so: the model trait that decides it is not
    // in the runtime schemas, so a caller that does not pass the decision gets the
    // framework's value half on every clause, exactly as before.
    stated_refusal(refusal, quote_pattern, |_| false)
}

/// The ONE body behind every entry point above: the framework's envelope over the
/// framework's clauses, with the two renderings this module owns applied — the `@pattern`
/// quoting the caller chose, and the value half dropped from a clause that states none (a
/// collection's `@length`, or a member the caller withholds the value of).
///
/// LINEAR in the number of violations: each clause is decided BEFORE the envelope is
/// rendered, so no clause is rewritten inside an already-rendered message (which would
/// rescan and reallocate the whole message once per violation), and the count the envelope
/// states is bounded by [`restated_violations`].
fn stated_refusal(
    refusal: &Refusal,
    quote_pattern: impl Fn(&str) -> String,
    withholds_value: impl Fn(&str) -> bool,
) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // The envelope and every clause are the framework's; a COLLECTION's length clause
    // then drops its value half, which the framework has no way to leave out (a clause
    // carrying no value reads `Value null at …`, which is how an ABSENT member is stated).
    let clauses: Vec<String> = restated_violations(violations, &quote_pattern)
        .iter()
        .map(|violation| {
            let stated = if withholds_value(&violation.path) {
                valueless_clause(violation)
            } else {
                collection_length_clause(violation)
            };
            stated.unwrap_or_else(|| violation.message())
        })
        .collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: envelope_over(&clauses)?,
    })
}

/// The violations one message restates, each with its `@pattern` requoted.
///
/// BOUNDED by the same ceiling the framework's own walk collects under
/// ([`crate::validation::MAX_VIOLATIONS`]), so the clause count — and with it the length of
/// the message and the work of rendering it — is a constant of this crate rather than
/// something one request chooses. The walk never hands over more than that many, so the
/// bound only states locally what is already true of every caller.
fn restated_violations(
    violations: &[Violation],
    quote_pattern: &dyn Fn(&str) -> String,
) -> Vec<Violation> {
    violations
        .iter()
        .take(crate::validation::MAX_VIOLATIONS)
        .map(|violation| requoted(violation, quote_pattern))
        .collect()
}

/// What a clause opens the element shape's constraint set with, when the set is stated at
/// the collection member the elements belong to.
const ELEMENT_SET_OPEN: &str = "Member must satisfy constraint: [";

/// State `refusal` as AWS states it for a service that reports a constraint broken by a
/// collection's ELEMENTS at the COLLECTION MEMBER, or `None` to keep the framework's own
/// wording.
///
/// This is [`aws_validation_refusal_with`] plus one grouping rule:
///
/// * a violation at a list ELEMENT (`<collection>.<position>.member`,
///   [`crate::validation::list_of_element`]) is stated at `<collection>` — no position and
///   no `member` step — with every requirement the ELEMENT shape declares nested inside
///   `Member must satisfy constraint: [ \u2026 ]`, and with no value: a collection has no scalar
///   rendering to quote, which is the same half a collection's own `@length` clause drops.
///   The set is the element shape's whole set, not just the constraint that happened to
///   fail, so one request breaking two of an element's constraints is ONE validation error
///   about one member; two offending elements of one collection are one clause too.
/// * a violation on the collection ITSELF (its `@length`, its `@uniqueItems`), or on a
///   member inside a structure element, keeps its own clause and its own path.
///
/// `input_schemas` are the operation input shapes of the calling service, which the element
/// sets are read off: codegen attaches a collection member's element constraints to that
/// member's schema as an [`ElementConstraintsTrait`], and the refusal carries only the
/// constraints that FAILED, so a set read off the violations would be missing the ones the
/// value satisfied. Only the TOP-LEVEL members of those shapes are looked up, and a member
/// path two of them declare with DIFFERENT element sets keeps the framework's own clause:
/// the refusal does not say which operation it came from, and a guessed set would not be
/// known to be true of the request.
///
/// `quote_pattern` chooses the `@pattern` rendering exactly as it does for
/// [`aws_validation_refusal_with`] — `str::to_string` for a service whose refusals quote
/// the expression the model declares, [`crate::validation::unanchored`] for one whose
/// recordings drop the anchors. The CHECK always used the model's pattern as written.
///
/// THE HOME for this grouping, under the same rule as [`pattern_mismatch`] and
/// [`enum_value_not_found`]: the service crates below each carry their own inlined copy of
/// it, written before this module had one, and are to be MIGRATED onto this entry point
/// rather than copied again (`grep -rln 'Member must satisfy constraint:' services/*/src`
/// lists them): opensearch-amazonopensearchservice,
/// bedrock-amazonbedrockcontrolplaneservice,
/// bedrockagentcorecontrol-amazonbedrockagentcorecontrol,
/// lambdacore-lambdacoreapiservice, rum-rum, identitystore-awsidentitystore,
/// acm-certificatemanager, codeguruprofiler-codeguruprofiler,
/// billingconductor-awsbillingconductor,
/// licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions, ssmsap-ssmsap and
/// workspacesinstances-eucmifrontendapiservice. Each migration is that service's own
/// change, because each has to be re-checked against its own recordings: they do not agree
/// on the value half of the clause or on how the `@pattern` is quoted.
pub fn aws_validation_refusal_over_elements(
    refusal: &Refusal,
    input_schemas: &[&'static Schema],
    quote_pattern: impl Fn(&str) -> String,
) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let quote = &quote_pattern as &dyn Fn(&str) -> String;
    let mut clauses: Vec<String> = Vec::new();
    // Dedupe through a set rather than by scanning what is already collected, so the cost
    // stays linear in the clause count.
    let mut stated: HashSet<String> = HashSet::new();
    for violation in &restated_violations(violations, quote) {
        let clause = list_of_element(&violation.path)
            .and_then(|collection| element_set_clause(collection, input_schemas, quote))
            .or_else(|| collection_length_clause(violation))
            .unwrap_or_else(|| violation.message());
        // An identical clause is stated once: the member is named once however many of
        // its elements the walk reported.
        if stated.insert(clause.clone()) {
            clauses.push(clause);
        }
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: envelope_over(&clauses)?,
    })
}

/// `violation` with its `@pattern` requirement quoted the way the caller chose, and
/// everything else untouched.
///
/// The ONE place that mapping is made, so the two entry points cannot drift in how an
/// expression is quoted back.
fn requoted(violation: &Violation, quote_pattern: &dyn Fn(&str) -> String) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(quote_pattern(pattern)),
            value: violation.value.clone(),
        },
        _ => violation.clone(),
    }
}

/// The clause stated at `collection` for a constraint its elements broke, or `None` when
/// the handed-over schemas declare no single element constraint set for that member.
fn element_set_clause(
    collection: &str,
    input_schemas: &[&'static Schema],
    quote_pattern: &dyn Fn(&str) -> String,
) -> Option<String> {
    let mut declared = input_schemas
        .iter()
        .filter_map(|schema| element_requirements(schema, collection, quote_pattern));
    let set = declared.next()?;
    if declared.any(|other| other != set) {
        return None;
    }
    Some(format!(
        "Value at '{collection}' failed to satisfy constraint: {ELEMENT_SET_OPEN}{}]",
        set.join(", ")
    ))
}

/// Every requirement the ELEMENT shape of `input_schema`'s `collection` member declares, in
/// the order a clause states them, or `None` when that member is not a collection of
/// constrained scalars.
///
/// The ORDER is the one AWS's element-constraint clauses state: the `@length` maximum, then
/// its minimum, then the numeric bounds the same way, then the `@pattern`, then an
/// enumerated value set. A `@length` with no declared minimum states `0`, the smallest
/// length a Smithy `@length` can bound.
///
/// Every sentence is the framework's own ([`requirement`]), so a requirement nested into a
/// collection clause reads exactly like one a reported member gets.
fn element_requirements(
    input_schema: &Schema,
    collection: &str,
    quote_pattern: &dyn Fn(&str) -> String,
) -> Option<Vec<String>> {
    let member = input_schema.members().iter().find(|member| {
        member
            .member_name()
            .is_some_and(|name| constraints::path::member(name) == collection)
    })?;
    let traits = constraint_trait::<ElementConstraintsTrait>(
        member.traits()?,
        &ElementConstraintsTrait::TRAIT_ID,
    )?
    .inner();

    let mut requirements: Vec<String> = Vec::new();
    if let Some(length) = constraint_trait::<LengthTrait>(traits, &LengthTrait::TRAIT_ID) {
        if let Some(max) = length.max {
            requirements.push(requirement(ViolationKind::MaxLength(max)));
        }
        requirements.push(requirement(ViolationKind::MinLength(
            length.min.unwrap_or(0),
        )));
    }
    if let Some(range) = constraint_trait::<RangeTrait>(traits, &RangeTrait::TRAIT_ID) {
        if let Some(max) = range.max {
            requirements.push(requirement(ViolationKind::MaxValue(rendered_bound(max))));
        }
        if let Some(min) = range.min {
            requirements.push(requirement(ViolationKind::MinValue(rendered_bound(min))));
        }
    }
    if let Some(pattern) = constraint_trait::<PatternTrait>(traits, &PatternTrait::TRAIT_ID) {
        requirements.push(requirement(ViolationKind::Pattern(quote_pattern(
            pattern.regex,
        ))));
    }
    if let Some(allowed) = constraint_trait::<EnumTrait>(traits, &EnumTrait::TRAIT_ID) {
        let values: Vec<String> = allowed.values.iter().map(|value| (*value).to_owned()).collect();
        requirements.push(requirement(ViolationKind::EnumValue(values)));
    }
    (!requirements.is_empty()).then_some(requirements)
}

/// One constraint trait off a trait map, downcast to the type carrying its bounds.
fn constraint_trait<'map, T: 'static>(traits: &'map TraitMap, id: &ShapeId) -> Option<&'map T> {
    traits.get(id)?.as_any().downcast_ref::<T>()
}

/// A `@range` bound as a requirement displays it: an integral bound without a trailing
/// `.0`, a fractional one with its decimals — the rendering the framework's own clause
/// carries, so a bound read off the model and one read off a violation agree.
///
/// `{bound:.0}` rather than a cast through `i64`, which SATURATES: a bound past `i64`'s
/// magnitude would otherwise advertise `i64::MAX` instead of what the model declares.
fn rendered_bound(bound: f64) -> String {
    if bound.fract() == 0.0 && bound.is_finite() {
        format!("{bound:.0}")
    } else {
        format!("{bound}")
    }
}

/// The framework's envelope over clauses this module stated rather than over violations.
///
/// The count word and the separator are the framework's: they are read back out of
/// [`envelope`]'s own rendering over as many placeholder violations as there are clauses,
/// so neither is spelled here and a grouped refusal cannot drift from an ungrouped one.
/// `None` for no clause at all, exactly as [`envelope`] answers for no violation.
fn envelope_over(clauses: &[String]) -> Option<String> {
    let placeholder = Violation {
        path: String::new(),
        kind: ViolationKind::MissingRequiredMember,
        value: None,
    };
    let rendered = envelope(&vec![placeholder.clone(); clauses.len()])?;
    let stated = placeholder.message();
    let (prefix, rest) = rendered.split_once(&stated)?;
    // With every placeholder identical, whatever sits between the first two is the
    // separator; one clause needs none.
    let separator = rest.split_once(&stated).map_or("", |(between, _)| between);
    Some(format!("{prefix}{}", clauses.join(separator)))
}

#[cfg(test)]
mod tests {
    use super::{
        aws_validation_refusal, aws_validation_refusal_over_elements, aws_validation_refusal_with,
        aws_validation_refusal_withholding, aws_validation_refusal_withholding_with,
        enum_value_not_found, envelope_over, map_key_refusal,
        pattern_mismatch, property_count_refusal, string_too_long, string_too_short,
    };
    use crate::validation::unanchored_grouping_alternation;
    use aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
    use aws_smithy_schema_server::constraints::{
        ElementConstraintsTrait, LengthTrait, PatternTrait,
    };
    use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
    use smithy_server_runtime::Refusal;
    use std::sync::LazyLock;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: &str) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some(value.to_owned()),
        }
    }

    /// RULE: a refusal about a map's KEY is the framework's envelope over a clause that
    /// names the map MEMBER, states no value (a map has no scalar rendering) and lists
    /// what a key has to satisfy, in the framework's own requirement sentences.
    ///
    /// The rows differ along the member path and along the bounds and pattern the key
    /// shape declares, so nothing of one caller's shape is baked into the rendering.
    #[test]
    fn a_map_key_refusal_states_what_a_key_must_satisfy() {
        // (case, member path, minimum, maximum, pattern, the message stated)
        let rows: [(&str, &str, i64, i64, &str, &str); 2] = [
            (
                "a top-level map member",
                "parameters",
                1,
                192,
                "^[.$a-zA-Z0-9_-]+$",
                "1 validation error detected: Value at 'parameters' failed to satisfy \
                 constraint: Map keys must satisfy constraint: [Member must have length less \
                 than or equal to 192, Member must have length greater than or equal to 1, \
                 Member must satisfy regular expression pattern: ^[.$a-zA-Z0-9_-]+$]",
            ),
            (
                "a map nested under a list element",
                "entries.2.member.tags",
                0,
                128,
                "[a-z]+",
                "1 validation error detected: Value at 'entries.2.member.tags' failed to \
                 satisfy constraint: Map keys must satisfy constraint: [Member must have \
                 length less than or equal to 128, Member must have length greater than or \
                 equal to 0, Member must satisfy regular expression pattern: [a-z]+]",
            ),
        ];

        for (case, member_path, min, max, pattern, expected) in rows {
            let stated = map_key_refusal(member_path, min, max, pattern);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    #[test]
    fn a_constraint_refusal_is_stated_as_aws_states_it() {
        let refusal = Refusal::Invalid(vec![violation(
            "identifiers.1.member.featureGroupName",
            ViolationKind::Pattern(r"^([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$".to_owned()),
            "not a valid name!!",
        )]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.code, "ValidationException",
            "expected the JSON protocols' own refusal code, got {:?}",
            stated.code
        );
        assert_eq!(
            stated.message,
            "1 validation error detected: Value 'not a valid name!!' at \
             'identifiers.1.member.featureGroupName' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: ([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})",
            "expected the AWS envelope with the pattern quoted unanchored, got {:?}",
            stated.message
        );
    }

    /// RULE: a COLLECTION member that broke its `@length` is stated with NO value —
    /// `Value at '<path>' failed to satisfy constraint: <requirement>` — while a SCALAR
    /// member keeps the value it submitted.
    #[test]
    fn a_collection_length_clause_states_no_value_and_a_scalar_one_does() {
        // (case, path, the broken bound, the echo the validating walk rendered, the clause
        // the refusal must state). The echoes are the framework's own: `<n> items` for a
        // list, `<n> entries` for a map, and the caller's own bytes for a string.
        let rows: [(&str, &str, ViolationKind, &str, &str); 8] = [
            (
                "a list one element over its maximum",
                "queries",
                ViolationKind::MaxLength(1),
                "2 items",
                "Value at 'queries' failed to satisfy constraint: Member must have length less \
                 than or equal to 1",
            ),
            (
                "a list far over a much larger maximum",
                "sources",
                ViolationKind::MaxLength(1000),
                "4096 items",
                "Value at 'sources' failed to satisfy constraint: Member must have length less \
                 than or equal to 1000",
            ),
            (
                "an empty list under its minimum",
                "sources",
                ViolationKind::MinLength(1),
                "0 items",
                "Value at 'sources' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
            (
                "a nested list, reported under its element path",
                "entries.1.member.values",
                ViolationKind::MaxLength(5),
                "6 items",
                "Value at 'entries.1.member.values' failed to satisfy constraint: Member must \
                 have length less than or equal to 5",
            ),
            (
                "a map over its maximum",
                "tags",
                ViolationKind::MaxLength(200),
                "201 entries",
                "Value at 'tags' failed to satisfy constraint: Member must have length less than \
                 or equal to 200",
            ),
            (
                "an empty map under its minimum",
                "tags",
                ViolationKind::MinLength(1),
                "0 entries",
                "Value at 'tags' failed to satisfy constraint: Member must have length greater \
                 than or equal to 1",
            ),
            (
                "a string over its maximum keeps the value it submitted",
                "name",
                ViolationKind::MaxLength(3),
                "abcd",
                "Value 'abcd' at 'name' failed to satisfy constraint: Member must have length \
                 less than or equal to 3",
            ),
            (
                "an empty string under its minimum keeps the value it submitted",
                "nextToken",
                ViolationKind::MinLength(1),
                "",
                "Value '' at 'nextToken' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
        ];

        for (case, path, kind, stored, clause) in rows {
            let refusal = Refusal::Invalid(vec![violation(path, kind, stored)]);
            for (entry_point, stated) in [
                ("the default rendering", aws_validation_refusal(&refusal)),
                (
                    "the grouping rendering",
                    aws_validation_refusal_with(&refusal, unanchored_grouping_alternation),
                ),
            ] {
                let stated = stated.expect("a constraint refusal is stated");
                assert_eq!(
                    stated.code, "ValidationException",
                    "case {case:?} under {entry_point}: expected the JSON protocols' refusal \
                     code, got {:?}",
                    stated.code
                );
                let expected = format!("1 validation error detected: {clause}");
                assert_eq!(
                    stated.message, expected,
                    "case {case:?} under {entry_point}: expected {expected:?}, got {:?}",
                    stated.message
                );
            }
        }
    }

    /// RULE, over a refusal carrying SEVERAL clauses: each clause is stated by its own
    /// shape's rule — the collection's without a value, the scalars' with one — inside the
    /// framework's counting envelope.
    #[test]
    fn an_envelope_states_each_clause_by_its_own_shapes_rule() {
        let refusal = Refusal::Invalid(vec![
            violation("queries", ViolationKind::MaxLength(1), "2 items"),
            violation("tags", ViolationKind::MinLength(1), "0 entries"),
            violation("maxResults", ViolationKind::MinValue("1".to_owned()), "0"),
        ]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        let expected = "3 validation errors detected: Value at 'queries' failed to satisfy \
                        constraint: Member must have length less than or equal to 1; Value at \
                        'tags' failed to satisfy constraint: Member must have length greater \
                        than or equal to 1; Value '0' at 'maxResults' failed to satisfy \
                        constraint: Member must have value greater than or equal to 1";
        assert_eq!(
            stated.message, expected,
            "expected {expected:?}, got {:?}",
            stated.message
        );
    }

    /// RULE: only a `@length` violation of a collection loses its value — a collection's
    /// OTHER constraint classes, and a member whose value the walk could not render,
    /// keep the framework's clause.
    #[test]
    fn nothing_but_a_collection_length_clause_loses_its_value() {
        // (case, the violation, the clause the refusal must state)
        let rows: [(&str, Violation, &str); 3] = [
            (
                "a list with a repeated element",
                violation("values", ViolationKind::UniqueItems, "duplicate"),
                "Value 'duplicate' at 'values' failed to satisfy constraint: Member must have \
                 unique values",
            ),
            (
                "an absent required member",
                Violation {
                    path: "queries".to_owned(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                },
                "Value null at 'queries' failed to satisfy constraint: Member must not be null",
            ),
            (
                "an absent member carrying a length bound",
                Violation {
                    path: "sources".to_owned(),
                    kind: ViolationKind::MinLength(1),
                    value: None,
                },
                "Value null at 'sources' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
        ];

        for (case, violation, clause) in rows {
            let stated = aws_validation_refusal(&Refusal::Invalid(vec![violation]))
                .expect("a constraint refusal is stated");
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE: a member the caller WITHHOLDS the value of is stated with no value half at
    /// all, whichever half the framework wrote and whatever the constraint was — and the
    /// members it does not withhold keep theirs, in the same envelope.
    #[test]
    fn a_withheld_members_clause_states_no_value() {
        // (case, the violation, the clause the refusal must state for the withheld member)
        let rows: [(&str, Violation, &str); 4] = [
            (
                "a withheld member that was absent",
                Violation {
                    path: "secret".to_owned(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                },
                "Value at 'secret' failed to satisfy constraint: Member must not be null",
            ),
            (
                "a withheld member shorter than its minimum",
                violation("secret", ViolationKind::MinLength(4), "ab"),
                "Value at 'secret' failed to satisfy constraint: Member must have length \
                 greater than or equal to 4",
            ),
            (
                "a withheld member longer than its maximum",
                violation("secret", ViolationKind::MaxLength(8), "abcdefghij"),
                "Value at 'secret' failed to satisfy constraint: Member must have length less \
                 than or equal to 8",
            ),
            (
                "a withheld member outside its pattern",
                violation(
                    "secret",
                    ViolationKind::Pattern("^[a-z]+$".to_owned()),
                    "A B",
                ),
                "Value at 'secret' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: [a-z]+",
            ),
        ];

        for (case, offending, clause) in rows {
            let refusal = Refusal::Invalid(vec![offending]);
            let stated = aws_validation_refusal_withholding(&refusal, |path| path == "secret")
                .expect("a constraint refusal is stated");
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
            // The same refusal, with nothing withheld, keeps the framework's value half:
            // the suppression is the caller's decision and nothing else about the clause.
            let kept = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
            assert_ne!(
                kept.message, expected,
                "case {case:?}: withholding nothing must keep the value half, got {:?}",
                kept.message
            );
        }

        // One envelope over both kinds of member: only the withheld one loses its value.
        let refusal = Refusal::Invalid(vec![
            violation("secret", ViolationKind::MinLength(4), "ab"),
            violation("name", ViolationKind::MinLength(2), "a"),
        ]);
        let stated = aws_validation_refusal_withholding(&refusal, |path| path == "secret")
            .expect("a constraint refusal is stated");
        let expected = "2 validation errors detected: Value at 'secret' failed to satisfy \
                        constraint: Member must have length greater than or equal to 4; Value \
                        'a' at 'name' failed to satisfy constraint: Member must have length \
                        greater than or equal to 2";
        assert_eq!(
            stated.message, expected,
            "expected {expected:?}, got {:?}",
            stated.message
        );
    }

    /// RULE: the entry point that takes BOTH choices applies both — the caller's
    /// `@pattern` rendering and its withholding — over the one envelope every other entry
    /// point here renders.
    ///
    /// The rows differ in which of the two choices each clause exercises: a withheld
    /// member outside its pattern loses its value half AND keeps the expression as the
    /// caller quotes it, while a member neither choice touches keeps both halves.
    #[test]
    fn both_choices_apply_over_the_one_envelope() {
        // The rendering a service whose recordings quote the model's expression verbatim
        // passes, anchors included.
        let as_written = |pattern: &str| pattern.to_owned();
        // (case, the violations, the message stated)
        let rows: [(&str, Vec<Violation>, &str); 3] = [
            (
                "a withheld member outside its pattern",
                vec![violation(
                    "secret",
                    ViolationKind::Pattern("^[a-z]+$".to_owned()),
                    "A B",
                )],
                "1 validation error detected: Value at 'secret' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: ^[a-z]+$",
            ),
            (
                "a member neither choice touches",
                vec![violation(
                    "name",
                    ViolationKind::Pattern("^[a-z]+$".to_owned()),
                    "A B",
                )],
                "1 validation error detected: Value 'A B' at 'name' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: ^[a-z]+$",
            ),
            (
                "both in one envelope",
                vec![
                    violation("secret", ViolationKind::MinLength(4), "ab"),
                    violation(
                        "name",
                        ViolationKind::Pattern("^[a-z]+$".to_owned()),
                        "A B",
                    ),
                ],
                "2 validation errors detected: Value at 'secret' failed to satisfy constraint: \
                 Member must have length greater than or equal to 4; Value 'A B' at 'name' \
                 failed to satisfy constraint: Member must satisfy regular expression pattern: \
                 ^[a-z]+$",
            ),
        ];

        for (case, violations, expected) in rows {
            let stated = aws_validation_refusal_withholding_with(
                &Refusal::Invalid(violations),
                as_written,
                |path| path == "secret",
            )
            .expect("a constraint refusal is stated");
            assert_eq!(
                stated.code, "ValidationException",
                "case {case:?}: expected the JSON protocols' own refusal code, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    #[test]
    fn a_pattern_requirement_is_quoted_by_the_one_renderer_this_crate_owns() {
        // Each row is (case, the model's pattern, the expression the clause must state) —
        // exactly what `crate::validation::unanchored` renders, which is the single
        // implementation of that rule and is table-tested in its own module. These rows
        // only pin that this restatement goes THROUGH it: an outer pair is dropped, and
        // anything else (a lone anchor, an alternation's own interior anchors) stays
        // verbatim.
        let rows: [(&str, &str, &str); 5] = [
            (
                "a wrapping pair",
                r"^[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}$",
                r"[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}",
            ),
            ("a leading anchor only", r"^arn:aws.*", r"^arn:aws.*"),
            ("a trailing anchor only", r"[0-9]{12}$", r"[0-9]{12}$"),
            (
                "an alternation whose branches carry the anchors",
                r"(^a$)|(^b$)",
                r"(^a$)|(^b$)",
            ),
            // The row this one previously stated as unchanged: without the parentheses the
            // branches SHARE the outer pair, which the rule drops positionally.
            (
                "an alternation sharing the outer pair",
                r"^a$|^b$",
                r"a$|^b",
            ),
        ];
        for (case, pattern, expected) in rows {
            let refusal = Refusal::Invalid(vec![violation(
                "name",
                ViolationKind::Pattern(pattern.to_owned()),
                "x",
            )]);
            let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
            let reported = stated
                .message
                .rsplit("regular expression pattern: ")
                .next()
                .unwrap_or_default();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {pattern:?} to be stated as {expected:?}, got \
                 {reported:?}"
            );
        }
    }

    #[test]
    fn a_requirement_that_is_not_a_pattern_keeps_the_frameworks_wording() {
        let refusal = Refusal::Invalid(vec![violation(
            "maxResults",
            ViolationKind::MaxValue("100".to_owned()),
            "1000",
        )]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert!(
            stated.message.contains("at 'maxResults'")
                && stated
                    .message
                    .contains("Member must have value less than or equal to 100")
                && !stated.message.contains("regular expression pattern"),
            "expected the framework's own numeric-bound requirement, got {:?}",
            stated.message
        );
    }

    /// RULE: the chosen rendering is the ONLY thing that differs between the two entry
    /// points — same code, same envelope, same clause around a differently quoted
    /// expression — and a caller that passes the grouping rendering gets it.
    #[test]
    fn the_chosen_pattern_rendering_is_the_only_difference_between_the_two_entry_points() {
        // (case, the model's pattern, what the default states, what the grouping rendering
        // states). The expressions are `crate::validation`'s own table rows: a wrapping
        // pair, and a pair the branches of a top-level alternation share.
        let rows: [(&str, &str, &str, &str); 3] = [
            (
                "a wrapping pair, where the two renderings agree",
                r"^[0-9]{12}$",
                r"[0-9]{12}",
                r"[0-9]{12}",
            ),
            (
                "a top-level alternation of two groups",
                r"^(arn:[^:]+:x)|([a-f0-9]{8})$",
                r"(arn:[^:]+:x)|([a-f0-9]{8})",
                r"((arn:[^:]+:x)|([a-f0-9]{8}))",
            ),
            (
                "a bare top-level alternation",
                r"^a|b$",
                r"a|b",
                r"(a|b)",
            ),
        ];
        for (case, pattern, plain, grouped) in rows {
            let refusal = Refusal::Invalid(vec![violation(
                "name",
                ViolationKind::Pattern(pattern.to_owned()),
                "x",
            )]);
            for (rendering, expected, stated) in [
                ("the default", plain, aws_validation_refusal(&refusal)),
                (
                    "the grouping rendering",
                    grouped,
                    aws_validation_refusal_with(&refusal, unanchored_grouping_alternation),
                ),
            ] {
                let stated = stated.expect("a constraint refusal is stated");
                assert_eq!(
                    stated.code, "ValidationException",
                    "case {case:?} under {rendering}: expected the JSON protocols' refusal \
                     code, got {:?}",
                    stated.code
                );
                let one = format!(
                    "1 validation error detected: Value 'x' at 'name' failed to satisfy \
                     constraint: Member must satisfy regular expression pattern: {expected}"
                );
                assert_eq!(
                    stated.message, one,
                    "case {case:?} under {rendering}: expected {one:?}, got {:?}",
                    stated.message
                );
            }
        }
    }

    #[test]
    fn a_refusal_that_is_not_a_constraint_keeps_the_frameworks_wording() {
        assert!(
            aws_validation_refusal(&Refusal::Unauthenticated).is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
        assert!(
            aws_validation_refusal(&Refusal::Invalid(Vec::new())).is_none(),
            "expected no violations to state nothing"
        );
        assert!(
            aws_validation_refusal_with(&Refusal::Unauthenticated, unanchored_grouping_alternation)
                .is_none(),
            "expected the chosen rendering not to change which refusals are stated"
        );
    }

    /// RULE: the two string findings state the value the caller submitted as it was
    /// submitted — the expression and the string for a pattern, the string and both
    /// lengths for a maximum — with nothing of the member or the envelope around them.
    ///
    /// The rows differ along what the sentences generalise over: the form of the
    /// expression, and the value's length, charset and content (including one carrying the
    /// quotes and braces the sentence itself uses).
    #[test]
    fn the_string_findings_state_the_submitted_value_and_the_schemas_own_bound() {
        // (case, the expression, the value, the finding expected)
        let patterns: [(&str, &str, &str, &str); 3] = [
            (
                "an anchored expression",
                r"^[\.\-_A-Za-z0-9]+$",
                "bad@name",
                r#"ECMA 262 regex "^[\.\-_A-Za-z0-9]+$" does not match input string "bad@name""#,
            ),
            (
                "an alternation",
                r"^smk://host:[0-9]{1,5}|arn:.+$",
                "https://host/queue",
                r#"ECMA 262 regex "^smk://host:[0-9]{1,5}|arn:.+$" does not match input string "https://host/queue""#,
            ),
            (
                "a value carrying the sentence's own punctuation",
                r"^[a-z]+$",
                r#"{"a": "b"}"#,
                r#"ECMA 262 regex "^[a-z]+$" does not match input string "{"a": "b"}""#,
            ),
        ];
        for (case, pattern, value, expected) in patterns {
            let actual = pattern_mismatch(pattern, value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // (case, the value, the length the constraint counted, the maximum)
        let lengths: [(&str, &str, usize, i64); 3] = [
            ("one character past the maximum", "xxx", 3, 2),
            // `@length` counts CHARACTERS, so the length is the one that was checked and
            // not the byte count of the value.
            ("a unicode value", "ééé", 3, 2),
            ("a maximum of zero", "x", 1, 0),
        ];
        for (case, value, length, max) in lengths {
            let expected =
                format!(r#"string "{value}" is too long (length: {length}, maximum allowed: {max})"#);
            let actual = string_too_long(value, length, max);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // The same rows against the MINIMUM bound: (case, the value, the length the
        // constraint counted, the minimum).
        let minimums: [(&str, &str, usize, i64); 3] = [
            ("one character short of the minimum", "x", 1, 2),
            ("the empty string", "", 0, 1),
            ("a unicode value", "é", 1, 2),
        ];
        for (case, value, length, min) in minimums {
            let expected = format!(
                r#"string "{value}" is too short (length: {length}, required minimum: {min})"#
            );
            let actual = string_too_short(value, length, min);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the enumerated-set finding states the value the caller submitted and every
    /// value the schema admits, in the order they were declared, with nothing of the member
    /// or the envelope around them.
    ///
    /// The rows differ along what the sentence generalises over: how many values the set
    /// holds, their order, and the form of the value that missed it (another spelling, a
    /// different case, the empty string).
    #[test]
    fn the_enumerated_set_finding_states_the_submitted_value_and_every_admitted_one() {
        // (case, the value, the values the schema admits, the finding expected)
        let rows: [(&str, &str, &[&str], &str); 5] = [
            (
                "a value of no admitted spelling",
                "NOTAVALIDTYPE",
                &["IPV4", "DUALSTACK"],
                r#"instance value ("NOTAVALIDTYPE") not found in enum (possible values: ["IPV4","DUALSTACK"])"#,
            ),
            (
                "an admitted value in another case",
                "ipv4",
                &["IPV4", "DUALSTACK"],
                r#"instance value ("ipv4") not found in enum (possible values: ["IPV4","DUALSTACK"])"#,
            ),
            (
                "a set of three, in its declared order",
                "MAYBE",
                &["ATLEAST", "AND", "OR"],
                r#"instance value ("MAYBE") not found in enum (possible values: ["ATLEAST","AND","OR"])"#,
            ),
            (
                "a set of one",
                "ARCHIVE",
                &["EXPIRE"],
                r#"instance value ("ARCHIVE") not found in enum (possible values: ["EXPIRE"])"#,
            ),
            (
                "the empty string",
                "",
                &["IPV4", "DUALSTACK"],
                r#"instance value ("") not found in enum (possible values: ["IPV4","DUALSTACK"])"#,
            ),
        ];
        for (case, value, permitted, expected) in rows {
            let actual = enum_value_not_found(value, permitted);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a MAP member over its `@length` maximum is stated as the schema layer's
    /// property-count finding, built from the entry count the request carried and the
    /// maximum the model declares — and nothing else is read as one.
    #[test]
    fn a_map_over_its_length_maximum_is_stated_as_a_property_count_finding() {
        // (case, the stored echo the walk rendered, the `@length` maximum, the whole
        // message expected, or `None` to keep the per-member envelope). The echoes are
        // the framework's own renderings: `<n> entries` for a map, `<n> items` for a
        // list, and the caller's own bytes for a string.
        let rows: [(&str, &str, i64, Option<&str>); 7] = [
            (
                "one entry past the maximum",
                "201 entries",
                200,
                Some("[object has too many properties (found 201 but schema requires at most 200)]"),
            ),
            (
                "far past a much smaller maximum",
                "4096 entries",
                50,
                Some("[object has too many properties (found 4096 but schema requires at most 50)]"),
            ),
            (
                "one entry past a maximum of zero",
                "1 entries",
                0,
                Some("[object has too many properties (found 1 but schema requires at most 0)]"),
            ),
            ("a list, not an object", "7 items", 5, None),
            ("a string the caller sent", "entries", 3, None),
            ("a string that only looks like a count", "lots entries", 3, None),
            // The accepted collision `map_entry_count` documents: a string whose value
            // IS the map rendering cannot be told apart from a map here, so it is stated
            // as a property-count finding. No emulated member's `@length` maximum is low
            // enough for a request to reach it.
            (
                "a string whose own value is the map rendering",
                "7 entries",
                3,
                Some("[object has too many properties (found 7 but schema requires at most 3)]"),
            ),
        ];

        for (case, stored, max, expected) in rows {
            let refusal = Refusal::Invalid(vec![violation(
                "tags",
                ViolationKind::MaxLength(max),
                stored,
            )]);
            let stated = property_count_refusal(&refusal);
            match expected {
                Some(message) => {
                    let stated = stated
                        .unwrap_or_else(|| panic!("case {case:?}: expected {message:?}, got none"));
                    assert_eq!(
                        stated.code, "ValidationException",
                        "case {case:?}: expected the JSON protocols' refusal code, got {:?}",
                        stated.code
                    );
                    assert_eq!(
                        stated.message, message,
                        "case {case:?}: expected {message:?}, got {:?}",
                        stated.message
                    );
                }
                None => assert!(
                    stated.is_none(),
                    "case {case:?}: expected the per-member envelope to stand, got {stated:?}"
                ),
            }
        }
    }

    /// RULE: only a length MAXIMUM on a map is that finding — a map under a `@length`
    /// MINIMUM, and a refusal that is no constraint failure at all, are not.
    #[test]
    fn nothing_but_a_map_over_its_maximum_is_a_property_count_finding() {
        assert!(
            property_count_refusal(&Refusal::Invalid(vec![violation(
                "tags",
                ViolationKind::MinLength(1),
                "0 entries",
            )]))
            .is_none(),
            "expected a map UNDER its minimum to keep the per-member envelope"
        );
        assert!(
            property_count_refusal(&Refusal::Unauthenticated).is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
        assert!(
            property_count_refusal(&Refusal::Invalid(Vec::new())).is_none(),
            "expected no violations to state nothing"
        );
    }

    // A collection member carrying its element shape's own constraints, written the way
    // codegen writes one: the element's `@length`/`@pattern` live in an
    // `ElementConstraintsTrait` wrapper on the COLLECTION member's schema, which also
    // carries the collection's own `@length` over the element count.
    static ELEMENT_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut traits = TraitMap::new();
        traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
        traits.insert(Box::new(PatternTrait::new(r"^[0-9]+$")));
        traits
    });
    static MEMBER_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut traits = TraitMap::new();
        traits.insert(Box::new(LengthTrait::new(Some(1), Some(1000))));
        traits.insert(Box::new(ElementConstraintsTrait::new(&ELEMENT_TRAITS)));
        traits
    });
    static COLLECTION_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$Numbers", "test", "Numbers"),
        ShapeType::List,
        "Numbers",
        0,
    )
    .with_traits(&MEMBER_TRAITS);
    static SCALAR_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$Name", "test", "Name"),
        ShapeType::String,
        "Name",
        1,
    );
    static INPUT: Schema = Schema::new_struct(
        ShapeId::from_static("test#Input", "test", "Input"),
        ShapeType::Structure,
        &[&COLLECTION_MEMBER, &SCALAR_MEMBER],
    );

    /// The element shape's whole set, as the clause nests it.
    const ELEMENT_SET: &str = "Member must satisfy constraint: [Member must have length less \
                               than or equal to 128, Member must have length greater than or \
                               equal to 1, Member must satisfy regular expression pattern: \
                               ^[0-9]+$]";

    /// RULE: a constraint broken by a collection's ELEMENTS is stated once at the
    /// collection member — no element position, no `member` step and no value — with the
    /// element shape's whole declared set nested inside `Member must satisfy constraint:
    /// [ … ]`, whichever of those constraints the request actually broke and however many
    /// elements broke them.
    #[test]
    fn an_element_constraint_is_stated_at_the_collection_member() {
        // (case, the violations one request produced, the message expected)
        let rows: Vec<(&str, Vec<Violation>, String)> = vec![
            (
                "one element outside the element shape's pattern",
                vec![violation(
                    "numbers.1.member",
                    ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                    "not-a-number",
                )],
                format!(
                    "1 validation error detected: Value at 'numbers' failed to satisfy \
                     constraint: {ELEMENT_SET}"
                ),
            ),
            (
                "one element breaking a bound the set also declares",
                vec![violation(
                    "numbers.3.member",
                    ViolationKind::MinLength(1),
                    "",
                )],
                format!(
                    "1 validation error detected: Value at 'numbers' failed to satisfy \
                     constraint: {ELEMENT_SET}"
                ),
            ),
            (
                "two elements, each breaking a different one",
                vec![
                    violation(
                        "numbers.1.member",
                        ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                        "x",
                    ),
                    violation("numbers.2.member", ViolationKind::MaxLength(128), "9999"),
                ],
                format!(
                    "1 validation error detected: Value at 'numbers' failed to satisfy \
                     constraint: {ELEMENT_SET}"
                ),
            ),
            (
                // The collection's OWN `@length` is not an element constraint: it keeps
                // its own clause, with the value half a collection has no rendering for
                // dropped.
                "the collection's own length, beside an element's",
                vec![
                    violation("numbers", ViolationKind::MaxLength(1000), "1001 items"),
                    violation(
                        "numbers.9.member",
                        ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                        "x",
                    ),
                ],
                format!(
                    "2 validation errors detected: Value at 'numbers' failed to satisfy \
                     constraint: Member must have length less than or equal to 1000; Value at \
                     'numbers' failed to satisfy constraint: {ELEMENT_SET}"
                ),
            ),
            (
                // A member the schemas declare no element constraints for keeps the
                // framework's own clause, path and echo.
                "a scalar member of the same request",
                vec![violation(
                    "name",
                    ViolationKind::Pattern(r"^[a-z]+$".to_owned()),
                    "NOPE",
                )],
                "1 validation error detected: Value 'NOPE' at 'name' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: ^[a-z]+$"
                    .to_owned(),
            ),
        ];

        for (case, violations, expected) in rows {
            let stated = aws_validation_refusal_over_elements(
                &Refusal::Invalid(violations),
                &[&INPUT],
                str::to_string,
            )
            .unwrap_or_else(|| panic!("case {case:?}: expected {expected:?}, got none"));
            assert_eq!(
                stated.code, "ValidationException",
                "case {case:?}: expected the JSON protocols' refusal code, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE: with no element set to read, nothing is grouped — the refusal is the one
    /// [`aws_validation_refusal_with`] states, clause for clause.
    #[test]
    fn an_element_whose_set_is_unknown_keeps_the_ungrouped_refusal() {
        let refusal = Refusal::Invalid(vec![
            violation(
                "numbers.1.member",
                ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                "x",
            ),
            violation("name", ViolationKind::MinLength(3), "ab"),
        ]);
        let grouped = aws_validation_refusal_over_elements(&refusal, &[], str::to_string)
            .expect("a constraint refusal is stated");
        let ungrouped = aws_validation_refusal_with(&refusal, str::to_string)
            .expect("a constraint refusal is stated");
        assert_eq!(
            grouped, ungrouped,
            "expected the ungrouped refusal, got {grouped:?}"
        );
        assert!(
            aws_validation_refusal_over_elements(&Refusal::Unauthenticated, &[], str::to_string)
                .is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
        assert!(
            aws_validation_refusal_over_elements(
                &Refusal::Invalid(Vec::new()),
                &[&INPUT],
                str::to_string
            )
            .is_none(),
            "expected no violations to state nothing"
        );
    }

    /// RULE: the envelope a grouped refusal is stated in is the framework's own — the same
    /// count word and the same separator it renders over the same number of violations.
    #[test]
    fn the_envelope_is_the_frameworks_own() {
        for count in 1..=4usize {
            let clauses: Vec<String> = (0..count)
                .map(|index| format!("Value at 'member{index}' failed to satisfy constraint: X"))
                .collect();
            let violations: Vec<Violation> = clauses
                .iter()
                .enumerate()
                .map(|(index, _)| Violation {
                    path: format!("member{index}"),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                })
                .collect();
            // The framework's envelope over the same number of clauses, with its own
            // clause text swapped for the stated one so only the envelope is compared.
            let mut expected = envelope(&violations).expect("a violation renders");
            for (index, clause) in clauses.iter().enumerate() {
                expected = expected.replace(&violations[index].message(), clause);
            }
            let actual = envelope_over(&clauses).expect("a clause renders");
            assert_eq!(
                actual, expected,
                "case {count} clauses: expected {expected:?}, got {actual:?}"
            );
        }
        assert!(
            envelope_over(&[]).is_none(),
            "expected no clause to state nothing"
        );
    }
}

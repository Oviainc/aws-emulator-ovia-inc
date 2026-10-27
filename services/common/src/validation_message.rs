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
//! [`too_many_properties`], [`pattern_mismatch`], [`string_too_long`] and
//! [`string_too_short`] are the SENTENCES that layer states for the constraint classes it
//! reports on a member it read from the request BODY. A service whose front door words
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

use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use smithy_server_runtime::{Refusal, Refused};

use crate::validation::unanchored;

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
    let valued = violation.message();
    let requirement_half = valued.strip_prefix(&format!("Value '{stored}' "))?;
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
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let restated: Vec<Violation> = violations
        .iter()
        .map(|violation| match &violation.kind {
            ViolationKind::Pattern(pattern) => Violation {
                path: violation.path.clone(),
                kind: ViolationKind::Pattern(quote_pattern(pattern)),
                value: violation.value.clone(),
            },
            _ => violation.clone(),
        })
        .collect();
    // The envelope and every clause are the framework's; a COLLECTION's length clause
    // then drops its value half, which the framework has no way to leave out (a clause
    // carrying no value reads `Value null at …`, which is how an ABSENT member is stated).
    let mut message = envelope(&restated)?;
    for violation in &restated {
        if let Some(valueless) = collection_length_clause(violation) {
            message = message.replace(&violation.message(), &valueless);
        }
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        aws_validation_refusal, aws_validation_refusal_with, pattern_mismatch,
        property_count_refusal, string_too_long, string_too_short,
    };
    use crate::validation::unanchored_grouping_alternation;
    use aws_smithy_schema_server::{Violation, ViolationKind};
    use smithy_server_runtime::Refusal;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: &str) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some(value.to_owned()),
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
}

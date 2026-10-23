// SPDX-License-Identifier: CC0-1.0

//! How a service states the framework's boundary refusal of a request.
//!
//! The framework hands a service the refusal STRUCTURALLY — `Refusal::Invalid` holding
//! one [`Violation`] per constraint the request broke — and lets the service choose the
//! words, because no model records them. [`aws_validation_refusal`] is the wording AWS
//! itself uses, so a service only has to delegate to it.
//!
//! The ONE thing AWS words differently from the framework is how a `@pattern` requirement
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

use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use smithy_server_runtime::{Refusal, Refused};

use crate::validation::unanchored;

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
/// constraint: <requirement>` — the envelope the framework already renders, except that
/// AWS quotes a `@pattern` requirement without the anchors a whole-value pattern wraps it
/// in ([`crate::validation::unanchored`]). The check itself still uses the model's pattern
/// as written: only the RENDERING drops them. Every other requirement, and the member
/// path, are the framework's, so the two cannot drift.
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
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: envelope(&restated)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{aws_validation_refusal, aws_validation_refusal_with};
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
}

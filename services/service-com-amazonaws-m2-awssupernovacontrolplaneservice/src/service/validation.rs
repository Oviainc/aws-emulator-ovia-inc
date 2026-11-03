// SPDX-License-Identifier: CC0-1.0

//! The WORDS of m2's modelled input-validation refusal: how the constraint violations of
//! one request become the `message` its `ValidationException` carries.
//!
//! [`crate::service`] owns the SEAMS — which refusal is answered where, and which of them
//! this one is reached for — and this module owns the sentence, so the wording rules are in
//! one place rather than spread through the paths that apply them.
//!
//! Almost all of that sentence is the framework's own, and deliberately so. Every violation
//! handed here comes from `aws_smithy_schema_server::constraints`, reached through
//! `service_common::validation::refusing_violations`, which also restores the value a clause
//! echoes to the bytes the caller submitted — a scalar as its own text and a list as
//! `[<element>, …]`, the value m2 states at a list member whose `@length` was broken, where
//! the walk has only the element count. The envelope, the clauses, the member paths and
//! every requirement are then rendered by `aws_smithy_schema_server::envelope`, the one
//! writer of that sentence, so none of it can drift per crate.
//!
//! The ONE thing m2 words for itself is the ORDER of an enumerated type's value set, which
//! this module therefore states by handing that renderer a violation whose value set is
//! re-ordered — not by rewriting its output — through
//! `service_common::validation_message::in_sorted_enum_order`, this emulator's one
//! statement of that ordering. m2 states the permitted values SORTED where the
//! model declares them in its own order (recorded behaviour: corpus
//! `simparity-wire-aws-v1`, fixture `m2-engine-discovery-and-blu-insights` step `task#5`,
//! where `ListEngineVersions` with an `engineType` outside `EngineType` is answered
//! `… Member must satisfy enum value set: [bluage, microfocus]` while
//! `smithy_query(query="shape",shape="EngineType")` declares `microfocus, bluage`). The
//! enumeration is the actionable half of that refusal — it is what a caller reads to pick a
//! value — so its order is part of the answer rather than prose, and the rule is about every
//! enum-typed member of every operation rather than the one member a recording happened to
//! cover. `service_common::validation`'s own header leaves which values an enum's value set
//! lists, and in which order, to the service for exactly this reason.

use aws_smithy_schema_server::constraints::Violation;
#[cfg(test)]
use aws_smithy_schema_server::constraints::ViolationKind;
use smithy_server_runtime::Refused;

/// The wire error code (`__type` / `x-amzn-errortype`) of this service's
/// input-validation refusal: `com.amazonaws.m2#ValidationException`, `@error("client")`
/// with `@httpError(400)` and a required `message`
/// (`smithy_query(query="shape",shape="ValidationException")`), which the model lists as
/// an error of every operation — and the same name a handler-returned `ValidationException`
/// carries, so a client branches on one code whichever layer refused it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How m2 states a set of broken modelled constraints: its `ValidationException`, carrying
/// the framework's envelope over the clauses of the violations as m2 orders their contents.
///
/// Each violation travels through `service_common::validation_message::in_sorted_enum_order`
/// — this emulator's one statement of that ordering: an enum violation's value set sorted by
/// VALUE, a total order over the strings the model declares, and every other violation
/// unchanged. The CLAUSE is still the framework's, so the member path, the echoed value and
/// the requirement's wording stay in the one place that writes them.
///
/// `None` when there is nothing to state, which keeps the framework's own wording rather
/// than answering with an empty message.
pub(super) fn validation_refusal(violations: &[Violation]) -> Option<Refused> {
    let stated: Vec<Violation> = violations
        .iter()
        .map(::service_common::validation_message::in_sorted_enum_order)
        .collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: ::aws_smithy_schema_server::envelope(&stated)?,
    })
}

#[cfg(test)]
mod tests {
    //! The wording rules, driven on violations built by hand. The wire behaviour they add
    //! up to — which operations are refused this way, with which status and code — is
    //! `tests/all/refusal_message_rules.rs`.

    use super::*;

    /// A violation of `kind` on `path`, carrying `value` as the walk would.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(ToString::to_string),
            sensitive: false,
        }
    }

    /// The message of the refusal `violations` are stated as.
    fn message(violations: &[Violation]) -> String {
        validation_refusal(violations)
            .expect("violations to state are stated")
            .message
    }

    /// An enum violation's value set is stated sorted, whatever order it is declared in,
    /// and the rest of the clause is the framework's.
    #[test]
    fn an_enum_value_set_is_stated_sorted() {
        // (case, the values as a model declares them, the set as a clause states them)
        let cases: &[(&str, &[&str], &str)] = &[
            (
                "declared out of order",
                &["microfocus", "bluage"],
                "[bluage, microfocus]",
            ),
            ("declared in order", &["dual", "ipv4"], "[dual, ipv4]"),
            ("one value", &["only"], "[only]"),
            (
                "mixed case, longest common prefix last",
                &[
                    "Succeeded With Warning",
                    "Succeeded",
                    "Cancelling",
                    "Cancelled",
                ],
                "[Cancelled, Cancelling, Succeeded, Succeeded With Warning]",
            ),
        ];
        for (case, declared, expected_set) in cases {
            let declared: Vec<String> = declared.iter().map(ToString::to_string).collect();
            let stated = message(&[violation(
                "engineType",
                ViolationKind::EnumValue(declared),
                Some("nope"),
            )]);
            let expected = format!(
                "1 validation error detected: Value 'nope' at 'engineType' failed to satisfy \
                 constraint: Member must satisfy enum value set: {expected_set}"
            );
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// Every other violation keeps the framework's clause, value and requirement alike.
    #[test]
    fn every_other_violation_keeps_the_frameworks_clause() {
        let cases: &[(&str, Violation)] = &[
            (
                "a list over its length maximum, echoing the list",
                violation("names", ViolationKind::MaxLength(10), Some("[a, b, c]")),
            ),
            (
                "an absent required member",
                violation("applicationId", ViolationKind::MissingRequiredMember, None),
            ),
            (
                "a value outside a pattern",
                violation(
                    "resourceArn",
                    ViolationKind::Pattern("^\\S{1,80}$".to_string()),
                    Some("not an arn"),
                ),
            ),
            (
                "a number over its range maximum",
                violation(
                    "maxResults",
                    ViolationKind::MaxValue("2000".to_string()),
                    Some("2001"),
                ),
            ),
        ];
        for (case, broken) in cases {
            let stated = message(std::slice::from_ref(broken));
            let expected = format!("1 validation error detected: {}", broken.message());
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// Nothing to state is no refusal of this kind at all, rather than an empty message.
    #[test]
    fn no_violations_state_no_refusal() {
        assert!(
            validation_refusal(&[]).is_none(),
            "case no violations: expected no refusal to be worded"
        );
    }

    /// Two broken constraints are the framework's plural envelope over both clauses, with
    /// the enum set in each of them still sorted.
    #[test]
    fn two_violations_are_stated_under_the_plural_envelope() {
        let page = violation("maxResults", ViolationKind::MinValue("1".to_string()), Some("0"));
        let engine = violation(
            "engineType",
            ViolationKind::EnumValue(vec!["microfocus".to_string(), "bluage".to_string()]),
            Some("nope"),
        );
        let stated = message(&[page.clone(), engine]);
        let expected = format!(
            "2 validation errors detected: {}; Value 'nope' at 'engineType' failed to satisfy \
             constraint: Member must satisfy enum value set: [bluage, microfocus]",
            page.message()
        );
        assert_eq!(
            stated, expected,
            "case two violations: expected {expected:?}, got {stated:?}"
        );
    }
}

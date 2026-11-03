// SPDX-License-Identifier: CC0-1.0

//! The members this service refuses OPAQUELY, and the sentence each of them is refused
//! with.
//!
//! # The rule
//!
//! Almost every modelled constraint this service declares is reported to the caller as the
//! constraint envelope AWS builds from the request — `1 validation error detected: Value
//! '<value>' at '<member>' failed to satisfy constraint: <requirement>` — which quotes the
//! value that was refused and the requirement it broke, so a caller can correct the
//! request. A member carrying an OPAQUE TOKEN is the exception: a token is minted by the
//! service, not composed by the caller, so there is nothing for a caller to correct and
//! AWS answers one fixed sentence naming the fault instead — it quotes neither the token
//! that was sent nor the form a usable one has.
//!
//! `CallbackId` is such a member. It is the identifier of one callback operation of a
//! durable execution ("The unique identifier for the callback operation",
//! `smithy_query(query="operation",operation="SendDurableExecutionCallbackSuccess")`),
//! minted by the service and handed to the external system that will complete the
//! callback; it is bound as the `@httpLabel` of all three of
//! `SendDurableExecutionCallbackSuccess`, `SendDurableExecutionCallbackFailure` and
//! `SendDurableExecutionCallbackHeartbeat`, each declaring it `@length(min: 1, max: 1024)`
//! with the base64 `@pattern` a minted token satisfies. A callback id that cannot be used
//! is refused `400 InvalidParameterValueException` whose body is that one sentence —
//! `{"message": "Invalid callback id"}`, and no other member (recorded behaviour: corpus
//! `simparity-wire-aws-v1`, fixture `lambda-durable-execution-list-and-callback-errors`,
//! steps `task#2`, `task#3` and `task#4`, one per callback operation).
//!
//! # Why it is stated here
//!
//! A modelled constraint is checked BEFORE any handler runs, so an unusable token never
//! reaches the operation that would answer for it: the refusal is the framework's, and the
//! only seams that can word it are the ones [`crate::service`] owns — the per-operation
//! wrapper every modelled operation is served behind, and `override_refusal` for a request
//! no wrapped operation saw. Both go through one renderer, so a caller cannot be told the
//! same broken constraint two ways, and this module is the one statement of which members
//! are answered this way.
//!
//! The rule is keyed by the MEMBER rather than by the operation, because being opaque is a
//! property of the token: every operation that binds the member states it alike, and an
//! operation added later that binds it inherits the same refusal.

use smithy_server_runtime::{Refused, Violation};

/// One member this service refuses opaquely.
struct OpaqueMember {
    /// The member path a violation of it carries, as a refusal clause of this service
    /// spells the member — the wire member name, lower-camel, at the top level of the
    /// input.
    path: &'static str,
    /// The error shape the refusal names (`__type` and `x-amzn-errortype`).
    code: &'static str,
    /// The WHOLE message the refusal carries: nothing is appended to it and nothing of the
    /// request is interpolated into it.
    message: &'static str,
}

/// Every member of this model that is refused opaquely, one row each.
const OPAQUELY_REFUSED_MEMBERS: &[OpaqueMember] = &[OpaqueMember {
    // `CallbackId`, bound by the three durable-execution callback operations.
    path: "callbackId",
    code: "InvalidParameterValueException",
    message: "Invalid callback id",
}];

/// How this service states `violations` when any of them broke a constraint on a member it
/// refuses opaquely: that member's own fixed refusal, whole.
///
/// `None` when no violation names such a member, which leaves the refusal to the
/// constraint envelope every other broken constraint of this service is stated with.
///
/// A request that breaks a constraint on an opaque member AND on another member is refused
/// on the opaque one: the token is what the request addressed the resource by, so it is
/// refused whatever else the request carried, and the fixed sentence is the whole answer
/// rather than one clause of a list that would quote the rest.
pub(super) fn opaque_refusal(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        OPAQUELY_REFUSED_MEMBERS
            .iter()
            .find(|member| member.path == violation.path)
            .map(|member| Refused {
                code: member.code.to_string(),
                message: member.message.to_string(),
            })
    })
}

/// Whether `violations` are answered as an opaque refusal, whose body is the fixed message
/// ALONE: AWS's recorded body for one carries no other member — not even the class of its
/// fault, which every other error body of this service names
/// ([`crate::service::fault_class`]). Opaque is opaque: the answer states the fault and
/// nothing else about it.
pub(super) fn names_no_fault_class(violations: &[Violation]) -> bool {
    opaque_refusal(violations).is_some()
}

#[cfg(test)]
mod tests {
    //! The wording rule, driven on violations built by hand. The wire behaviour it adds up
    //! to — which operations answer it, with which status, code and body — is
    //! `tests/all/durable_execution_refusals.rs`.

    use super::*;
    use smithy_server_runtime::ViolationKind;

    /// A violation of `kind` on `path`, carrying `value` as the constraint walk would.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(ToString::to_string),
            sensitive: false,
        }
    }

    /// The model's `CallbackId` pattern, as the member declares it.
    const CALLBACK_ID_PATTERN: &str = "^[A-Za-z0-9+/]+={0,2}$";

    /// Every way a `CallbackId` can break a constraint it declares is stated as the one
    /// fixed refusal, which quotes neither the id nor the requirement.
    #[test]
    fn an_unusable_callback_id_is_refused_opaquely() {
        // One row per constraint the member declares — its pattern, both ends of its
        // `@length`, and its absence — because the refusal is a property of the member
        // rather than of which of its constraints was broken.
        let cases: &[(&str, Violation)] = &[
            (
                "a value outside the token pattern",
                violation(
                    "callbackId",
                    ViolationKind::Pattern(CALLBACK_ID_PATTERN.to_string()),
                    Some("not a token!"),
                ),
            ),
            (
                "a value longer than the maximum",
                violation(
                    "callbackId",
                    ViolationKind::MaxLength(1_024),
                    Some(&"A".repeat(1_100)),
                ),
            ),
            (
                "a value shorter than the minimum",
                violation("callbackId", ViolationKind::MinLength(1), Some("")),
            ),
            (
                "no value at all",
                violation("callbackId", ViolationKind::MissingRequiredMember, None),
            ),
        ];
        for (case, broken) in cases {
            let stated = opaque_refusal(std::slice::from_ref(broken))
                .unwrap_or_else(|| panic!("case {case:?}: expected the opaque refusal"));
            assert_eq!(
                (stated.code.as_str(), stated.message.as_str()),
                ("InvalidParameterValueException", "Invalid callback id"),
                "case {case:?}: expected the fixed callback refusal, got {stated:?}"
            );
        }
    }

    /// A broken constraint on an opaque member is the whole answer, whatever else the same
    /// request broke — in either order, so nothing of the other clause is disclosed.
    #[test]
    fn an_opaque_member_decides_a_refusal_it_shares() {
        let callback = violation(
            "callbackId",
            ViolationKind::Pattern(CALLBACK_ID_PATTERN.to_string()),
            Some("not a token!"),
        );
        let payload = violation("result", ViolationKind::MaxLength(1_048_576), Some("[...]"));
        for (case, violations) in [
            ("the opaque member first", vec![callback.clone(), payload.clone()]),
            ("the opaque member last", vec![payload, callback]),
        ] {
            let stated = opaque_refusal(&violations)
                .unwrap_or_else(|| panic!("case {case:?}: expected the opaque refusal"));
            assert_eq!(
                stated.message, "Invalid callback id",
                "case {case:?}: expected the fixed callback refusal, got {stated:?}"
            );
        }
    }

    /// Every other member keeps the constraint envelope: nothing else of this model is
    /// refused opaquely, so nothing else loses the clause that says what to correct.
    #[test]
    fn every_other_member_keeps_the_constraint_envelope() {
        let cases: &[(&str, Violation)] = &[
            (
                "a function name outside its pattern",
                violation(
                    "functionName",
                    ViolationKind::Pattern("^[a-zA-Z0-9-_]+$".to_string()),
                    Some("bad name!"),
                ),
            ),
            (
                "a page size over its range",
                violation(
                    "maxItems",
                    ViolationKind::MaxValue("1000".to_string()),
                    Some("5000"),
                ),
            ),
            (
                "a checkpoint token outside the same base64 pattern",
                violation(
                    "checkpointToken",
                    ViolationKind::Pattern(CALLBACK_ID_PATTERN.to_string()),
                    Some("not a token!"),
                ),
            ),
            (
                "a member whose name merely ends in the opaque one",
                violation(
                    "notACallbackId",
                    ViolationKind::Pattern(CALLBACK_ID_PATTERN.to_string()),
                    Some("not a token!"),
                ),
            ),
        ];
        for (case, broken) in cases {
            let stated = opaque_refusal(std::slice::from_ref(broken));
            assert!(
                stated.is_none(),
                "case {case:?}: expected the constraint envelope, got {stated:?}"
            );
        }
    }

    /// Nothing to state is no refusal of this kind at all.
    #[test]
    fn no_violations_state_no_refusal() {
        assert!(
            opaque_refusal(&[]).is_none(),
            "case no violations: expected no refusal to be worded"
        );
    }

    /// An opaque refusal is the message alone, and nothing else is: the one answer that
    /// names no fault class is the one this module states.
    #[test]
    fn only_an_opaque_refusal_names_no_fault_class() {
        let cases: &[(&str, Violation, bool)] = &[
            (
                "an unusable callback id",
                violation(
                    "callbackId",
                    ViolationKind::Pattern(CALLBACK_ID_PATTERN.to_string()),
                    Some("not a token!"),
                ),
                true,
            ),
            (
                "a function name outside its pattern",
                violation(
                    "functionName",
                    ViolationKind::Pattern("^[a-zA-Z0-9-_]+$".to_string()),
                    Some("bad name!"),
                ),
                false,
            ),
        ];
        for (case, broken, expected) in cases {
            let actual = names_no_fault_class(std::slice::from_ref(broken));
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected names_no_fault_class {expected}, got {actual}"
            );
        }
    }
}

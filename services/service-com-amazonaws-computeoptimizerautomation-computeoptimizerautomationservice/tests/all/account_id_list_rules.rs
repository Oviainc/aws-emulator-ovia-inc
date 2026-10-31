// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `accountIds` rules BOTH organization-association
//! operations read (`AssociateAccounts` and `DisassociateAccounts`).
//!
//! The rule under test: the list of member accounts an association request names
//! is judged the same way whichever of the paired operations reads it — a list
//! naming none, and one longer than the documented maximum
//! (`state::MAX_ACCOUNT_IDS_PER_REQUEST`, "You can specify up to 50 account
//! IDs"), is refused with the same HTTP status, the same error code and the same
//! message, and a list naming an id more than once acts on that account exactly
//! once, in the order the request first names it.
//!
//! The rows drive BOTH operations with the same list, over the wire, through the
//! harness `automation_rule_lifecycle` owns, and assert the two answers agree:
//! that agreement is the rule, so a row is about the pair and not about either
//! operation on its own (what each one PERSISTS has its own suite,
//! [`crate::account_association`] and [`crate::account_disassociation`]).

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;

use super::account_association::stored_enrollment;
use super::automation_rule_lifecycle::{call, emulator_with_engine, error_code, member};

/// Member account ids, each matching `AccountId`'s modelled `^[0-9]{12}$`, that
/// are not the calling account.
const MEMBER_A: &str = "111111111111";
const MEMBER_B: &str = "222222222222";
const MEMBER_C: &str = "333333333333";

/// `n` distinct member account ids in the modelled `^[0-9]{12}$` form, for the
/// rows that are about the SIZE of the list.
fn member_account_ids(n: usize) -> Vec<String> {
    (0..n)
        .map(|index| format!("{:012}", 700_000_000_000u64 + index as u64))
        .collect()
}

/// The ids `account_ids` names, each once and in the order it first names them —
/// what both operations must act on and report, computed here independently of
/// the service.
///
/// `pub(crate)` so the sibling suites about what an association request NAMES
/// read a list the same way rather than each growing a copy of this.
pub(crate) fn first_named_once(account_ids: &[String]) -> Vec<String> {
    let mut distinct: Vec<String> = Vec::new();
    for account_id in account_ids {
        if !distinct.iter().any(|kept| kept == account_id) {
            distinct.push(account_id.clone());
        }
    }
    distinct
}

/// The `accountIds` a response reports, in the order it reports them.
///
/// `pub(crate)` for the same reason as [`first_named_once`].
pub(crate) fn reported_account_ids(body: &Value) -> Vec<String> {
    member(body, "accountIds")
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The two operations that read an `accountIds` list of this service.
///
/// `pub(crate)` so a sibling suite about a rule BOTH of them follow drives the
/// same pair.
pub(crate) const ASSOCIATION_OPERATIONS: [&str; 2] = ["AssociateAccounts", "DisassociateAccounts"];

/// The wording a refusal of this member carries, as the service reports a
/// constraint the model cannot express: the member's path and the requirement it
/// failed, with no value echoed.
fn too_short_message() -> String {
    "1 validation error detected: Value at 'accountIds' failed to satisfy constraint: Member must \
     have length greater than or equal to 1"
        .to_string()
}

fn too_long_message() -> String {
    format!(
        "1 validation error detected: Value at 'accountIds' failed to satisfy constraint: Member \
         must have length less than or equal to {}",
        state::MAX_ACCOUNT_IDS_PER_REQUEST
    )
}

// ---------------------------------------------------------------------------
// The size of `accountIds`, judged the same by both operations
// ---------------------------------------------------------------------------

/// A list outside the documented size is refused by BOTH operations, with the
/// same status, the same code and the same message — including a list whose
/// entries are not distinct, because the bound is on the list the request sends
/// and not on the accounts it names.
#[tokio::test]
async fn both_association_operations_refuse_the_same_account_id_lists() {
    // (case, the ids the request names, the message both refusals must carry) —
    // each published bound and one past it, plus a list that is over the bound
    // only because of its repeats.
    let cases: Vec<(&str, Vec<String>, String)> = vec![
        ("none", Vec::new(), too_short_message()),
        (
            "one-past-the-maximum",
            member_account_ids(state::MAX_ACCOUNT_IDS_PER_REQUEST + 1),
            too_long_message(),
        ),
        ("sixty", member_account_ids(60), too_long_message()),
        (
            "over-the-maximum-only-with-repeats",
            member_account_ids(state::MAX_ACCOUNT_IDS_PER_REQUEST)
                .into_iter()
                .chain(member_account_ids(5))
                .collect(),
            too_long_message(),
        ),
    ];

    for (case, requested, expected_message) in cases {
        for operation in ASSOCIATION_OPERATIONS {
            // One fresh store per call, so no row is judged against what another
            // left behind.
            let (router, engine) = emulator_with_engine();
            let (status, body) = call(&router, operation, &json!({"accountIds": requested})).await;

            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: a list outside the documented size is a client \
                 error, got {status} and {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}/{operation}: the refusal must be the operation's parameter-value \
                 error, got {body}"
            );
            assert_eq!(
                crate::error_payloads::error_message(&body),
                expected_message,
                "case {case:?}/{operation}: expected {expected_message:?}, got {body}"
            );
            // A refused request acted on nothing, not even a prefix of the list.
            for account_id in requested.iter().take(3) {
                let stored = stored_enrollment(&engine, account_id);
                assert_eq!(
                    stored.organization_rule_mode, None,
                    "case {case:?}/{operation}: a refused request must store nothing for \
                     {account_id}, got {stored:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A repeated id, read the same way by both operations
// ---------------------------------------------------------------------------

/// A list inside the documented size is accepted by BOTH operations and acts on
/// each account it names ONCE, in the order the list first names it: the
/// association reports and stores it once, and the disassociation that follows
/// reports and stores it once too.
#[tokio::test]
async fn both_association_operations_act_on_each_named_account_once() {
    let caller = state::caller_account_id();
    // (case, the ids the request names) — differing in count, in how the repeats
    // fall and in whether the calling account is among them.
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("one", vec![MEMBER_A.to_string()]),
        (
            "two-distinct",
            vec![MEMBER_A.to_string(), MEMBER_B.to_string()],
        ),
        (
            "the-same-id-twice",
            vec![MEMBER_B.to_string(), MEMBER_B.to_string()],
        ),
        (
            "repeats-interleaved",
            vec![
                MEMBER_C.to_string(),
                MEMBER_A.to_string(),
                MEMBER_C.to_string(),
                MEMBER_B.to_string(),
                MEMBER_A.to_string(),
            ],
        ),
        (
            "the-calling-account-repeated",
            vec![caller.clone(), MEMBER_A.to_string(), caller.clone()],
        ),
        (
            "the-documented-maximum",
            member_account_ids(state::MAX_ACCOUNT_IDS_PER_REQUEST),
        ),
        (
            "the-documented-maximum-of-one-repeated-id",
            vec![MEMBER_A.to_string(); state::MAX_ACCOUNT_IDS_PER_REQUEST],
        ),
    ];

    for (case, requested) in cases {
        let (router, engine) = emulator_with_engine();
        let expected = first_named_once(&requested);

        let (status, associated) = call(
            &router,
            "AssociateAccounts",
            &json!({"accountIds": requested}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}/AssociateAccounts: the list is within the documented size, got \
             {status} and {associated}"
        );
        assert_eq!(
            reported_account_ids(&associated),
            expected,
            "case {case:?}/AssociateAccounts: expected each named account once in the order the \
             request first names it, got {associated}"
        );
        for account_id in &expected {
            let stored = stored_enrollment(&engine, account_id);
            assert_eq!(
                stored.organization_rule_mode.as_deref(),
                Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
                "case {case:?}/AssociateAccounts: {account_id} must be stored associated, got \
                 {stored:?}"
            );
        }

        let (status, disassociated) = call(
            &router,
            "DisassociateAccounts",
            &json!({"accountIds": requested}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}/DisassociateAccounts: the list is within the documented size, got \
             {status} and {disassociated}"
        );
        assert_eq!(
            reported_account_ids(&disassociated),
            expected,
            "case {case:?}/DisassociateAccounts: expected each named account once in the order \
             the request first names it, got {disassociated}"
        );
        for account_id in &expected {
            let stored = stored_enrollment(&engine, account_id);
            assert_eq!(
                stored.organization_rule_mode.as_deref(),
                Some(state::ORGANIZATION_RULE_MODE_NONE_ALLOWED),
                "case {case:?}/DisassociateAccounts: {account_id} must be stored as no longer \
                 associated, got {stored:?}"
            );
        }
    }
}

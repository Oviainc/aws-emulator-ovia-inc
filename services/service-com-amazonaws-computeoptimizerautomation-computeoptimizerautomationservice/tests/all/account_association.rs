// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ORGANIZATION ACCOUNT ASSOCIATION (`AssociateAccounts`).
//!
//! The rule under test: an association request stores, for EVERY account id it
//! names, an enrollment configuration whose organization rule mode is
//! `AnyAllowed` ("When you associate a member account, its organization rule mode
//! is automatically set to \"Any allowed\"") and whose enrollment status is the
//! enabled one ("If the member account has not previously enabled the Automation
//! feature, the association process automatically enables it"), and it answers
//! with the accounts the STORE reports as associated. The request is bounded by
//! the documented size of `accountIds` ("Associates one or more member accounts
//! …", "You can specify up to 50 account IDs") and is idempotent under a
//! replayed `clientToken`.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the emulator binary mounts — using the harness `automation_rule_lifecycle`
//! owns.
//!
//! The caller's OWN association is read back over the wire through
//! `GetEnrollmentConfiguration`, which reports the stored configuration of the
//! calling account. A MEMBER account's configuration is reported by
//! `ListAccounts`, which is still a scaffold answering 501 (its own stub issue,
//! `…-computeoptimizerautomation-listaccounts-stub`), so those rows read the
//! store where the handler writes it instead of asserting through a 501.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, member, string_member,
    unenrolled_emulator_with_engine,
};

/// Member account ids, each matching `AccountId`'s modelled `^[0-9]{12}$`, that
/// are not the calling account (`state::caller_account_id`, the emulator's one
/// account and this organization's management account).
const MEMBER_A: &str = "111111111111";
const MEMBER_B: &str = "222222222222";
const MEMBER_C: &str = "333333333333";

/// `n` distinct member account ids in the modelled `^[0-9]{12}$` form, for the
/// rows that are about the SIZE of `accountIds`.
fn member_account_ids(n: usize) -> Vec<String> {
    (0..n).map(|index| format!("{:012}", 100_000_000_000u64 + index as u64)).collect()
}

/// The stored enrollment configuration of `account_id`, read where the handlers
/// write it.
fn stored_enrollment(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
) -> state::EnrollmentConfig {
    let service_state =
        state::service_state(&**engine).expect("the engine bears this service's state");
    let stored = service_state.lock().enrollment(account_id);
    stored
}

/// Put `account_id`'s enrollment configuration in the store at `status`, with
/// `status_reason` and `organization_rule_mode`.
///
/// A test PRECONDITION about an account that is NOT the caller: no operation of
/// this API sets another account's enrollment status, and two of the four
/// `EnrollmentStatus` values ("The Pending and Failed options cannot be used to
/// update the enrollment status of an account") have no wire path at all.
fn seed_enrollment(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    status: &str,
    status_reason: Option<&str>,
    organization_rule_mode: Option<&str>,
) {
    let service_state =
        state::service_state(&**engine).expect("the engine bears this service's state");
    service_state.lock().enrollments.insert(
        account_id.to_string(),
        state::EnrollmentConfig {
            status: status.to_string(),
            status_reason: status_reason.map(str::to_string),
            organization_rule_mode: organization_rule_mode.map(str::to_string),
            last_updated_timestamp: Some(state::now()),
        },
    );
}

/// The `accountIds` an association response reports, sorted, so a case asserts
/// WHICH accounts were associated without pinning an order the model does not
/// give.
fn reported_account_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = member(body, "accountIds")
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// Assert `account_id` is stored associated and enrolled, which is what an
/// association persists.
fn assert_stored_association(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    case: &str,
) {
    let stored = stored_enrollment(engine, account_id);
    assert_eq!(
        stored.status, state::ENROLLED_STATUS,
        "case {case:?}: associating {account_id} must enable the Automation feature, got \
         {stored:?}"
    );
    assert_eq!(
        stored.organization_rule_mode.as_deref(),
        Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        "case {case:?}: associating {account_id} must set its organization rule mode to \
         AnyAllowed, got {stored:?}"
    );
    assert!(
        stored.last_updated_timestamp.is_some(),
        "case {case:?}: an association is a change to {account_id}'s configuration, so it \
         carries an update time, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// What an association stores, and what it answers
// ---------------------------------------------------------------------------

/// Every account id a request names is associated, whatever the SHAPE of the
/// list: one id, several, the same id twice, the calling account itself, and the
/// documented maximum. The response reports exactly the accounts the store holds
/// as associated, each once, and no per-account error.
#[tokio::test]
async fn an_association_stores_every_account_the_request_names() {
    // (case, requested ids, the accounts the response must report)
    let fifty = member_account_ids(state::MAX_ACCOUNT_IDS_PER_REQUEST);
    let caller = state::caller_account_id();
    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one-member",
            vec![MEMBER_A.to_string()],
            vec![MEMBER_A.to_string()],
        ),
        (
            "three-members",
            vec![
                MEMBER_A.to_string(),
                MEMBER_B.to_string(),
                MEMBER_C.to_string(),
            ],
            vec![
                MEMBER_A.to_string(),
                MEMBER_B.to_string(),
                MEMBER_C.to_string(),
            ],
        ),
        (
            "the-same-id-twice",
            vec![MEMBER_B.to_string(), MEMBER_B.to_string()],
            vec![MEMBER_B.to_string()],
        ),
        (
            "the-calling-account",
            vec![caller.clone()],
            vec![caller.clone()],
        ),
        (
            "a-member-and-the-calling-account",
            vec![MEMBER_C.to_string(), caller.clone()],
            vec![MEMBER_C.to_string(), caller.clone()],
        ),
        ("the-documented-maximum", fifty.clone(), fifty),
    ];

    for (case, requested, expected) in cases {
        // One fresh store per row, so each row starts from an organization in
        // which nothing has been associated.
        let (router, engine) = emulator_with_engine();

        let associated = call_ok(
            &router,
            "AssociateAccounts",
            &json!({"accountIds": requested}),
            case,
        )
        .await;

        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            reported_account_ids(&associated),
            expected_sorted,
            "case {case:?}: the response must report each associated account once, got \
             {associated}"
        );
        assert_eq!(
            member(&associated, "errors")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            0,
            "case {case:?}: every well-formed account id was associated, so nothing failed, got \
             {associated}"
        );

        for account_id in &expected {
            assert_stored_association(&engine, account_id, case);
        }
        // An account the request did not name is untouched.
        let untouched = stored_enrollment(&engine, "999999999999");
        assert_eq!(
            untouched.organization_rule_mode, None,
            "case {case:?}: an account the request never named must not be associated, got \
             {untouched:?}"
        );
    }
}

/// One row's PRIOR enrollment configuration of the member account under test:
/// its `status`, the `statusReason` that explains it and its
/// `organizationRuleMode`.
type PriorEnrollment = (&'static str, Option<&'static str>, Option<&'static str>);

/// The association an account already carries decides what the call CHANGES: an
/// account that had not enabled the Automation feature has it enabled, whatever
/// non-enabled status it was in, and the reason that explained that status is
/// dropped; an account already enabled keeps its status and only gains the
/// `AnyAllowed` rule mode.
#[tokio::test]
async fn an_association_enables_the_feature_from_every_prior_status() {
    // The `EnrollmentStatus` value set, from the Smithy model
    // (`smithy_query(query="shape",shape="EnrollmentStatus")`), as the status a
    // member account is in BEFORE the association — plus the account that has
    // never configured an enrollment at all (`None`), and one that is already
    // enabled but whose rule mode is `NoneAllowed`.
    let cases: [(&str, Option<PriorEnrollment>); 6] = [
        ("never-enrolled", None),
        ("inactive", Some(("Inactive", None, None))),
        ("pending", Some(("Pending", Some("opt-in in progress"), None))),
        ("failed", Some(("Failed", Some("the opt-in did not complete"), None))),
        ("active-without-a-rule-mode", Some(("Active", None, None))),
        (
            "active-but-not-allowed",
            Some(("Active", None, Some("NoneAllowed"))),
        ),
    ];

    for (case, seed) in cases {
        let (router, engine) = emulator_with_engine();
        if let Some((status, reason, mode)) = seed {
            seed_enrollment(&engine, MEMBER_A, status, reason, mode);
        }

        let associated = call_ok(
            &router,
            "AssociateAccounts",
            &json!({"accountIds": [MEMBER_A]}),
            case,
        )
        .await;
        assert_eq!(
            reported_account_ids(&associated),
            vec![MEMBER_A.to_string()],
            "case {case:?}: the account must be reported associated, got {associated}"
        );

        assert_stored_association(&engine, MEMBER_A, case);
        let stored = stored_enrollment(&engine, MEMBER_A);
        assert_eq!(
            stored.status_reason, None,
            "case {case:?}: the reason belongs to the status it explains, and the account is \
             now enabled, got {stored:?}"
        );
    }
}

/// The caller's own association is readable back over the wire: associating the
/// calling account leaves `GetEnrollmentConfiguration` reporting `AnyAllowed`
/// and the enabled status, and re-associating it changes nothing at all — not
/// even the update timestamp.
#[tokio::test]
async fn the_callers_association_is_readable_through_its_enrollment_configuration() {
    // The unenrolled harness: this case establishes the caller's enrollment over
    // the wire, so nothing may have been stored for it first.
    let (router, _engine) = unenrolled_emulator_with_engine();
    let caller = state::caller_account_id();

    let opted_in = call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active"}),
        "opt-in",
    )
    .await;
    assert_eq!(
        string_member(&opted_in, "status"),
        "Active",
        "case \"opt-in\": the caller must be enrolled before it may associate, got {opted_in}"
    );

    let before = call_ok(
        &router,
        "GetEnrollmentConfiguration",
        &json!({}),
        "before-association",
    )
    .await;
    assert!(
        before.get("organizationRuleMode").is_none(),
        "case \"before-association\": an account that was never associated has no organization \
         rule mode, got {before}"
    );

    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [caller, MEMBER_A], "clientToken": "assoc-readback-1"}),
        "associate",
    )
    .await;

    let after = call_ok(
        &router,
        "GetEnrollmentConfiguration",
        &json!({}),
        "after-association",
    )
    .await;
    assert_eq!(
        string_member(&after, "organizationRuleMode"),
        state::ORGANIZATION_RULE_MODE_ANY_ALLOWED,
        "case \"after-association\": the association must be readable back, got {after}"
    );
    assert_eq!(
        string_member(&after, "status"),
        state::ENROLLED_STATUS,
        "case \"after-association\": an associated account has the Automation feature enabled, \
         got {after}"
    );
    let stamped = member(&after, "lastUpdatedTimestamp").clone();
    assert!(
        stamped.is_number(),
        "case \"after-association\": the association changed the configuration, so it is \
         stamped, got {after}"
    );

    // A second association of the same account, under a DIFFERENT client token
    // (so no replay is involved): the account is already associated, so nothing
    // changes — the stored timestamp included.
    let again = call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [state::caller_account_id()], "clientToken": "assoc-readback-2"}),
        "re-associate",
    )
    .await;
    assert_eq!(
        reported_account_ids(&again),
        vec![state::caller_account_id()],
        "case \"re-associate\": an already-associated account is still reported associated, got \
         {again}"
    );
    let read_back = call_ok(
        &router,
        "GetEnrollmentConfiguration",
        &json!({}),
        "after-re-association",
    )
    .await;
    assert_eq!(
        member(&read_back, "lastUpdatedTimestamp"),
        &stamped,
        "case \"after-re-association\": re-associating an associated account changes nothing, \
         so its update time stands, got {read_back}"
    );
}

// ---------------------------------------------------------------------------
// The documented size of `accountIds`
// ---------------------------------------------------------------------------

/// `accountIds` is bounded by the documented 1..=50: every count inside the
/// bound is associated, and a count at either side of it is refused with
/// `InvalidParameterValueException` (400) naming the member — having associated
/// nothing.
#[tokio::test]
async fn the_documented_account_id_count_is_enforced() {
    // (case, how many ids, accepted) — each published bound and one past it.
    let cases: [(&str, usize, bool); 6] = [
        ("none", 0, false),
        ("one", 1, true),
        ("two", 2, true),
        ("fifty", state::MAX_ACCOUNT_IDS_PER_REQUEST, true),
        ("fifty-one", state::MAX_ACCOUNT_IDS_PER_REQUEST + 1, false),
        ("sixty", 60, false),
    ];

    for (case, count, accepted) in cases {
        let (router, engine) = emulator_with_engine();
        let requested = member_account_ids(count);
        let answer = call(&router, "AssociateAccounts", &json!({"accountIds": requested})).await;

        if accepted {
            let (status, body) = answer;
            assert_eq!(
                status, 200,
                "case {case:?}: {count} account ids are within the documented bound, got \
                 {status} and {body}"
            );
            assert_eq!(
                reported_account_ids(&body).len(),
                count,
                "case {case:?}: every requested account must be reported associated, got {body}"
            );
            for account_id in member_account_ids(count) {
                assert_stored_association(&engine, &account_id, case);
            }
        } else {
            let message = answer
                .1
                .get("message")
                .or_else(|| answer.1.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_refused(answer, 400, "InvalidParameterValueException", case);
            assert!(
                message.contains("accountIds"),
                "case {case:?}: the refusal must name the member it is about, got {message:?}"
            );
            // A refused request associated nothing, not even a prefix of the list.
            for account_id in member_account_ids(count) {
                let stored = stored_enrollment(&engine, &account_id);
                assert_eq!(
                    stored.organization_rule_mode, None,
                    "case {case:?}: a refused association must store nothing for {account_id}, \
                     got {stored:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The store's own capacity
// ---------------------------------------------------------------------------

/// The enrollment store holds at most `state::MAX_ENROLLED_ACCOUNTS` accounts:
/// an association that would bring in more NEW accounts than there is room for
/// is refused with `InvalidParameterValueException` (400) and stores nothing,
/// while re-associating accounts the store already holds still succeeds, because
/// that grows it by nothing.
#[tokio::test]
async fn an_association_cannot_grow_the_store_past_its_capacity() {
    let (router, engine) = emulator_with_engine();

    // Fill the store to one short of capacity. Written where the handlers write
    // it, because reaching the cap over the wire would take hundreds of calls.
    let filled = member_account_ids(state::MAX_ENROLLED_ACCOUNTS - 1);
    for account_id in &filled {
        seed_enrollment(
            &engine,
            account_id,
            state::ENROLLED_STATUS,
            None,
            Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        );
    }
    // The caller's own enrollment is the store's last free slot's neighbour: the
    // enrolled harness already stored it, so the store now holds exactly the cap.
    let occupied = state::MAX_ENROLLED_ACCOUNTS;
    assert_eq!(
        filled.len() + 1,
        occupied,
        "case \"precondition\": the store must be filled to its cap before the cases run"
    );

    // (case, requested ids, accepted)
    let known = filled.first().cloned().unwrap_or_default();
    let cases: Vec<(&str, Vec<String>, bool)> = vec![
        (
            "accounts-already-stored",
            vec![known.clone(), state::caller_account_id()],
            true,
        ),
        ("one-new-account", vec![MEMBER_A.to_string()], false),
        (
            "a-known-account-and-a-new-one",
            vec![known, MEMBER_B.to_string()],
            false,
        ),
    ];

    for (case, requested, accepted) in cases {
        let answer = call(
            &router,
            "AssociateAccounts",
            &json!({"accountIds": requested.clone()}),
        )
        .await;
        if accepted {
            let (status, body) = answer;
            assert_eq!(
                status, 200,
                "case {case:?}: accounts the store already holds grow it by nothing, got {status} \
                 and {body}"
            );
            for account_id in &requested {
                assert_stored_association(&engine, account_id, case);
            }
        } else {
            assert_refused(answer, 400, "InvalidParameterValueException", case);
            for account_id in &requested {
                let stored = stored_enrollment(&engine, account_id);
                if account_id == MEMBER_A || account_id == MEMBER_B {
                    assert_eq!(
                        stored.organization_rule_mode, None,
                        "case {case:?}: a refused association must store nothing for \
                         {account_id}, got {stored:?}"
                    );
                }
            }
        }
    }
}


// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// A replayed `clientToken` answers the first call's result recomputed from the
/// store and associates nothing twice; the same token with different parameters
/// is refused with `IdempotentParameterMismatchException` (409), and a token
/// another operation already used with `IdempotencyTokenInUseException` (409).
#[tokio::test]
async fn a_client_token_makes_an_association_idempotent() {
    let (router, engine) = emulator_with_engine();
    let request = json!({"accountIds": [MEMBER_A, MEMBER_B], "clientToken": "assoc-token-1"});

    let first = call_ok(&router, "AssociateAccounts", &request, "first").await;
    let stamped = stored_enrollment(&engine, MEMBER_A).last_updated_timestamp;
    assert!(
        stamped.is_some(),
        "case \"first\": the association must be stamped where it is stored"
    );

    let replay = call_ok(&router, "AssociateAccounts", &request, "replay").await;
    assert_eq!(
        reported_account_ids(&replay),
        reported_account_ids(&first),
        "case \"replay\": the retry must answer the first call's result, got {replay}"
    );
    assert_eq!(
        stored_enrollment(&engine, MEMBER_A)
            .last_updated_timestamp
            .map(|at| at.as_secs_f64()),
        stamped.map(|at| at.as_secs_f64()),
        "case \"replay\": a replayed token must not store the association a second time"
    );

    // The same token, a different account list: the request behind the token is
    // not the one being retried.
    assert_refused(
        call(
            &router,
            "AssociateAccounts",
            &json!({"accountIds": [MEMBER_C], "clientToken": "assoc-token-1"}),
        )
        .await,
        409,
        "IdempotentParameterMismatchException",
        "parameter-mismatch",
    );
    let untouched = stored_enrollment(&engine, MEMBER_C);
    assert_eq!(
        untouched.organization_rule_mode, None,
        "case \"parameter-mismatch\": the refused call must have associated nothing, got \
         {untouched:?}"
    );

    // A token another operation claimed first.
    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active", "clientToken": "assoc-token-2"}),
        "other-operation",
    )
    .await;
    assert_refused(
        call(
            &router,
            "AssociateAccounts",
            &json!({"accountIds": [MEMBER_C], "clientToken": "assoc-token-2"}),
        )
        .await,
        409,
        "IdempotencyTokenInUseException",
        "token-in-use",
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ORGANIZATION ACCOUNT DISASSOCIATION
//! (`DisassociateAccounts`).
//!
//! The rule under test: a disassociation request records, for EVERY account id it
//! names that the enrollment store already holds, that the organization no longer
//! reaches it — the account's organization rule mode becomes `NoneAllowed`
//! ("Once disassociated, organization rules no longer apply to the member
//! account, and the management account (or delegated administrator) cannot create
//! Automation rules for that account") — while that account's OWN enrollment
//! status and the reason that explains it are left exactly as they were, and it
//! answers with the accounts the STORE reports as no longer associated. The
//! request is bounded by the size of `accountIds` and is idempotent under a
//! replayed `clientToken`.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the emulator binary mounts — using the harness `automation_rule_lifecycle`
//! owns. A member account's configuration is read back over the wire through
//! `ListAccounts` in the round-trip case; the table rows read the store where the
//! handler writes it (through the reader `account_association` owns), so a case
//! about what a disassociation PERSISTS does not depend on a second operation's
//! projection of it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::account_association::{seed_enrollment, stored_enrollment};
use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, member, string_member,
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
    (0..n)
        .map(|index| format!("{:012}", 500_000_000_000u64 + index as u64))
        .collect()
}

/// The `accountIds` a disassociation response reports, sorted, so a case asserts
/// WHICH accounts it named without pinning an order the model does not give.
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

/// Associate `account_ids` over the wire, which is how a row reaches the state a
/// disassociation acts on.
async fn associate(router: &MultiServiceRouter, account_ids: &[String], case: &str) {
    call_ok(
        router,
        "AssociateAccounts",
        &json!({"accountIds": account_ids}),
        case,
    )
    .await;
}

/// Assert the store reports `account_id` as carrying no association, which is
/// what a disassociation persists.
fn assert_stored_disassociation(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    case: &str,
) {
    let stored = stored_enrollment(engine, account_id);
    assert_eq!(
        stored.organization_rule_mode.as_deref(),
        Some(state::ORGANIZATION_RULE_MODE_NONE_ALLOWED),
        "case {case:?}: disassociating {account_id} must record that the organization no longer \
         reaches it, got {stored:?}"
    );
    assert!(
        stored.last_updated_timestamp.is_some(),
        "case {case:?}: the removal is a change to {account_id}'s configuration, so it carries \
         an update time, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// What a disassociation stores, and what it answers
// ---------------------------------------------------------------------------

/// One row of the list-shape table: its case name, the accounts to associate
/// before the call, the ids the request names and the accounts the response must
/// report.
type ListShapeRow = (&'static str, Vec<String>, Vec<String>, Vec<String>);

/// Every account id a request names loses its association, whatever the SHAPE of
/// the list: one id, several, the same id twice, the calling account itself, a
/// subset of the associated accounts, and the maximum one request may name. The
/// response reports exactly the accounts the store holds as no longer
/// associated, each once, and no per-account error — and an associated account
/// the request did not name keeps its association.
#[tokio::test]
async fn a_disassociation_removes_the_association_of_every_account_the_request_names() {
    // (case, the accounts to associate first, the requested ids, the accounts the
    // response must report)
    let fifty = member_account_ids(state::MAX_ACCOUNT_IDS_PER_REQUEST);
    let caller = state::caller_account_id();
    let cases: Vec<ListShapeRow> = vec![
        (
            "one-member",
            vec![MEMBER_A.to_string()],
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
            vec![
                MEMBER_A.to_string(),
                MEMBER_B.to_string(),
                MEMBER_C.to_string(),
            ],
        ),
        (
            "the-same-id-twice",
            vec![MEMBER_B.to_string()],
            vec![MEMBER_B.to_string(), MEMBER_B.to_string()],
            vec![MEMBER_B.to_string()],
        ),
        (
            "a-subset-of-the-associated-accounts",
            vec![MEMBER_A.to_string(), MEMBER_B.to_string()],
            vec![MEMBER_A.to_string()],
            vec![MEMBER_A.to_string()],
        ),
        (
            "the-calling-account",
            vec![caller.clone()],
            vec![caller.clone()],
            vec![caller.clone()],
        ),
        (
            "a-member-and-the-calling-account",
            vec![MEMBER_C.to_string(), caller.clone()],
            vec![MEMBER_C.to_string(), caller.clone()],
            vec![MEMBER_C.to_string(), caller.clone()],
        ),
        (
            "an-account-the-store-never-saw",
            Vec::new(),
            vec![MEMBER_A.to_string()],
            vec![MEMBER_A.to_string()],
        ),
        (
            "the-largest-request",
            fifty.clone(),
            fifty.clone(),
            fifty,
        ),
    ];

    for (case, associated, requested, expected) in cases {
        // One fresh store per row, so each row starts from the organization its
        // own associations describe.
        let (router, engine) = emulator_with_engine();
        if !associated.is_empty() {
            associate(&router, &associated, case).await;
        }

        let answer = call_ok(
            &router,
            "DisassociateAccounts",
            &json!({"accountIds": requested.clone()}),
            case,
        )
        .await;

        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            reported_account_ids(&answer),
            expected_sorted,
            "case {case:?}: the response must report each disassociated account once, got {answer}"
        );
        assert_eq!(
            member(&answer, "errors")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            0,
            "case {case:?}: every well-formed account id was disassociated, so nothing failed, \
             got {answer}"
        );

        for account_id in &expected {
            let stored = stored_enrollment(&engine, account_id);
            assert_eq!(
                stored.reported_organization_rule_mode(),
                state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
                "case {case:?}: the store must report {account_id} as carrying no association, \
                 got {stored:?}"
            );
        }

        // An ASSOCIATED account the request never named keeps its association.
        for account_id in associated.iter().filter(|id| !requested.contains(id)) {
            let stored = stored_enrollment(&engine, account_id);
            assert_eq!(
                stored.organization_rule_mode.as_deref(),
                Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
                "case {case:?}: an account the request never named must keep its association, \
                 got {stored:?}"
            );
        }
    }
}

/// An account the enrollment store has never seen carries no association to
/// remove, so a disassociation that names it stores nothing for it: the store
/// does not grow, and the account is reported by nothing that lists the accounts
/// this service knows.
#[tokio::test]
async fn disassociating_an_unknown_account_stores_nothing_for_it() {
    let (router, engine) = emulator_with_engine();
    let unknown = member_account_ids(3);

    let answer = call_ok(
        &router,
        "DisassociateAccounts",
        &json!({"accountIds": unknown.clone()}),
        "unknown-accounts",
    )
    .await;
    assert_eq!(
        reported_account_ids(&answer).len(),
        unknown.len(),
        "case \"unknown-accounts\": an account that carries no association is not associated \
         after the call either, got {answer}"
    );

    for account_id in &unknown {
        let stored = stored_enrollment(&engine, account_id);
        assert_eq!(
            stored.organization_rule_mode, None,
            "case \"unknown-accounts\": the call must store no configuration for {account_id}, \
             got {stored:?}"
        );
        assert_eq!(
            stored.last_updated_timestamp, None,
            "case \"unknown-accounts\": nothing was written for {account_id}, so nothing is \
             stamped, got {stored:?}"
        );
    }

    let listed = call_ok(&router, "ListAccounts", &json!({}), "unknown-accounts").await;
    let listed_ids: Vec<String> = member(&listed, "accounts")
        .as_array()
        .map(|accounts| {
            accounts
                .iter()
                .map(|account| string_member(account, "accountId"))
                .collect()
        })
        .unwrap_or_default();
    for account_id in &unknown {
        assert!(
            !listed_ids.contains(account_id),
            "case \"unknown-accounts\": the service must still know nothing of {account_id}, got \
             {listed}"
        );
    }
}

/// One row's PRIOR enrollment configuration of the member account under test:
/// its `status`, the `statusReason` that explains it and its
/// `organizationRuleMode`.
type PriorEnrollment = (&'static str, Option<&'static str>, Option<&'static str>);

/// A disassociation touches the ASSOCIATION only: whatever enrollment status the
/// account is in, and whatever reason explains it, both stand — an account's own
/// enrollment is set by that account, never by the organization's management
/// account.
#[tokio::test]
async fn a_disassociation_leaves_the_accounts_own_enrollment_alone() {
    // The `EnrollmentStatus` value set, from the Smithy model
    // (`smithy_query(query="shape",shape="EnrollmentStatus")`), as the status the
    // associated member account is in BEFORE the removal.
    let cases: [(&str, PriorEnrollment); 5] = [
        ("active", ("Active", None, Some("AnyAllowed"))),
        ("inactive", ("Inactive", None, Some("AnyAllowed"))),
        (
            "pending",
            ("Pending", Some("opt-in in progress"), Some("AnyAllowed")),
        ),
        (
            "failed",
            (
                "Failed",
                Some("the opt-in did not complete"),
                Some("AnyAllowed"),
            ),
        ),
        ("active-already-not-allowed", ("Active", None, Some("NoneAllowed"))),
    ];

    for (case, (status, reason, mode)) in cases {
        let (router, engine) = emulator_with_engine();
        seed_enrollment(&engine, MEMBER_A, status, reason, mode);
        let stamped_before = stored_enrollment(&engine, MEMBER_A).last_updated_timestamp;

        let answer = call_ok(
            &router,
            "DisassociateAccounts",
            &json!({"accountIds": [MEMBER_A]}),
            case,
        )
        .await;
        assert_eq!(
            reported_account_ids(&answer),
            vec![MEMBER_A.to_string()],
            "case {case:?}: the account must be reported disassociated, got {answer}"
        );

        let stored = stored_enrollment(&engine, MEMBER_A);
        assert_eq!(
            stored.reported_organization_rule_mode(),
            state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
            "case {case:?}: the organization must no longer reach the account, got {stored:?}"
        );
        assert_eq!(
            stored.status, status,
            "case {case:?}: the account's own enrollment status is not the organization's to \
             change, got {stored:?}"
        );
        assert_eq!(
            stored.status_reason.as_deref(),
            reason,
            "case {case:?}: the reason belongs to the status it explains, which did not change, \
             got {stored:?}"
        );
        if mode == Some(state::ORGANIZATION_RULE_MODE_NONE_ALLOWED) {
            assert_eq!(
                stored.last_updated_timestamp.map(|at| at.as_secs_f64()),
                stamped_before.map(|at| at.as_secs_f64()),
                "case {case:?}: an account that carries no association is not changed, so its \
                 update time stands, got {stored:?}"
            );
        } else {
            assert_stored_disassociation(&engine, MEMBER_A, case);
        }
    }
}

/// The removal is readable back over the wire: a member account `ListAccounts`
/// reported as `AnyAllowed` reports `NoneAllowed` once it is disassociated, the
/// caller's own removal is readable through `GetEnrollmentConfiguration`, and a
/// second removal under a different client token changes nothing at all — not
/// even the update timestamp.
#[tokio::test]
async fn a_disassociation_is_readable_back_through_the_account_readers() {
    let (router, _engine) = emulator_with_engine();
    let caller = state::caller_account_id();

    associate(
        &router,
        &[caller.clone(), MEMBER_A.to_string()],
        "associate",
    )
    .await;

    let listed = call_ok(&router, "ListAccounts", &json!({}), "before-removal").await;
    let member_account = member(&listed, "accounts")
        .as_array()
        .and_then(|accounts| {
            accounts
                .iter()
                .find(|account| string_member(account, "accountId") == MEMBER_A)
                .cloned()
        })
        .unwrap_or(Value::Null);
    assert_eq!(
        string_member(&member_account, "organizationRuleMode"),
        state::ORGANIZATION_RULE_MODE_ANY_ALLOWED,
        "case \"before-removal\": the association must be readable first, got {listed}"
    );

    call_ok(
        &router,
        "DisassociateAccounts",
        &json!({"accountIds": [caller.clone(), MEMBER_A], "clientToken": "disassoc-readback-1"}),
        "removal",
    )
    .await;

    let listed = call_ok(&router, "ListAccounts", &json!({}), "after-removal").await;
    let member_account = member(&listed, "accounts")
        .as_array()
        .and_then(|accounts| {
            accounts
                .iter()
                .find(|account| string_member(account, "accountId") == MEMBER_A)
                .cloned()
        })
        .unwrap_or(Value::Null);
    assert_eq!(
        string_member(&member_account, "organizationRuleMode"),
        state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
        "case \"after-removal\": the removal must be readable back, got {listed}"
    );
    assert_eq!(
        string_member(&member_account, "status"),
        state::ENROLLED_STATUS,
        "case \"after-removal\": the member account keeps the Automation feature it had \
         enabled, got {listed}"
    );

    let own = call_ok(
        &router,
        "GetEnrollmentConfiguration",
        &json!({}),
        "after-removal",
    )
    .await;
    assert_eq!(
        string_member(&own, "organizationRuleMode"),
        state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
        "case \"after-removal\": the caller's own removal must be readable back, got {own}"
    );
    assert_eq!(
        string_member(&own, "status"),
        state::ENROLLED_STATUS,
        "case \"after-removal\": the caller stays opted in to Compute Optimizer Automation, got \
         {own}"
    );
    let stamped = member(&own, "lastUpdatedTimestamp").clone();

    // A second removal of the same accounts, under a DIFFERENT client token (so
    // no replay is involved): they already carry no association, so nothing
    // changes — the stored timestamp included.
    let again = call_ok(
        &router,
        "DisassociateAccounts",
        &json!({"accountIds": [caller.clone()], "clientToken": "disassoc-readback-2"}),
        "re-disassociate",
    )
    .await;
    assert_eq!(
        reported_account_ids(&again),
        vec![caller],
        "case \"re-disassociate\": an account that carries no association is still reported \
         disassociated, got {again}"
    );
    let read_back = call_ok(
        &router,
        "GetEnrollmentConfiguration",
        &json!({}),
        "after-re-disassociation",
    )
    .await;
    assert_eq!(
        member(&read_back, "lastUpdatedTimestamp"),
        &stamped,
        "case \"after-re-disassociation\": disassociating an account that carries no association \
         changes nothing, so its update time stands, got {read_back}"
    );
}

// ---------------------------------------------------------------------------
// The size of `accountIds`
// ---------------------------------------------------------------------------

/// `accountIds` is bounded by 1..=`state::MAX_ACCOUNT_IDS_PER_REQUEST`: every
/// count inside the bound is acted on, and a count at either side of it is
/// refused with `InvalidParameterValueException` (400) naming the member —
/// having disassociated nothing.
#[tokio::test]
async fn the_account_id_count_is_enforced() {
    // (case, how many ids, accepted) — each bound and one past it.
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
        // Every requested account starts out ASSOCIATED, so a refused call is
        // visible as an association that is still there.
        for chunk in requested.chunks(state::MAX_ACCOUNT_IDS_PER_REQUEST) {
            associate(&router, chunk, case).await;
        }

        let answer = call(
            &router,
            "DisassociateAccounts",
            &json!({"accountIds": requested.clone()}),
        )
        .await;

        if accepted {
            let (status, body) = answer;
            assert_eq!(
                status, 200,
                "case {case:?}: {count} account ids are within the bound, got {status} and {body}"
            );
            assert_eq!(
                reported_account_ids(&body).len(),
                count,
                "case {case:?}: every requested account must be reported disassociated, got \
                 {body}"
            );
            for account_id in &requested {
                assert_stored_disassociation(&engine, account_id, case);
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
            // A refused request disassociated nothing, not even a prefix of the
            // list.
            for account_id in &requested {
                let stored = stored_enrollment(&engine, account_id);
                assert_eq!(
                    stored.organization_rule_mode.as_deref(),
                    Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
                    "case {case:?}: a refused removal must leave {account_id} associated, got \
                     {stored:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// A replayed `clientToken` answers the first call's result recomputed from the
/// store and disassociates nothing twice; the same token with different
/// parameters is refused with `IdempotentParameterMismatchException` (409), and a
/// token another operation already used with `IdempotencyTokenInUseException`
/// (409).
#[tokio::test]
async fn a_client_token_makes_a_disassociation_idempotent() {
    let (router, engine) = emulator_with_engine();
    associate(
        &router,
        &[
            MEMBER_A.to_string(),
            MEMBER_B.to_string(),
            MEMBER_C.to_string(),
        ],
        "seed",
    )
    .await;

    let request = json!({"accountIds": [MEMBER_A, MEMBER_B], "clientToken": "disassoc-token-1"});
    let first = call_ok(&router, "DisassociateAccounts", &request, "first").await;
    let stamped = stored_enrollment(&engine, MEMBER_A).last_updated_timestamp;
    assert!(
        stamped.is_some(),
        "case \"first\": the removal must be stamped where it is stored"
    );

    let replay = call_ok(&router, "DisassociateAccounts", &request, "replay").await;
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
        "case \"replay\": a replayed token must not store the removal a second time"
    );

    // The same token, a different account list: the request behind the token is
    // not the one being retried.
    assert_refused(
        call(
            &router,
            "DisassociateAccounts",
            &json!({"accountIds": [MEMBER_C], "clientToken": "disassoc-token-1"}),
        )
        .await,
        409,
        "IdempotentParameterMismatchException",
        "parameter-mismatch",
    );
    let untouched = stored_enrollment(&engine, MEMBER_C);
    assert_eq!(
        untouched.organization_rule_mode.as_deref(),
        Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        "case \"parameter-mismatch\": the refused call must have disassociated nothing, got \
         {untouched:?}"
    );

    // A token another operation claimed first.
    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [MEMBER_A], "clientToken": "disassoc-token-2"}),
        "other-operation",
    )
    .await;
    assert_refused(
        call(
            &router,
            "DisassociateAccounts",
            &json!({"accountIds": [MEMBER_C], "clientToken": "disassoc-token-2"}),
        )
        .await,
        409,
        "IdempotencyTokenInUseException",
        "token-in-use",
    );
    let untouched = stored_enrollment(&engine, MEMBER_C);
    assert_eq!(
        untouched.organization_rule_mode.as_deref(),
        Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        "case \"token-in-use\": the refused call must have disassociated nothing, got \
         {untouched:?}"
    );
}

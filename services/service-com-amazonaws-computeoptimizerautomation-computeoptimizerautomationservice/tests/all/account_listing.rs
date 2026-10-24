// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORGANIZATION ACCOUNT LISTING (`ListAccounts`).
//!
//! The rule under test: the listing reports one `AccountInfo` per account the
//! enrollment store holds a configuration for, each member projected from what
//! is PERSISTED for that account — its `status` ("The enrollment status of the
//! account"), the `statusReason` that explains it, its `organizationRuleMode`
//! ("whether the management account can create Automation rules that implement
//! optimization actions for this account") and the `lastUpdatedTimestamp` of
//! that configuration — over one stable sequence that `maxResults` and
//! `nextToken` page.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the emulator binary mounts — using the harness `automation_rule_lifecycle`
//! owns. A configuration no operation of this API can establish (the two
//! `EnrollmentStatus` values a caller may not send, and any configuration of an
//! account other than the caller's) is seeded through the writer
//! `account_association` owns.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::account_association::{seed_enrollment, stored_enrollment};
use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, is_next_token, member, string_member,
    unenrolled_emulator_with_engine,
};

/// Member account ids in the modelled `AccountId` form (`^[0-9]{12}$`) that are
/// not the calling account.
const MEMBER_A: &str = "111111111111";
const MEMBER_B: &str = "222222222222";

/// `n` distinct member account ids, for the cases that are about the SIZE of the
/// listing rather than about one account's configuration.
fn member_account_ids(n: usize) -> Vec<String> {
    (0..n)
        .map(|index| format!("{:012}", 700_000_000_000u64 + index as u64))
        .collect()
}

/// The `accounts` a listing reported.
fn listed(body: &Value) -> Vec<Value> {
    member(body, "accounts")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The account ids a listing reported, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|account| string_member(account, "accountId"))
        .collect()
}

/// The one account a listing reported for `account_id`, or `None`.
fn listed_account(body: &Value, account_id: &str) -> Option<Value> {
    listed(body)
        .into_iter()
        .find(|account| string_member(account, "accountId") == account_id)
}

/// Every account the listing reports, walked to the end so no case depends on a
/// page size. `maxResults` is left out, so the service's own default page size
/// is what cuts the pages.
async fn list_every_account(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = call_ok(router, "ListAccounts", &request, case).await;
        walked.extend(listed(&page));
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() <= state::MAX_ENROLLED_ACCOUNTS,
            "case {case:?}: the walk must terminate, walked {} accounts",
            walked.len()
        );
    }
    walked
}

/// `AccountId` is modelled `^[0-9]{12}$`.
fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Assert the reported `account` is the projection of what the store holds for
/// `account_id`: the stored status and reason verbatim, the stored organization
/// rule mode or `NoneAllowed` when the account carries none, and the stored
/// update instant.
fn assert_projects_the_store(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    account: &Value,
    account_id: &str,
    case: &str,
) {
    let stored = stored_enrollment(engine, account_id);
    assert_eq!(
        string_member(account, "status"),
        stored.status,
        "case {case:?}: the listing must report the stored status of {account_id}, got {account}"
    );
    assert_eq!(
        account.get("statusReason").and_then(Value::as_str),
        stored.status_reason.as_deref(),
        "case {case:?}: the listing must report the stored status reason of {account_id}, got \
         {account}"
    );
    let expected_mode = stored
        .organization_rule_mode
        .clone()
        .unwrap_or_else(|| state::ORGANIZATION_RULE_MODE_NONE_ALLOWED.to_string());
    assert_eq!(
        string_member(account, "organizationRuleMode"),
        expected_mode,
        "case {case:?}: an account carries the rule mode the store holds, and NoneAllowed while \
         it holds none, got {account}"
    );
    let reported = member(account, "lastUpdatedTimestamp")
        .as_f64()
        .unwrap_or(f64::NAN);
    match stored.last_updated_timestamp {
        Some(stamped) => assert!(
            (reported - stamped.as_secs_f64()).abs() < 1e-3,
            "case {case:?}: {account_id} must report the stored update instant {}, got {account}",
            stamped.as_secs_f64()
        ),
        None => assert!(
            reported > 0.0,
            "case {case:?}: lastUpdatedTimestamp is a required member, got {account}"
        ),
    }
}

/// One row of the projection table: the case name, the account id, and the
/// `status`, `statusReason` and `organizationRuleMode` the store holds for it.
type StoredRow = (
    &'static str,
    String,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
);

// ---------------------------------------------------------------------------
// What the listing reports
// ---------------------------------------------------------------------------

/// One row per account the store holds, projected from that account's stored
/// configuration — across the whole `EnrollmentStatus` value set, both
/// `OrganizationRuleMode` values, an account carrying no rule mode at all, and a
/// status with and without the reason that explains it.
#[tokio::test]
async fn the_listing_projects_every_stored_enrollment() {
    // (case, account id, stored status, stored reason, stored rule mode).
    //
    // The statuses are the `EnrollmentStatus` value set and the modes the
    // `OrganizationRuleMode` value set, from the Smithy model
    // (`smithy_query(query="shape",shape="EnrollmentStatus,OrganizationRuleMode")`),
    // including the two statuses no wire call can set. The calling account is one
    // of the rows, and is `Active` because the listing is an action and the
    // service refuses every action to an account that is not opted in.
    let caller = state::caller_account_id();
    let rows: Vec<StoredRow> = vec![
        (
            "the-calling-account",
            caller.clone(),
            "Active",
            None,
            Some("AnyAllowed"),
        ),
        (
            "active-and-associated",
            MEMBER_A.to_string(),
            "Active",
            None,
            Some("AnyAllowed"),
        ),
        (
            "active-but-not-allowed",
            MEMBER_B.to_string(),
            "Active",
            None,
            Some("NoneAllowed"),
        ),
        (
            "inactive-with-no-rule-mode",
            "333333333333".to_string(),
            "Inactive",
            None,
            None,
        ),
        (
            "pending-with-a-reason",
            "444444444444".to_string(),
            "Pending",
            Some("the opt-in is still being processed"),
            Some("NoneAllowed"),
        ),
        (
            "failed-with-a-reason",
            "555555555555".to_string(),
            "Failed",
            Some("the opt-in did not complete"),
            Some("AnyAllowed"),
        ),
    ];

    let (router, engine) = emulator_with_engine();
    for (_, account_id, status, reason, mode) in &rows {
        seed_enrollment(&engine, account_id, status, *reason, *mode);
    }

    let accounts = list_every_account(&router, "every-stored-enrollment").await;
    assert_eq!(
        accounts.len(),
        rows.len(),
        "case \"every-stored-enrollment\": the listing reports one account per stored \
         enrollment, got {accounts:?}"
    );

    let listing = json!({"accounts": accounts});
    for (case, account_id, status, reason, mode) in &rows {
        let account = listed_account(&listing, account_id)
            .unwrap_or_else(|| panic!("case {case:?}: {account_id} must be listed, got {listing}"));
        assert!(
            is_account_id(&string_member(&account, "accountId")),
            "case {case:?}: accountId must match ^[0-9]{{12}}$, got {account}"
        );
        assert_eq!(
            string_member(&account, "status"),
            *status,
            "case {case:?}: the stored status must be reported as stored, got {account}"
        );
        assert_eq!(
            account.get("statusReason").and_then(Value::as_str),
            *reason,
            "case {case:?}: the reason belongs to the status it explains, got {account}"
        );
        assert_eq!(
            string_member(&account, "organizationRuleMode"),
            mode.unwrap_or(state::ORGANIZATION_RULE_MODE_NONE_ALLOWED),
            "case {case:?}: an account with no stored rule mode is one the management account \
             may not rule, got {account}"
        );
        assert_projects_the_store(&engine, &account, account_id, case);
    }

    // An account the store holds nothing for is reported by nothing.
    assert!(
        listed_account(&listing, "999999999999").is_none(),
        "case \"an-unknown-account\": only accounts the store holds are listed, got {listing}"
    );
}

/// The listing is a READ: calling it twice, and calling it between the two
/// readings of another operation, changes no account's stored configuration.
#[tokio::test]
async fn the_listing_changes_nothing() {
    let (router, engine) = emulator_with_engine();
    seed_enrollment(&engine, MEMBER_A, "Pending", Some("in progress"), None);
    seed_enrollment(
        &engine,
        MEMBER_B,
        state::ENROLLED_STATUS,
        None,
        Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
    );

    let first = list_every_account(&router, "first-read").await;
    let second = list_every_account(&router, "second-read").await;
    assert_eq!(
        first, second,
        "case \"second-read\": a read must leave the next caller the same answer, got \
         {second:?} after {first:?}"
    );
    for account_id in [MEMBER_A, MEMBER_B] {
        let stored = stored_enrollment(&engine, account_id);
        assert_eq!(
            stored.status,
            if account_id == MEMBER_A {
                "Pending"
            } else {
                state::ENROLLED_STATUS
            },
            "case \"second-read\": listing accounts must not move {account_id}'s status, got \
             {stored:?}"
        );
    }
}

/// What the wire calls that WRITE an enrollment put in the store is what the
/// listing reports: the caller's own opt-in makes it the one listed account, and
/// an association adds every member account it names, enabled and `AnyAllowed`.
#[tokio::test]
async fn the_listing_reports_what_the_wire_calls_stored() {
    // The unenrolled harness: every row of this case is established over the
    // wire, so nothing may have been stored for the caller first.
    let (router, engine) = unenrolled_emulator_with_engine();
    let caller = state::caller_account_id();

    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active"}),
        "opt-in",
    )
    .await;

    let after_opt_in = list_every_account(&router, "after-opt-in").await;
    assert_eq!(
        after_opt_in
            .iter()
            .map(|account| string_member(account, "accountId"))
            .collect::<Vec<String>>(),
        vec![caller.clone()],
        "case \"after-opt-in\": the account that opted in is the one the store holds, got \
         {after_opt_in:?}"
    );
    let listing = json!({"accounts": after_opt_in});
    let caller_row = listed_account(&listing, &caller).expect("the caller is listed");
    assert_eq!(
        string_member(&caller_row, "status"),
        state::ENROLLED_STATUS,
        "case \"after-opt-in\": the opt-in's status must be reported, got {caller_row}"
    );
    assert_eq!(
        string_member(&caller_row, "organizationRuleMode"),
        state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
        "case \"after-opt-in\": an account that was never associated may not be ruled by the \
         management account, got {caller_row}"
    );
    assert_projects_the_store(&engine, &caller_row, &caller, "after-opt-in");

    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [caller.clone(), MEMBER_A, MEMBER_B]}),
        "associate",
    )
    .await;

    let after_association = list_every_account(&router, "after-association").await;
    let listing = json!({"accounts": after_association});
    let mut expected = vec![caller.clone(), MEMBER_A.to_string(), MEMBER_B.to_string()];
    expected.sort();
    assert_eq!(
        listed_ids(&listing),
        expected,
        "case \"after-association\": every associated account joins the listing, got {listing}"
    );
    for account_id in &expected {
        let account = listed_account(&listing, account_id).expect("an associated account is listed");
        assert_eq!(
            string_member(&account, "status"),
            state::ENROLLED_STATUS,
            "case \"after-association\": an association enables the Automation feature, got \
             {account}"
        );
        assert_eq!(
            string_member(&account, "organizationRuleMode"),
            state::ORGANIZATION_RULE_MODE_ANY_ALLOWED,
            "case \"after-association\": an association sets the rule mode to AnyAllowed, got \
             {account}"
        );
        assert_projects_the_store(&engine, &account, account_id, "after-association");
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The pages of a walk are windows onto ONE sequence: for every page size from
/// one to one past the number of stored accounts, each page holds
/// `min(maxResults, remaining)` accounts, the walk reports every account exactly
/// once in the same order, and the last page carries no token.
#[tokio::test]
async fn the_listing_pages_over_one_stable_sequence() {
    let (router, engine) = emulator_with_engine();
    let members = member_account_ids(6);
    for account_id in &members {
        seed_enrollment(
            &engine,
            account_id,
            state::ENROLLED_STATUS,
            None,
            Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        );
    }

    // The sequence the pages are windows onto, taken from the unpaged walk.
    let whole = list_every_account(&router, "whole").await;
    let expected: Vec<String> = whole
        .iter()
        .map(|account| string_member(account, "accountId"))
        .collect();
    assert_eq!(
        expected.len(),
        members.len() + 1,
        "case \"whole\": the caller and every seeded member must be listed, got {whole:?}"
    );
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(
        expected, sorted,
        "case \"whole\": the listing walks one stable sequence, got {expected:?}"
    );

    for page_size in 1..=expected.len() + 1 {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = call_ok(&router, "ListAccounts", &request, &case).await;
            let ids = listed_ids(&page);
            let remaining = expected.len() - walked.len();
            assert_eq!(
                ids.len(),
                remaining.min(page_size),
                "case {case:?}: a page holds min(maxResults, remaining) accounts, got {page}"
            );
            walked.extend(ids);

            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if let Some(token) = &token {
                assert!(
                    is_next_token(token),
                    "case {case:?}: nextToken must match ^[A-Za-z0-9+/=]+$, got {token:?}"
                );
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: the walk must terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must report every account once, in the one order"
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size),
            "case {case:?}: no page beyond the last, and no nextToken on it"
        );
    }

    // A token this listing issued resumes it under ANY page size: the token is
    // bound to the walk, not to the size of the page that produced it.
    let first = call_ok(&router, "ListAccounts", &json!({"maxResults": 1}), "resume").await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a one-account page of seven leaves a further page")
        .to_string();
    let resumed = call_ok(
        &router,
        "ListAccounts",
        &json!({"maxResults": 2, "nextToken": token}),
        "resume",
    )
    .await;
    assert_eq!(
        listed_ids(&resumed),
        expected[1..3].to_vec(),
        "case \"resume\": a token resumes exactly where the previous page stopped, got {resumed}"
    );
}

/// A `nextToken` this listing did not issue is refused with
/// `InvalidParameterValueException` (400) naming the member, rather than
/// answered with a page of something else.
#[tokio::test]
async fn a_token_this_listing_did_not_issue_is_refused() {
    let (router, engine) = emulator_with_engine();
    for account_id in member_account_ids(3) {
        seed_enrollment(
            &engine,
            &account_id,
            state::ENROLLED_STATUS,
            None,
            Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        );
    }
    let before = list_every_account(&router, "before").await;

    // A token issued by ANOTHER paginated operation of this service, so the
    // rejected forms are not all hand-made: two rules make a first page of one
    // leave a token behind.
    for name in ["token-rule-a", "token-rule-b"] {
        call_ok(
            &router,
            "CreateAutomationRule",
            &super::automation_rule_lifecycle::minimal_rule(name),
            "seed-rules",
        )
        .await;
    }
    let rules_page = call_ok(
        &router,
        "ListAutomationRules",
        &json!({"maxResults": 1}),
        "seed-rules",
    )
    .await;
    let foreign_token = string_member(&rules_page, "nextToken");
    assert!(
        is_next_token(&foreign_token),
        "case \"seed-rules\": the other listing must hand back a token, got {rules_page}"
    );

    // (case, the token sent) — every form the model's `NextToken` pattern
    // (`^[A-Za-z0-9+/=]+$`) admits but this listing never issued.
    let cases: Vec<(&str, String)> = vec![
        ("not-a-token-at-all", "notatokenweissued".to_string()),
        ("a-foreign-fingerprint", hex::encode("0:")),
        ("a-cursor-with-no-offset", hex::encode(":deadbeef")),
        ("another-operations-token", foreign_token),
    ];

    for (case, token) in cases {
        let answer = call(&router, "ListAccounts", &json!({"nextToken": token})).await;
        let message = answer
            .1
            .get("message")
            .or_else(|| answer.1.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_refused(answer, 400, "InvalidParameterValueException", case);
        assert!(
            message.contains("nextToken"),
            "case {case:?}: the refusal must name the member it is about, got {message:?}"
        );
    }

    // The refused calls listed nothing and changed nothing.
    assert_eq!(
        list_every_account(&router, "after").await,
        before,
        "case \"after\": a refused token must leave the listing where it was"
    );
}

/// `ListAccountsRequest$maxResults` carries `@range(min: 1, max: 1000)`
/// (`smithy_query(query="operation",operation="ListAccounts")`), which the
/// generated request walk enforces before the handler runs: each published bound
/// is served and a value past either end is refused.
#[tokio::test]
async fn the_modelled_max_results_range_is_enforced() {
    let (router, _engine) = emulator_with_engine();

    // (case, maxResults, expected status) — each published bound, one past each,
    // and a negative.
    let cases: [(&str, i64, u16); 6] = [
        ("at-the-range-minimum", 1, 200),
        ("inside-the-range", 7, 200),
        ("at-the-range-maximum", 1000, 200),
        ("below-the-range-minimum", 0, 400),
        ("above-the-range-maximum", 1001, 400),
        ("negative", -1, 400),
    ];

    for (case, max_results, expected) in cases {
        let (status, body) = call(&router, "ListAccounts", &json!({"maxResults": max_results})).await;
        assert_eq!(
            status, expected,
            "case {case:?}: maxResults={max_results} must answer HTTP {expected}, got {status} \
             and {body}"
        );
        if expected == 200 {
            assert!(
                !listed(&body).is_empty(),
                "case {case:?}: the opted-in caller is itself a stored account, got {body}"
            );
        }
    }
}

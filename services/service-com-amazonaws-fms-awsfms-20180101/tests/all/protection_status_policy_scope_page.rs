// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a POLICY-WIDE report of a (policy, member account)
//! surface covers when the request names no member account.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`], with the
//! `X-Amz-Target` header and a SigV4 credential scope — so the whole
//! deserialize → handle → serialize walk runs and the assertions are about the
//! WIRE response.
//!
//! The rule under test is one sentence: an operation that reports about a policy
//! reports only the member accounts the policy applies to, whether the request
//! names one account or none — Firewall Manager evaluates a policy only against
//! the accounts in the policy's own scope, so a record filed under the policy for
//! an account outside that scope is a record of an evaluation that never
//! happened and is reported on no surface of the policy.
//!
//! That is the same resolution the per-account half of each surface makes (the
//! `member_account_policy_scope` suite owns it, and this suite ranges over its
//! scope table), so the two halves of one surface cannot disagree about one
//! (policy, account) pair: an account answered as not found when it is NAMED is
//! not served inside the page when it is not.
//!
//! The inventories these reports read are seeded through the store: Shield
//! Advanced detects attacks and Firewall Manager evaluates policies, and this
//! emulator stands in for neither, so the only honest source of a record is one
//! somebody put in the store. Every row seeds a record for each account the
//! policy applies to AND for each account it does not, which is the state the
//! rule is about — the scope of a policy can be narrowed after a record was
//! filed under it.
//!
//! The rows are the scope class of the `member_account_policy_scope` suite: they
//! differ in which scope map (`IncludeMap`, `ExcludeMap`, neither) and which
//! scope key (`ACCOUNT`, `ORG_UNIT`) names the accounts, in whether the named
//! values are account ids at all, in account-id length, and in how many accounts
//! the policy applies to (including none at all). Within a row the seeded attacks
//! differ in account, in start instant and in how many attacks one account has,
//! and the page is read at several page sizes so the scope filter is asserted
//! across page boundaries too.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness, the store seam, the organisation fixtures, the attack seeder
// and the scope table live in one place each for this test target, so no two
// suites can drift in how a request is signed, the store is seeded or a policy is
// scoped.
use crate::administrator_gated_operations::with_store;
use crate::member_account_policy_scope::{rows, seeded, ScopeRow};
use crate::policy_lifecycle::call;
use crate::protection_status::{data_entries, seed_attack, Attack};

/// The ids the seeded attacks are named by, in the order they are handed out —
/// enough for one attack per account of the widest row plus the extra attack the
/// plan gives one account.
const ATTACK_IDS: &[&str] = &[
    "attack-0", "attack-1", "attack-2", "attack-3", "attack-4", "attack-5", "attack-6", "attack-7",
];

/// How far apart the seeded attacks start, so the chronological order the page
/// serves is total and independent of the order they were inserted in.
const SPACING_SECS: u64 = 600;

/// One seeded attack of a row: which account it was recorded against, whether the
/// policy applies to that account, and the attack's own id.
struct Seeded {
    account: &'static str,
    in_scope: bool,
    attack_id: &'static str,
}

/// The attacks one row seeds, in the order they start.
///
/// The accounts the policy applies to and the accounts it does not are
/// INTERLEAVED, so a report that applied no scope filter would serve a different
/// sequence rather than the same one in a different order; and the row's first
/// in-scope account is given a second attack, so one account with several attacks
/// is a case too.
fn plan(row: &ScopeRow) -> Vec<Seeded> {
    let mut accounts: Vec<(&'static str, bool)> = Vec::new();
    for index in 0..row.in_scope.len().max(row.out_of_scope.len()) {
        if let Some(account) = row.out_of_scope.get(index) {
            accounts.push((*account, false));
        }
        if let Some(account) = row.in_scope.get(index) {
            accounts.push((*account, true));
        }
    }
    if let Some(account) = row.in_scope.first() {
        accounts.push((*account, true));
    }
    assert!(
        accounts.len() <= ATTACK_IDS.len(),
        "case {}: the row needs {} attack ids, more than the {} this suite names",
        row.name,
        accounts.len(),
        ATTACK_IDS.len()
    );
    accounts
        .into_iter()
        .zip(ATTACK_IDS)
        .map(|((account, in_scope), attack_id)| Seeded {
            account,
            in_scope,
            attack_id,
        })
        .collect()
}

/// Stand up one row's organisation and policy and seed its attacks.
///
/// Returns the router, the shared engine, the `PolicyId` and the row's plan.
async fn seeded_row(
    row: &ScopeRow,
) -> (
    MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
    String,
    Vec<Seeded>,
) {
    let (router, engine, policy_id) = seeded(row).await;
    let plan = plan(row);
    with_store(&engine, |store| {
        for (index, attack) in plan.iter().enumerate() {
            seed_attack(
                store,
                &policy_id,
                &Attack {
                    attack_id: attack.attack_id,
                    account: attack.account,
                    // An offset from the instant the seeder places an attack
                    // around, so the attacks start in the plan's own order.
                    start_secs: (index as u64) * SPACING_SECS,
                    start_millis: 0,
                    duration_secs: Some(60),
                    vectors: &["UDP_REFLECTION"],
                },
            );
        }
    });
    (router, engine, policy_id, plan)
}

/// The attack ids a response's `Data` document carries, in the order served.
fn reported_ids(case: &str, body: &Value) -> Vec<String> {
    data_entries(case, body)
        .iter()
        .map(|entry| {
            entry["AttackId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a Data entry carries no AttackId: {entry}"))
                .to_string()
        })
        .collect()
}

/// The account ids a response's `Data` document carries, in the order served.
fn reported_accounts(case: &str, body: &Value) -> Vec<String> {
    data_entries(case, body)
        .iter()
        .map(|entry| {
            entry["AccountId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a Data entry carries no AccountId: {entry}"))
                .to_string()
        })
        .collect()
}

/// `GetProtectionStatus` for one policy, optionally narrowed to one member
/// account and bounded by one page size.
async fn protection_status(
    router: &MultiServiceRouter,
    policy_id: &str,
    account: Option<&str>,
    max_results: Option<usize>,
) -> (u16, Value) {
    let mut body = json!({ "PolicyId": policy_id });
    if let Some(account) = account {
        body["MemberAccountId"] = json!(account);
    }
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    call(router, "GetProtectionStatus", &body).await
}

/// Read every page of `GetProtectionStatus`, following `NextToken`, and return
/// the attack ids in the order they were served.
async fn page_through(
    router: &MultiServiceRouter,
    case: &str,
    policy_id: &str,
    max_results: Option<usize>,
) -> Vec<String> {
    let mut served: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    // Enough iterations for the widest row at the smallest page size, so a token
    // that never clears fails here instead of looping forever.
    for page in 0..32 {
        let mut body = json!({ "PolicyId": policy_id });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(token) = next_token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "GetProtectionStatus", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: page {page} must answer 200, got {status} with {response}"
        );
        served.extend(reported_ids(case, &response));
        next_token = response["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return served;
        }
    }
    panic!("case {case}: NextToken never cleared, having served {served:?}");
}

/// A request that names no member account reports the attacks of the accounts the
/// policy applies to, and only those — at every page size, so the filter holds
/// across a page boundary as well as within one page.
#[tokio::test]
async fn get_protection_status_reports_only_accounts_the_policy_applies_to() {
    for row in &rows() {
        let (router, engine, policy_id, plan) = seeded_row(row).await;
        let expected: Vec<String> = plan
            .iter()
            .filter(|attack| attack.in_scope)
            .map(|attack| attack.attack_id.to_string())
            .collect();
        let out_of_scope: Vec<&str> = plan
            .iter()
            .filter(|attack| !attack.in_scope)
            .map(|attack| attack.attack_id)
            .collect();

        // Every seeded attack really is in the inventory, so an empty or narrowed
        // page is the report's doing and not the seeding's.
        let seeded_keys = with_store(&engine, |store| {
            store.shield_attacks.keys().cloned().collect::<Vec<String>>()
        });
        assert_eq!(
            seeded_keys.len(),
            plan.len(),
            "case {}: expected {} seeded attacks in the inventory, got {seeded_keys:?}",
            row.name,
            plan.len()
        );

        // Unbounded, then at page sizes that cut the row into pages of one, two
        // and three.
        for max_results in [None, Some(1), Some(2), Some(3)] {
            let case = format!("{}/MaxResults={max_results:?}", row.name);
            let served = page_through(&router, &case, &policy_id, max_results).await;
            assert_eq!(
                served, expected,
                "case {case}: expected only the attacks of the accounts the policy applies to \
                 ({expected:?}), got {served:?}"
            );
            assert!(
                !served.iter().any(|id| out_of_scope.contains(&id.as_str())),
                "case {case}: an attack of an account the policy does not apply to must not be \
                 reported, got {served:?} against the out-of-scope {out_of_scope:?}"
            );
        }

        // And the accounts the page names are exactly the accounts the policy
        // applies to that have an attack recorded.
        let case = format!("{}/accounts", row.name);
        let (status, body) = protection_status(&router, &policy_id, None, None).await;
        assert_eq!(
            status, 200,
            "case {case}: the policy-wide report must answer 200, got {status} with {body}"
        );
        let mut served_accounts = reported_accounts(&case, &body);
        served_accounts.sort();
        served_accounts.dedup();
        let mut expected_accounts: Vec<String> = plan
            .iter()
            .filter(|attack| attack.in_scope)
            .map(|attack| attack.account.to_string())
            .collect();
        expected_accounts.sort();
        expected_accounts.dedup();
        assert_eq!(
            served_accounts, expected_accounts,
            "case {case}: expected the page to name exactly {expected_accounts:?}, got {body}"
        );

        // A reporting operation writes nothing: the inventory is as it was
        // seeded.
        assert_eq!(
            with_store(&engine, |store| store
                .shield_attacks
                .keys()
                .cloned()
                .collect::<Vec<String>>()),
            seeded_keys,
            "case {}: the read must neither add nor drop an attack",
            row.name
        );
    }
}

/// The two halves of the surface agree about one (policy, account) pair: an
/// account the policy applies to is reported with its OWN attacks when it is
/// named and inside the policy-wide page, and an account the policy does not
/// apply to is the modelled not-found when it is named and absent from that page
/// — even though the inventory holds an attack filed under the policy for it.
#[tokio::test]
async fn a_named_account_and_the_policy_wide_page_agree() {
    for row in &rows() {
        let (router, _engine, policy_id, plan) = seeded_row(row).await;
        let case_row = row.name;

        let (status, body) = protection_status(&router, &policy_id, None, None).await;
        assert_eq!(
            status, 200,
            "case {case_row}: the policy-wide report must answer 200, got {status} with {body}"
        );
        let page = reported_ids(case_row, &body);

        for account in row.in_scope {
            let case = format!("{case_row}/{account}");
            let (status, body) = protection_status(&router, &policy_id, Some(account), None).await;
            assert_eq!(
                status, 200,
                "case {case}: an account the policy applies to must be reported, got {status} \
                 with {body}"
            );
            let expected: Vec<String> = plan
                .iter()
                .filter(|attack| attack.account == *account)
                .map(|attack| attack.attack_id.to_string())
                .collect();
            let served = reported_ids(&case, &body);
            assert_eq!(
                served, expected,
                "case {case}: expected this account's own attacks {expected:?}, got {served:?}"
            );
            // Whatever the named account is served is also inside the page the
            // same policy serves when no account is named.
            assert!(
                expected.iter().all(|id| page.contains(id)),
                "case {case}: the policy-wide page must carry this account's attacks {expected:?}, \
                 got {page:?}"
            );
        }

        for attack in plan.iter().filter(|attack| !attack.in_scope) {
            let case = format!("{case_row}/{}", attack.account);
            let (status, body) =
                protection_status(&router, &policy_id, Some(attack.account), None).await;
            assert_eq!(
                status, 400,
                "case {case}: an account the policy does not apply to must be refused, got \
                 {status} with {body}"
            );
            assert!(
                !page.contains(&attack.attack_id.to_string()),
                "case {case}: the policy-wide page must not serve the attack {:?} of an account \
                 the same policy reports as not found, got {page:?}",
                attack.attack_id
            );
        }
    }
}

/// The rule on the OTHER policy-wide reader of the same (policy, member account)
/// surface: `ListComplianceStatus` reports an entry for exactly the accounts the
/// policy applies to, so neither policy-wide report can name an account the other
/// one leaves out.
#[tokio::test]
async fn list_compliance_status_reports_only_accounts_the_policy_applies_to() {
    for row in &rows() {
        let (router, _engine, policy_id, _plan) = seeded_row(row).await;
        let case = row.name;

        let (status, body) = call(
            &router,
            "ListComplianceStatus",
            &json!({ "PolicyId": policy_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the compliance listing must answer 200, got {status} with {body}"
        );
        let mut served: Vec<String> = body["PolicyComplianceStatusList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no PolicyComplianceStatusList in {body}"))
            .iter()
            .map(|entry| {
                entry["MemberAccount"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: an entry carries no MemberAccount: {entry}")
                    })
                    .to_string()
            })
            .collect();
        served.sort();
        let mut expected: Vec<String> = row.in_scope.iter().map(|a| (*a).to_string()).collect();
        expected.sort();
        assert_eq!(
            served, expected,
            "case {case}: expected an entry for exactly {expected:?}, got {body}"
        );
    }
}

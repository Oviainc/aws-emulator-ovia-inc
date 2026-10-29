// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE resolution every reader that reports about a
//! (policy, member account) pair performs.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`], with the
//! `X-Amz-Target` header and a SigV4 credential scope — so the whole
//! deserialize → handle → serialize walk runs and the assertions are about the
//! WIRE response.
//!
//! The rule under test is one sentence: an operation that reports about a policy
//! together with a member account resolves that account against the accounts the
//! policy applies to, and answers the modelled `ResourceNotFoundException` when
//! the policy does not apply to it — Firewall Manager evaluates a policy only
//! against the accounts in the policy's own scope, so an account outside that
//! scope was never evaluated against the policy and the policy holds no report
//! of it at all.
//!
//! The readers the rule covers are the three operations whose input pairs a
//! `PolicyId` with a member account: `GetProtectionStatus`,
//! `GetComplianceDetail` and `GetViolationDetails`. They are asserted to agree,
//! so the answer a caller gets cannot depend on WHICH surface of the policy was
//! asked.
//!
//! The rows below are the class the rule ranges over: they differ in which scope
//! map (`IncludeMap`, `ExcludeMap`, neither) and which scope key (`ACCOUNT`,
//! `ORG_UNIT`) names the accounts, in whether the named values are account ids at
//! all, and in account-id length — including an `AWSAccountId` at the minimum
//! length the model publishes.
//!
//! The distinction the rule is about is the point of the 200 half of each row: an
//! account the policy DOES apply to, with nothing recorded against it, is a
//! successful empty report — which is exactly what an account outside the scope
//! must NOT be answered with.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness, the store seam, the organisation fixtures and the policy
// document builder live in one place each for this test target, so no two suites
// can drift in how a request is signed, the store is read or a policy is scoped.
use crate::administrator_gated_operations::with_store;
use crate::compliance_status::{associate, error_message, put_roster, ADMIN, ROSTER};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code};
use crate::protection_status::{data_entries, put_scoped_policy};

/// The security-service type `GetProtectionStatus` reports for ("If you created a
/// Shield Advanced policy, returns policy-level attack summary information …
/// Other policy types are currently unsupported"), so every row's policy is one:
/// the scope resolution must be asserted on a policy the operation serves.
const SHIELD_ADVANCED: &str = "SHIELD_ADVANCED";

/// An `AWSAccountId` (the model's `com.amazonaws.fms#AWSAccountId`: length
/// 1..1024, `^[0-9]+$`) that no row's scope names and that is in no stored
/// policy's maps, so it is an account of no organisation this store records.
const FOREIGN_ACCOUNT: &str = "999999999999";

/// An `AWSAccountId` at the MINIMUM length the model publishes (1).
const SHORTEST_ACCOUNT: &str = "7";

/// A resource `GetViolationDetails` can be asked about, so the third reader can
/// be asked about the pair at all. Which resource is named does not matter to
/// the rule: the account is resolved before any resource is looked up.
const A_RESOURCE_ID: &str = "i-0123456789abcdef0";

/// The organisation accounts [`put_roster`] contributes beside the
/// administrator, named one by one so a row's table can mention each.
const ROSTER_FIRST: &str = ROSTER[0];
const ROSTER_SECOND: &str = ROSTER[1];

/// One row of the scope table: the policy's scope maps, the accounts the policy
/// therefore applies to, and accounts it does not.
struct ScopeRow {
    name: &'static str,
    include: Option<Value>,
    exclude: Option<Value>,
    in_scope: &'static [&'static str],
    out_of_scope: &'static [&'static str],
}

/// The scope class the resolution ranges over.
///
/// Every row is read against the SAME organisation: the administrator
/// [`ADMIN`] plus the [`ROSTER`] a second stored policy contributes.
fn rows() -> Vec<ScopeRow> {
    vec![
        ScopeRow {
            name: "no-scope-maps",
            include: None,
            exclude: None,
            in_scope: &[ADMIN, ROSTER_FIRST, ROSTER_SECOND],
            out_of_scope: &[FOREIGN_ACCOUNT, SHORTEST_ACCOUNT],
        },
        ScopeRow {
            name: "included-accounts",
            include: Some(json!({ "ACCOUNT": ["222222222222", "333333333333"] })),
            exclude: None,
            in_scope: &["222222222222", "333333333333"],
            out_of_scope: &[ADMIN, "111111111111", FOREIGN_ACCOUNT],
        },
        ScopeRow {
            name: "included-account-at-the-minimum-length",
            include: Some(json!({ "ACCOUNT": [SHORTEST_ACCOUNT] })),
            exclude: None,
            in_scope: &[SHORTEST_ACCOUNT],
            out_of_scope: &[ADMIN, "111111111111"],
        },
        ScopeRow {
            name: "organizational-units-only",
            // An OU is "the equivalent of specifying all accounts in the OU and
            // in any of its child OUs", which is membership Organizations holds
            // and this emulator stands in for no Organizations — so the policy
            // applies to no account this store can name.
            include: Some(json!({ "ORG_UNIT": ["ou-abc1-12345678"] })),
            exclude: None,
            in_scope: &[],
            out_of_scope: &[ADMIN, ROSTER_FIRST, FOREIGN_ACCOUNT],
        },
        ScopeRow {
            name: "included-values-that-are-not-account-ids",
            // A `CustomerPolicyScopeId` is a free-form string, so a value that is
            // not an `AWSAccountId` names no account.
            include: Some(json!({ "ACCOUNT": ["not-an-account-id", "555555555555"] })),
            exclude: None,
            in_scope: &["555555555555"],
            out_of_scope: &[ADMIN, "1234"],
        },
        ScopeRow {
            name: "excluded-account-of-the-roster",
            include: None,
            exclude: Some(json!({ "ACCOUNT": [ROSTER_FIRST] })),
            in_scope: &[ADMIN, ROSTER_SECOND],
            out_of_scope: &[ROSTER_FIRST, FOREIGN_ACCOUNT],
        },
        ScopeRow {
            name: "excluded-accounts-and-organizational-units",
            include: None,
            exclude: Some(json!({
                "ACCOUNT": [ROSTER_SECOND],
                "ORG_UNIT": ["ou-abc1-11112222"]
            })),
            in_scope: &[ADMIN, ROSTER_FIRST],
            out_of_scope: &[ROSTER_SECOND],
        },
        ScopeRow {
            name: "include-map-that-names-nothing",
            // A map whose every id list is empty scopes the policy to nothing, so
            // it is no inclusion at all: the whole organisation applies.
            include: Some(json!({ "ACCOUNT": [] })),
            exclude: None,
            in_scope: &[ADMIN, ROSTER_FIRST, ROSTER_SECOND],
            out_of_scope: &[FOREIGN_ACCOUNT],
        },
    ]
}

/// A fresh emulator with the administrator associated, the organisation roster
/// recorded and one Shield Advanced policy stored with `row`'s scope maps.
///
/// Returns the router, the shared engine and that policy's `PolicyId`.
async fn seeded(row: &ScopeRow) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String) {
    // A fresh emulator per row, so one row's accounts cannot join another row's
    // organisation.
    let (router, engine) = emulator_with_engine();
    associate(&router, ADMIN).await;
    put_roster(&router).await;
    let policy_id = put_scoped_policy(
        &router,
        row.name,
        SHIELD_ADVANCED,
        row.include.clone(),
        row.exclude.clone(),
    )
    .await;
    (router, engine, policy_id)
}

/// `GetProtectionStatus` for one (policy, member account) pair.
async fn protection_status(
    router: &MultiServiceRouter,
    policy_id: &str,
    account: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "PolicyId": policy_id });
    if let Some(account) = account {
        body["MemberAccountId"] = json!(account);
    }
    call(router, "GetProtectionStatus", &body).await
}

/// Assert one response is the service's modelled not-found.
fn assert_not_found(case: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case}: an account the policy does not apply to must be the modelled not-found, \
         got {status} with {body}"
    );
    assert_eq!(
        error_message(body),
        state::RESOURCE_NOT_FOUND_MESSAGE,
        "case {case}: expected the service's not-found text, got {body}"
    );
}

/// The rule, on `GetProtectionStatus`: an account the policy applies to is a
/// successful report (empty, with nothing recorded against it), and an account it
/// does not apply to is the modelled not-found — never an empty success, which is
/// the one thing a caller is asking this operation to tell apart.
#[tokio::test]
async fn get_protection_status_resolves_the_member_account_against_the_policy_scope() {
    for row in &rows() {
        let (router, engine, policy_id) = seeded(row).await;

        for account in row.in_scope {
            let case = format!("{}/{account}", row.name);
            let (status, body) = protection_status(&router, &policy_id, Some(account)).await;
            assert_eq!(
                status, 200,
                "case {case}: an account the policy applies to must be reported, got {status} \
                 with {body}"
            );
            assert_eq!(
                data_entries(&case, &body),
                Vec::<Value>::new(),
                "case {case}: with no attack recorded the report must be empty, got {body}"
            );
            assert_eq!(
                (
                    body["AdminAccountId"].as_str(),
                    body["ServiceType"].as_str()
                ),
                (Some(ADMIN), Some(SHIELD_ADVANCED)),
                "case {case}: the report must name the administrator and the policy's own \
                 service type, got {body}"
            );
        }

        for account in row.out_of_scope {
            let case = format!("{}/{account}", row.name);
            let (status, body) = protection_status(&router, &policy_id, Some(account)).await;
            assert_not_found(&case, status, &body);
        }

        // The member account is an OPTIONAL member, so a request that names none
        // asks for no resolution and still reports the whole policy.
        let (status, body) = protection_status(&router, &policy_id, None).await;
        assert_eq!(
            status, 200,
            "case {}: a request naming no member account must still be reported, got {status} \
             with {body}",
            row.name
        );

        // A reporting operation writes nothing: no refusal and no report put an
        // attack in the inventory.
        let attacks = with_store(&engine, |store| store.shield_attacks.len());
        assert_eq!(
            attacks, 0,
            "case {}: the read must record no attack, got {attacks}",
            row.name
        );
    }
}

/// The same rule, on every reader of a (policy, member account) pair: the three
/// operations agree about which accounts a policy applies to, so the answer does
/// not depend on which surface of the policy was asked.
#[tokio::test]
async fn every_member_account_reader_of_a_policy_agrees_on_its_scope() {
    for row in &rows() {
        let (router, _engine, policy_id) = seeded(row).await;

        for account in row.out_of_scope {
            for (operation, request) in [
                (
                    "GetProtectionStatus",
                    json!({ "PolicyId": policy_id, "MemberAccountId": account }),
                ),
                (
                    "GetComplianceDetail",
                    json!({ "PolicyId": policy_id, "MemberAccount": account }),
                ),
                (
                    "GetViolationDetails",
                    json!({
                        "PolicyId": policy_id,
                        "MemberAccount": account,
                        "ResourceId": A_RESOURCE_ID,
                        "ResourceType": state::RESOURCE_TYPE_EC2_INSTANCE,
                    }),
                ),
            ] {
                let case = format!("{}/{operation}/{account}", row.name);
                let (status, body) = call(&router, operation, &request).await;
                assert_not_found(&case, status, &body);
            }
        }

        // And an account the policy DOES apply to is served by both readers that
        // report without a resource identifier. (`GetViolationDetails` names a
        // resource too, which has its own not-found until a violation of it is
        // recorded — the `violation_details` suite owns that half.)
        for account in row.in_scope {
            for (operation, request) in [
                (
                    "GetProtectionStatus",
                    json!({ "PolicyId": policy_id, "MemberAccountId": account }),
                ),
                (
                    "GetComplianceDetail",
                    json!({ "PolicyId": policy_id, "MemberAccount": account }),
                ),
            ] {
                let case = format!("{}/{operation}/{account}", row.name);
                let (status, body) = call(&router, operation, &request).await;
                assert_eq!(
                    status, 200,
                    "case {case}: an account the policy applies to must be reported, got \
                     {status} with {body}"
                );
            }
        }
    }
}

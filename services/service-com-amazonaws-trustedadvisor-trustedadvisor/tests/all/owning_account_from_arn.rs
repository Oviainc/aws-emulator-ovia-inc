// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE derivation both organization-level views of a
//! stored resource report their `accountId` from
//! (`service_com_amazonaws_trustedadvisor_trustedadvisor::state::arn_account_id`),
//! driven over HTTP through the same `MultiServiceRouter` the emulator serves
//! with.
//!
//! The rule: the account a view reports for a stored resource is the account
//! segment of that resource's OWN ARN, and no account at all when the ARN
//! carries none. Both organization-level lists derive it the same way, so the
//! account-level and the organization-level view of one stored resource can
//! never report different owners:
//!
//! * `ListOrganizationRecommendationAccounts` reports it per member account
//!   recommendation, from that recommendation's ARN;
//! * `ListOrganizationRecommendationResources` reports it per member resource,
//!   from that resource's ARN.
//!
//! Both members are modelled `AccountId` (`length 12..12, pattern ^\d+$`) and
//! both ARNs are modelled with a 12-digit account segment
//! (`AccountRecommendationArn`, `RecommendationResourceArn`:
//! `smithy_query(query="shape",shape="AccountRecommendationLifecycleSummary,OrganizationRecommendationResourceSummary",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`),
//! so the rows below span several 12-digit accounts; the accountless forms this
//! service also holds (a check ARN carries no account segment) are pinned on the
//! shared derivation itself, which is where that half of the rule lives.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler is pinned in
//! `tests/all/support_level_gate.rs`.

use std::collections::BTreeMap;

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{
    arn_account_id, service_state, SupportPlan,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::{get_path, organization_recommendation_arn};
use crate::recommendation_loop::{call, emulator_on_plan, label, recommendation_arn};

/// The check both lists are read for: its recommendation is `priority`, the only
/// type with an organization recommendation, and it has three seeded resources
/// so a member account contributes more than one row. Source: the `SEED` table
/// in `src/state.rs`.
const CHECK: &str = "low-utilization-ec2-instances";

/// The ids of [`CHECK`]'s seeded resources, written out by hand; their source is
/// the `SEED` table in `src/state.rs`. Each member account below holds a copy of
/// every one of them, under its own ARN.
const RESOURCE_IDS: &[&str] = &["res-ec2-0001", "res-ec2-0002", "res-ec2-0003"];

/// The member accounts of the organization whose recommendations and resources
/// the two lists report: the emulator's own account (the seeded member) plus
/// three enrolled ones, differing along the account value itself — two
/// unrelated ids and one with leading zeros, every one a 12-digit `AccountId`.
fn member_accounts() -> Vec<&'static str> {
    vec![
        engine::EMULATOR_ACCOUNT_ID,
        "111122223333",
        "999988887777",
        "000000000012",
    ]
}

/// A mounted emulator on a plan that includes the Trusted Advisor API, with the
/// engine it was built on so a test can enrol organization member accounts the
/// way something outside this API would.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    emulator_on_plan(SupportPlan::Business)
}

/// The modelled URI of `ListOrganizationRecommendationAccounts` for [`CHECK`]'s
/// organization recommendation.
fn accounts_path() -> String {
    format!(
        "/v1/organization-recommendations/{}/accounts",
        label(&organization_recommendation_arn(CHECK))
    )
}

/// The modelled URI of `ListOrganizationRecommendationResources` for [`CHECK`]'s
/// organization recommendation.
fn resources_path() -> String {
    format!(
        "/v1/organization-recommendations/{}/resources",
        label(&organization_recommendation_arn(CHECK))
    )
}

/// `arn:aws:trustedadvisor::<account>:recommendation/<check>-recommendation` —
/// the ARN pattern `AccountRecommendationArn` declares, for any member account
/// of the organization (the emulator's own included).
fn member_recommendation_arn(account_id: &str, check_id: &str) -> String {
    format!("arn:aws:trustedadvisor::{account_id}:recommendation/{check_id}-recommendation")
}

/// `arn:aws:trustedadvisor::<account>:recommendation-resource/<rec>/<resource>`
/// — the ARN pattern `RecommendationResourceArn` declares, for any member
/// account of the organization.
fn member_resource_arn(account_id: &str, check_id: &str, resource_id: &str) -> String {
    format!(
        "arn:aws:trustedadvisor::{account_id}:recommendation-resource/\
         {check_id}-recommendation/{resource_id}"
    )
}

/// Enrol one more member account of the organization: a copy of `check_id`'s
/// seeded recommendation and of every one of its resources, owned by
/// `account_id` and aggregated by that check's organization recommendation.
///
/// Written in process because no modelled operation of this API enrols an
/// account or produces its check results — organization membership comes from
/// AWS Organizations, outside this service — exactly as the sibling suites do
/// it. The member list is grown in an order that is not ARN order, so the rows
/// below depend on nothing but each stored ARN.
fn enrol_member_account(engine: &engine::InMemoryEngine, check_id: &str, account_id: &str) {
    let seeded_arn = recommendation_arn(check_id);
    let organization_arn = organization_recommendation_arn(check_id);
    let state = service_state(engine).expect("the in-memory engine carries this service's state");
    let mut state = state.lock();

    let mut member = state
        .recommendations
        .get(&seeded_arn)
        .unwrap_or_else(|| panic!("no seeded recommendation for {check_id}"))
        .clone();
    member.arn = member_recommendation_arn(account_id, check_id);
    let member_arn = member.arn.clone();

    let copies: Vec<_> = state
        .resources_of(&seeded_arn)
        .map(|(_, resource)| {
            let mut copy = resource.clone();
            copy.arn = member_resource_arn(account_id, check_id, &copy.id);
            copy.recommendation_arn = member_arn.clone();
            copy
        })
        .collect();

    state.recommendations.insert(member_arn.clone(), member);
    for copy in copies {
        state.recommendation_resources.insert(copy.arn.clone(), copy);
    }
    state
        .organization_recommendations
        .get_mut(&organization_arn)
        .unwrap_or_else(|| panic!("no organization recommendation for {check_id}"))
        .member_recommendation_arns
        .push(member_arn);
}

/// The body one read reports, asserting 200 first.
async fn read(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} with {body}"
    );
    body
}

/// The `accountId` each summary of `list` reports, keyed by the summary's own
/// ARN member (`arn_member`). `None` is an absent `accountId`.
async fn reported_accounts(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    list: &str,
    arn_member: &str,
) -> BTreeMap<String, Option<String>> {
    let body = read(router, case, path).await;
    body[list]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no {list} in {body}"))
        .iter()
        .map(|summary| {
            let arn = summary[arn_member].as_str().unwrap_or_else(|| {
                panic!("case {case}: a {list} item has no {arn_member}: {summary}")
            });
            (
                arn.to_string(),
                summary["accountId"].as_str().map(str::to_string),
            )
        })
        .collect()
}

#[tokio::test]
async fn both_organization_views_report_the_account_that_owns_each_stored_arn() {
    let (router, engine) = emulator();
    let accounts = member_accounts();
    for account in accounts
        .iter()
        .filter(|account| **account != engine::EMULATOR_ACCOUNT_ID)
    {
        enrol_member_account(&engine, CHECK, account);
    }

    // Rows, written out by hand: one member recommendation per member account,
    // and one resource per member account per seeded resource id. The account
    // each row expects is the account it was stored under.
    let expected_recommendations: BTreeMap<String, Option<String>> = accounts
        .iter()
        .map(|account| {
            (
                member_recommendation_arn(account, CHECK),
                Some((*account).to_string()),
            )
        })
        .collect();
    let expected_resources: BTreeMap<String, Option<String>> = accounts
        .iter()
        .flat_map(|account| {
            RESOURCE_IDS.iter().map(move |id| {
                (
                    member_resource_arn(account, CHECK, id),
                    Some((*account).to_string()),
                )
            })
        })
        .collect();

    // `ListOrganizationRecommendationAccounts`: the account of each member
    // recommendation, from that recommendation's own ARN.
    let listed_recommendations = reported_accounts(
        &router,
        "accounts",
        &format!("{}?maxResults=200", accounts_path()),
        "accountRecommendationLifecycleSummaries",
        "accountRecommendationArn",
    )
    .await;
    assert_eq!(
        listed_recommendations, expected_recommendations,
        "case accounts: expected {expected_recommendations:?}, got {listed_recommendations:?}"
    );

    // `ListOrganizationRecommendationResources`: the account of each member
    // resource, from that resource's own ARN.
    let listed_resources = reported_accounts(
        &router,
        "resources",
        &format!("{}?maxResults=200", resources_path()),
        "organizationRecommendationResourceSummaries",
        "arn",
    )
    .await;
    assert_eq!(
        listed_resources, expected_resources,
        "case resources: expected {expected_resources:?}, got {listed_resources:?}"
    );

    // And the two views of one member account agree: the owner the accounts
    // view reports for a member recommendation is the owner the resources view
    // reports for every resource of that same member.
    for account in &accounts {
        let case = format!("agreement/{account}");
        let from_accounts = listed_recommendations
            .get(&member_recommendation_arn(account, CHECK))
            .unwrap_or_else(|| panic!("case {case}: the accounts view reports no such member"));
        for id in RESOURCE_IDS {
            let from_resources = listed_resources
                .get(&member_resource_arn(account, CHECK, id))
                .unwrap_or_else(|| {
                    panic!("case {case}/{id}: the resources view reports no such resource")
                });
            assert_eq!(
                from_accounts, from_resources,
                "case {case}/{id}: expected both views to report {from_accounts:?}, the resources \
                 view reports {from_resources:?}"
            );
        }
    }

    // A read reports; it changes nothing a later caller sees, so the same two
    // reads answer the same owners again.
    let again = reported_accounts(
        &router,
        "accounts/again",
        &format!("{}?maxResults=200", accounts_path()),
        "accountRecommendationLifecycleSummaries",
        "accountRecommendationArn",
    )
    .await;
    assert_eq!(
        again, expected_recommendations,
        "case accounts/again: expected {expected_recommendations:?}, got {again:?}"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_organization_recommendation_reports_no_account() {
    let (router, _engine) = emulator();
    // The shared derivation is reached only through a resolved organization
    // recommendation: an identifier naming none is 404 in both views, so neither
    // reports an account for a resource the store does not hold.
    let missing = label(&organization_recommendation_arn("no-such-check"));
    for (case, path) in [
        (
            "accounts",
            format!("/v1/organization-recommendations/{missing}/accounts"),
        ),
        (
            "resources",
            format!("/v1/organization-recommendations/{missing}/resources"),
        ),
    ] {
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404 for an identifier naming no organization recommendation, \
             got {status} with {body}"
        );
    }
    // The seeded organization recommendation is still readable, so the refusals
    // above left the store as it was.
    let (status, body) = call(&router, "GET", &get_path(CHECK), None).await;
    assert_eq!(
        status, 200,
        "case seeded: expected the seeded organization recommendation to still read 200, got \
         {status} with {body}"
    );
}

#[test]
fn the_shared_derivation_reports_no_account_for_an_arn_that_carries_none() {
    // Rows: the ARN forms this service holds, from the patterns the model
    // declares — a member recommendation and a member resource carry a 12-digit
    // account segment, while a check ARN and an organization recommendation ARN
    // carry an EMPTY one (`CheckSummary$arn`,
    // `OrganizationRecommendationArn`), as does another service's accountless
    // resource ARN — plus strings with fewer segments than an ARN's account.
    let rows: &[(&str, Option<&str>)] = &[
        (
            "arn:aws:trustedadvisor::123456789012:recommendation/a-check-recommendation",
            Some("123456789012"),
        ),
        (
            "arn:aws:trustedadvisor::000000000012:recommendation-resource/a-check-recommendation/res-0001",
            Some("000000000012"),
        ),
        ("arn:aws:trustedadvisor:::check/a-check", None),
        (
            "arn:aws:trustedadvisor:::organization-recommendation/a-check-organization-recommendation",
            None,
        ),
        ("arn:aws:s3:::a-bucket", None),
        // The fifth segment of a four-colon string is EMPTY, so there is no
        // account here either.
        ("arn:aws:trustedadvisor::", None),
        ("arn:aws:trustedadvisor:us-east-1", None),
        ("arn", None),
        ("", None),
    ];
    for (arn, expected) in rows {
        let actual = arn_account_id(arn);
        assert_eq!(
            actual.as_deref(),
            *expected,
            "case {arn:?}: expected the owning account {expected:?}, got {actual:?}"
        );
    }
}

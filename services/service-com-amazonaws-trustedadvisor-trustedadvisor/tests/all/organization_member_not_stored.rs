// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE rule every organization-level view of an
//! organization recommendation obeys about the members it is derived from,
//! driven over HTTP through the same `MultiServiceRouter` the emulator serves
//! with:
//!
//! * **a member recommendation ARN the store holds no recommendation for names
//!   nothing, so it contributes no row and no count to ANY organization-level
//!   view** — no `AccountRecommendationLifecycleSummary` of
//!   `ListOrganizationRecommendationAccounts`, no
//!   `OrganizationRecommendationResourceSummary` of
//!   `ListOrganizationRecommendationResources` (whatever resource rows are
//!   stored under that ARN), and nothing in
//!   `GetOrganizationRecommendation`'s `resourcesAggregates` or
//!   `pillarSpecificAggregates`;
//! * so the accounts view, the resources view and the aggregates of one
//!   organization recommendation always agree: every `accountId` the resources
//!   view reports is an account the accounts view reports as a member, and the
//!   counters count exactly the resources the resources view lists, at every
//!   page size.
//!
//! The rows below differ along the number of member accounts, which named
//! member the store holds no recommendation for (the first in ARN order, a
//! middle one, the last, every one of them, none at all) and how many of them
//! are unresolvable; the whole table is driven for each of the two seeded
//! checks that have an organization recommendation, which differ in how many
//! resources and what cost totals one member contributes.
//!
//! What ONE member contributes is not written out here: it is read from the
//! member's OWN `GetRecommendation` and `ListRecommendationResources` on a
//! pristine emulator, so these expectations stay the account-level view of the
//! same store.
//!
//! Organization membership comes from AWS Organizations, outside this service,
//! and no modelled operation of this API enrols an account, produces its check
//! results or deletes a recommendation, so a member — and a member ARN the
//! store holds no recommendation for — is written in process, exactly as
//! `tests/all/organization_member_set.rs` writes a repeated member.
//!
//! The modelled URIs, members and statuses come from the model:
//! `smithy_query(query="operation",operation="ListOrganizationRecommendationResources,ListOrganizationRecommendationAccounts,GetOrganizationRecommendation",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`.

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::{organization_recommendation_arn, read};
use crate::recommendation_loop::{
    call, emulator_on_plan, field_of_each, label, recommendation_arn, resource_arn,
};

/// The seeded checks whose recommendation is prioritized, and so has an
/// organization recommendation: the only ones the organization operations
/// resolve at all. Written out by hand; their source is the `SEED` table in
/// `src/state.rs`, and they carry a different number of resources each.
const CHECKS: &[&str] = &["low-utilization-ec2-instances", "rds-idle-db-instances"];

/// The emulator's own account, the one the seed enrolled as each organization
/// recommendation's single member. Its ARN sorts FIRST of the accounts below.
const OWN: &str = engine::EMULATOR_ACCOUNT_ID;

/// One row of the table: the member accounts of the organization, each with
/// whether the store holds that account's member recommendation.
struct Row {
    case: &'static str,
    named: &'static [(&'static str, bool)],
}

const ROWS: &[Row] = &[
    Row {
        case: "the only member's recommendation is not stored",
        named: &[("111122223333", false)],
    },
    Row {
        case: "the only member's recommendation is stored",
        named: &[(OWN, true)],
    },
    Row {
        case: "the first member in ARN order is not stored",
        named: &[(OWN, false), ("111122223333", true), ("999988887777", true)],
    },
    Row {
        case: "a middle member is not stored",
        named: &[(OWN, true), ("111122223333", false), ("999988887777", true)],
    },
    Row {
        case: "the last member in ARN order is not stored",
        named: &[(OWN, true), ("111122223333", true), ("999988887777", false)],
    },
    Row {
        case: "no member's recommendation is stored",
        named: &[(OWN, false), ("111122223333", false), ("999988887777", false)],
    },
    Row {
        case: "four members, every recommendation stored",
        named: &[
            (OWN, true),
            ("111122223333", true),
            ("444455556666", true),
            ("999988887777", true),
        ],
    },
    Row {
        case: "four members, two recommendations not stored",
        named: &[
            (OWN, true),
            ("111122223333", false),
            ("444455556666", true),
            ("777788889999", false),
        ],
    },
];

/// `arn:aws:trustedadvisor::<account>:recommendation/<check>-recommendation` —
/// the ARN pattern `AccountRecommendationArn` declares.
fn member_arn(account_id: &str, check: &str) -> String {
    if account_id == OWN {
        return recommendation_arn(check);
    }
    format!("arn:aws:trustedadvisor::{account_id}:recommendation/{check}-recommendation")
}

/// `arn:aws:trustedadvisor::<account>:recommendation-resource/<rec>/<resource>`
/// — the ARN pattern `RecommendationResourceArn` declares.
fn member_resource_arn(account_id: &str, check: &str, resource_id: &str) -> String {
    if account_id == OWN {
        return resource_arn(check, resource_id);
    }
    format!(
        "arn:aws:trustedadvisor::{account_id}:recommendation-resource/\
         {check}-recommendation/{resource_id}"
    )
}

/// What ONE member recommendation of a check contributes, read from the
/// account-level view of the pristine seed rather than written out here.
struct Baseline {
    /// The resource ids of that member's recommendation, as
    /// `ListRecommendationResources` reports them.
    resource_ids: Vec<String>,
    /// `resourcesAggregates` as `(okCount, warningCount, errorCount,
    /// excludedCount)`.
    counts: [u64; 4],
    /// `pillarSpecificAggregates.costOptimizing.estimatedMonthlySavings`.
    savings: f64,
}

/// The body one read reports, asserting 200 first.
async fn ok_body(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from GET {path}, got {status} with {body}"
    );
    body
}

/// Read one member's own contribution off a pristine emulator: the seeded
/// account-level recommendation of `check` as its own operations report it.
async fn baseline(check: &str) -> Baseline {
    let (router, _engine) = emulator_on_plan(SupportPlan::Business);
    let arn = recommendation_arn(check);
    let resources = ok_body(
        &router,
        check,
        &format!("/v1/recommendations/{}/resources", label(&arn)),
    )
    .await;
    let resource_ids = field_of_each(&resources, "recommendationResourceSummaries", "id");
    let got = ok_body(&router, check, &format!("/v1/recommendations/{}", label(&arn))).await;
    let recommendation = &got["recommendation"];
    let aggregates = &recommendation["resourcesAggregates"];
    let mut counts = [0u64; 4];
    for (index, counter) in ["okCount", "warningCount", "errorCount", "excludedCount"]
        .iter()
        .enumerate()
    {
        counts[index] = aggregates[counter]
            .as_u64()
            .unwrap_or_else(|| panic!("case {check}: expected a {counter}, got {aggregates}"));
    }
    let savings = recommendation["pillarSpecificAggregates"]["costOptimizing"]
        ["estimatedMonthlySavings"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {check}: expected cost savings, got {recommendation}"));
    Baseline {
        resource_ids,
        counts,
        savings,
    }
}

/// Write the row's membership for `check`: every named account gets a copy of
/// the seeded recommendation's RESOURCES, and only an account the row marks
/// stored gets the member recommendation itself — the state a member enrolment
/// that records findings without the recommendation leaves.
///
/// The stored member list is left in an order that is NOT ARN order, so the
/// sequence under test is the handler's.
fn enrol(engine: &engine::InMemoryEngine, check: &str, named: &[(&str, bool)]) {
    let seeded_arn = recommendation_arn(check);
    let organization_arn = organization_recommendation_arn(check);
    let state = service_state(engine).expect("the in-memory engine carries this service's state");
    let mut state = state.lock();

    let seeded = state
        .recommendations
        .get(&seeded_arn)
        .unwrap_or_else(|| panic!("no seeded recommendation for {check}"))
        .clone();
    let seeded_resources: Vec<_> = state
        .resources_of(&seeded_arn)
        .map(|(_, resource)| resource.clone())
        .collect();

    let mut members: Vec<String> = Vec::new();
    for (account, stored) in named {
        let arn = member_arn(account, check);
        for resource in &seeded_resources {
            let mut copy = resource.clone();
            copy.arn = member_resource_arn(account, check, &copy.id);
            copy.recommendation_arn = arn.clone();
            state.recommendation_resources.insert(copy.arn.clone(), copy);
        }
        if *stored {
            let mut member = seeded.clone();
            member.arn = arn.clone();
            state.recommendations.insert(arn.clone(), member);
        } else {
            state.recommendations.remove(&arn);
        }
        members.push(arn);
    }
    // Reverse ARN order, so no handler can get the page order for free.
    members.reverse();
    state
        .organization_recommendations
        .get_mut(&organization_arn)
        .unwrap_or_else(|| panic!("no organization recommendation for {check}"))
        .member_recommendation_arns = members;
}

/// The accounts of a row whose member recommendation the store holds.
fn stored_accounts<'a>(named: &'a [(&'a str, bool)]) -> Vec<&'a str> {
    named
        .iter()
        .filter(|(_, stored)| *stored)
        .map(|(account, _)| *account)
        .collect()
}

/// The member recommendations a row's organization recommendation really
/// aggregates, in the ARN order every organization list pages in.
fn expected_members(named: &[(&str, bool)], check: &str) -> Vec<String> {
    let mut arns: Vec<String> = stored_accounts(named)
        .into_iter()
        .map(|account| member_arn(account, check))
        .collect();
    arns.sort();
    arns.dedup();
    arns
}

/// The resources those member recommendations hold, in ARN order.
fn expected_resources(named: &[(&str, bool)], check: &str, baseline: &Baseline) -> Vec<String> {
    let mut arns: Vec<String> = stored_accounts(named)
        .into_iter()
        .flat_map(|account| {
            baseline
                .resource_ids
                .iter()
                .map(move |id| member_resource_arn(account, check, id))
        })
        .collect();
    arns.sort();
    arns.dedup();
    arns
}

/// The modelled URI of `ListOrganizationRecommendationAccounts` for one check's
/// organization recommendation.
fn accounts_path(check: &str) -> String {
    format!(
        "/v1/organization-recommendations/{}/accounts",
        label(&organization_recommendation_arn(check))
    )
}

/// The modelled URI of `ListOrganizationRecommendationResources` for one
/// check's organization recommendation.
fn resources_path(check: &str) -> String {
    format!(
        "/v1/organization-recommendations/{}/resources",
        label(&organization_recommendation_arn(check))
    )
}

/// Follow `nextToken` from the first page to the last at `page_size`, and
/// report the `field` of every entry of the `list` the walk saw, in order.
async fn paged_walk(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    list: &str,
    field: &str,
    page_size: usize,
    expected_len: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One round beyond the number of pages the size implies, so a token that
    // never clears is caught as a loop rather than hanging.
    for _ in 0..=expected_len + 1 {
        let query = match &token {
            Some(value) => format!("?maxResults={page_size}&nextToken={}", label(value)),
            None => format!("?maxResults={page_size}"),
        };
        let body = ok_body(router, case, &format!("{path}{query}")).await;
        seen.extend(field_of_each(&body, list, field));
        token = body["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {case}: expected the token to clear, got {token:?}"
    );
    seen
}

#[tokio::test]
async fn a_member_the_store_holds_no_recommendation_for_lists_no_resource() {
    for check in CHECKS {
        let baseline = baseline(check).await;
        for row in ROWS {
            let case = format!("{} of {check}", row.case);
            let (router, engine) = emulator_on_plan(SupportPlan::Business);
            enrol(&engine, check, row.named);
            let expected = expected_resources(row.named, check, &baseline);

            let body = ok_body(&router, &case, &resources_path(check)).await;
            let listed = field_of_each(&body, "organizationRecommendationResourceSummaries", "arn");
            assert_eq!(
                listed, expected,
                "case {case}: expected only the stored members' resources in ARN order, got \
                 {listed:?}"
            );

            // The two organization-level views of one aggregate agree: every
            // account the resources view names is a member the accounts view
            // reports.
            let members = field_of_each(
                &ok_body(&router, &case, &accounts_path(check)).await,
                "accountRecommendationLifecycleSummaries",
                "accountId",
            );
            let owners = field_of_each(
                &body,
                "organizationRecommendationResourceSummaries",
                "accountId",
            );
            for owner in &owners {
                assert!(
                    members.contains(owner),
                    "case {case}: expected every reported resource's accountId to be a listed \
                     member, got {owner} outside {members:?}"
                );
            }

            for page_size in [1usize, 2, 5, 200] {
                let case = format!("{case} at maxResults={page_size}");
                let seen = paged_walk(
                    &router,
                    &case,
                    &resources_path(check),
                    "organizationRecommendationResourceSummaries",
                    "arn",
                    page_size,
                    expected.len(),
                )
                .await;
                assert_eq!(
                    seen, expected,
                    "case {case}: expected the paged walk to report the same sequence, got \
                     {seen:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_member_the_store_holds_no_recommendation_for_is_no_account_and_no_count() {
    for check in CHECKS {
        let baseline = baseline(check).await;
        for row in ROWS {
            let case = format!("{} of {check}", row.case);
            let (router, engine) = emulator_on_plan(SupportPlan::Business);
            enrol(&engine, check, row.named);
            let expected = expected_members(row.named, check);
            let stored = expected.len() as u64;

            let listed = field_of_each(
                &ok_body(&router, &case, &accounts_path(check)).await,
                "accountRecommendationLifecycleSummaries",
                "accountRecommendationArn",
            );
            assert_eq!(
                listed, expected,
                "case {case}: expected only the stored member recommendations in ARN order, got \
                 {listed:?}"
            );

            let organization = read(&router, &case, check).await;
            let aggregates = &organization["resourcesAggregates"];
            for (index, counter) in ["okCount", "warningCount", "errorCount", "excludedCount"]
                .iter()
                .enumerate()
            {
                let want = baseline.counts[index] * stored;
                assert_eq!(
                    aggregates[counter].as_u64(),
                    Some(want),
                    "case {case}: expected resourcesAggregates.{counter} {want} over {stored} \
                     stored member recommendations, got {:?}",
                    aggregates[counter]
                );
            }

            // The counters and the resources view are two views of one store,
            // so they agree on how many resources the organization has.
            let counted: u64 = baseline.counts[..3].iter().sum::<u64>() * stored;
            let resources = field_of_each(
                &ok_body(&router, &case, &resources_path(check)).await,
                "organizationRecommendationResourceSummaries",
                "arn",
            );
            assert_eq!(
                resources.len() as u64,
                counted,
                "case {case}: expected resourcesAggregates to count the {} listed resources, got \
                 {counted}",
                resources.len()
            );

            // The cost aggregate is summed over the same members, and an
            // organization recommendation no stored member optimizes cost for
            // reports no pillar aggregate at all.
            let cost = &organization["pillarSpecificAggregates"]["costOptimizing"];
            if stored == 0 {
                assert!(
                    organization["pillarSpecificAggregates"].is_null(),
                    "case {case}: expected no pillarSpecificAggregates, got {:?}",
                    organization["pillarSpecificAggregates"]
                );
            } else {
                let want = baseline.savings * stored as f64;
                let got = cost["estimatedMonthlySavings"].as_f64();
                assert!(
                    got.is_some_and(|got| (got - want).abs() < 1e-6),
                    "case {case}: expected estimatedMonthlySavings {want} over {stored} stored \
                     member recommendations, got {got:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn filtering_to_a_member_the_store_holds_no_recommendation_for_reports_nothing() {
    for check in CHECKS {
        let baseline = baseline(check).await;
        for row in ROWS {
            let (router, engine) = emulator_on_plan(SupportPlan::Business);
            enrol(&engine, check, row.named);

            for (account, stored) in row.named {
                let case = format!("{} of {check} filtered to {account}", row.case);
                let resources = field_of_each(
                    &ok_body(
                        &router,
                        &case,
                        &format!("{}?affectedAccountId={account}", resources_path(check)),
                    )
                    .await,
                    "organizationRecommendationResourceSummaries",
                    "arn",
                );
                let members = field_of_each(
                    &ok_body(
                        &router,
                        &case,
                        &format!("{}?affectedAccountId={account}", accounts_path(check)),
                    )
                    .await,
                    "accountRecommendationLifecycleSummaries",
                    "accountRecommendationArn",
                );

                let expected_member: Vec<String> = if *stored {
                    vec![member_arn(account, check)]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    members, expected_member,
                    "case {case}: expected {expected_member:?}, got {members:?}"
                );

                let mut expected_resource: Vec<String> = if *stored {
                    baseline
                        .resource_ids
                        .iter()
                        .map(|id| member_resource_arn(account, check, id))
                        .collect()
                } else {
                    Vec::new()
                };
                expected_resource.sort();
                assert_eq!(
                    resources, expected_resource,
                    "case {case}: expected {expected_resource:?}, got {resources:?}"
                );
            }
        }
    }
}
